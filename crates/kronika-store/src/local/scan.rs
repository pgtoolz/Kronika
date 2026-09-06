//! The bounded journal scan and the cached completion built on top of it.

use std::io;
use std::path::Path;
use std::sync::Arc;

use kronika_format::{JOURNAL_HEADER_LEN, MAX_JOURNAL_PARTS, MAX_PART_LEN, ReadAt};
use kronika_layout::FileIdentity;

use crate::catalog_summary::CatalogDigest;
use crate::source::{
    ActiveJournalWarningReason, ActivePart, FinalUnit, InvalidZmsReason, JournalScan, LocalScan,
    StoreIoFailure, StoreIoOperation, StoreObject, StoreWarning, StoreWarningReason,
};

use super::budget::{
    accounted_scan_metadata_bytes, active_metadata_bytes, advance_previous, catalog_metadata_bytes,
    ensure_active_part_budget, ensure_retained_metadata, ensure_scan_metadata_budget, layout_io,
    metadata_limit_io, push_warning_bounded, reserve_active_slots, retained_metadata_with_warnings,
    summary_allocation_bytes,
};
use super::journal::{
    PrefixReader, active_journal_source, active_part_limit_io, is_stale_journal, read_journal_plan,
    visit_journal_frames,
};
use super::segment::{
    FinishedValidation, ZmsInvalid, ZmsOpen, classify_zms_validation, invalid_zms_warning,
    read_zms_summary,
};
use super::{ACTIVE_ARC_ALLOCATION_BYTES, LocalDir};

fn active_growth(capacity: usize, len: usize, max_parts: usize) -> Option<usize> {
    let available = max_parts.checked_sub(len)?;
    let additional = capacity.max(4).min(available);
    (additional != 0).then_some(additional)
}

impl LocalDir {
    #[expect(
        clippy::rc_buffer,
        reason = "incremental append needs Arc::make_mut while unchanged scans retain the Vec allocation"
    )]
    pub(super) fn scan_journal_reader_from<R: ReadAt>(
        &self,
        reader: &R,
        start_at: u64,
        prev_active: Arc<Vec<ActivePart>>,
        journal_path: &Path,
    ) -> io::Result<JournalScan> {
        self.scan_journal_reader_bounded_from(
            reader,
            start_at,
            prev_active,
            journal_path,
            MAX_JOURNAL_PARTS,
        )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "journal admission, validation, and retained-budget accounting are one transaction"
    )]
    #[expect(
        clippy::rc_buffer,
        reason = "incremental append needs Arc::make_mut while unchanged scans retain the Vec allocation"
    )]
    pub(super) fn scan_journal_reader_bounded_from<R: ReadAt>(
        &self,
        reader: &R,
        start_at: u64,
        prev_active: Arc<Vec<ActivePart>>,
        journal_path: &Path,
        max_parts: usize,
    ) -> io::Result<JournalScan> {
        let plan = match read_journal_plan(reader) {
            Ok(plan) => plan,
            Err(error) if is_stale_journal(&error) => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    format!(
                        "{} changed while reading its header: {error}",
                        journal_path.display()
                    ),
                ));
            }
            Err(error) => return Err(error),
        };
        let Some(segment_id) = plan.segment_id else {
            if ACTIVE_ARC_ALLOCATION_BYTES > self.limits.max_metadata_bytes {
                return Err(metadata_limit_io(self.limits.max_metadata_bytes));
            }
            return Ok(JournalScan {
                active: Arc::new(Vec::new()),
                valid_len: plan.valid_len,
                committed_reset: false,
                metadata_bytes: ACTIVE_ARC_ALLOCATION_BYTES,
            });
        };
        let body_reader = PrefixReader {
            inner: reader,
            len: plan.scan_len,
        };
        let previous_matches = prev_active
            .first()
            .is_none_or(|part| part.segment_id == segment_id);
        let can_resume = !plan.committed_reset
            && previous_matches
            && start_at >= JOURNAL_HEADER_LEN as u64
            && start_at <= plan.scan_len;
        let start_at = if can_resume {
            start_at
        } else {
            JOURNAL_HEADER_LEN as u64
        };
        let prev_active = if can_resume {
            prev_active
        } else {
            Arc::new(Vec::new())
        };
        let Some(remaining_parts) = max_parts.checked_sub(prev_active.len()) else {
            return Err(active_part_limit_io(journal_path, max_parts));
        };
        let mut active = prev_active;
        let mut active_metadata = active_metadata_bytes(&active, active.capacity())?;
        let mut previous_active_metadata = 0_usize;
        let scanned_valid_len = if plan.committed_reset {
            visit_journal_frames(
                &body_reader,
                start_at,
                remaining_parts,
                max_parts,
                journal_path,
                |_part_ref, _catalog, _part_buffer_capacity| Ok(()),
            )?
        } else {
            visit_journal_frames(
                &body_reader,
                start_at,
                remaining_parts,
                max_parts,
                journal_path,
                |part_ref, catalog, part_buffer_capacity| {
                    if part_ref.len as u64 > MAX_PART_LEN {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "active part at offset {} exceeds the max part length",
                                part_ref.offset
                            ),
                        ));
                    }
                    let part_metadata = catalog_metadata_bytes(&catalog)?;
                    if previous_active_metadata == 0 && Arc::strong_count(&active) > 1 {
                        previous_active_metadata = active_metadata;
                    }
                    let needs_capacity = active.len() == active.capacity();
                    let needs_detach = Arc::strong_count(&active) > 1;
                    if needs_capacity || needs_detach {
                        let additional = if needs_capacity {
                            active_growth(active.capacity(), active.len(), max_parts)
                                .ok_or_else(|| active_part_limit_io(journal_path, max_parts))?
                        } else {
                            1
                        };
                        let transient = part_metadata
                            .checked_add(part_buffer_capacity)
                            .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?;
                        active_metadata = reserve_active_slots(
                            &mut active,
                            additional,
                            active_metadata,
                            transient,
                            previous_active_metadata,
                            self.limits.max_metadata_bytes,
                        )?;
                    } else {
                        let retained = previous_active_metadata
                            .checked_add(active_metadata)
                            .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?;
                        ensure_active_part_budget(
                            retained,
                            part_metadata,
                            part_buffer_capacity,
                            self.limits.max_metadata_bytes,
                        )?;
                    }
                    let catalog_digest = CatalogDigest::from_catalog(&catalog);
                    Arc::make_mut(&mut active).push(ActivePart {
                        segment_id,
                        part: part_ref,
                        catalog,
                        catalog_digest,
                    });
                    active_metadata = active_metadata
                        .checked_add(part_metadata)
                        .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?;
                    if previous_active_metadata
                        .checked_add(active_metadata)
                        .is_none_or(|bytes| bytes > self.limits.max_metadata_bytes)
                    {
                        return Err(metadata_limit_io(self.limits.max_metadata_bytes));
                    }
                    Ok(())
                },
            )?
        };
        if previous_active_metadata
            .checked_add(active_metadata)
            .is_none_or(|peak| peak > self.limits.max_metadata_bytes)
        {
            return Err(metadata_limit_io(self.limits.max_metadata_bytes));
        }
        if u64::try_from(scanned_valid_len).unwrap_or(u64::MAX) != plan.scan_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} contains torn or damaged version-1 frames",
                    journal_path.display()
                ),
            ));
        }
        Ok(JournalScan {
            active,
            valid_len: plan.valid_len,
            committed_reset: plan.committed_reset,
            metadata_bytes: previous_active_metadata
                .checked_add(active_metadata)
                .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?,
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one bounded merge accounts retained metadata while classifying each stable segment"
    )]
    pub(super) fn complete_scan_cached_with_warnings(
        &self,
        journal: JournalScan,
        previous_finished: &[FinalUnit],
        initial_warnings: &[StoreWarning],
        validation: FinishedValidation,
    ) -> io::Result<LocalScan> {
        let layout = self.root.scan(self.limits).map_err(layout_io)?;
        let segment_count = layout.segments.len();
        ensure_scan_metadata_budget(
            layout.metadata_bytes,
            journal.metadata_bytes,
            previous_finished.len(),
            segment_count,
            0,
            self.limits.max_metadata_bytes,
        )?;
        let mut retained_during_build = accounted_scan_metadata_bytes(
            layout.metadata_bytes,
            journal.metadata_bytes,
            previous_finished.len(),
            segment_count,
            0,
        )
        .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?;

        let mut finished = Vec::with_capacity(segment_count);
        let mut warnings = Vec::new();
        for warning in initial_warnings {
            push_warning_bounded(
                &mut warnings,
                *warning,
                retained_during_build,
                self.limits.max_metadata_bytes,
            )?;
        }
        for foreign in &layout.foreign_entries {
            let diagnostic = foreign.diagnostic();
            push_warning_bounded(
                &mut warnings,
                StoreWarning {
                    affected: StoreObject::Foreign(diagnostic.path),
                    reason: StoreWarningReason::ForeignEntry(diagnostic.reason),
                    identity: Some(diagnostic.path.file),
                    failure: None,
                },
                retained_during_build,
                self.limits.max_metadata_bytes,
            )?;
        }
        let mut previous_at = 0_usize;
        let mut open_day = None;
        for artifact in layout.segments {
            advance_previous(previous_finished, &mut previous_at, artifact.address);
            if let Some(previous) = previous_finished.get(previous_at).filter(|previous| {
                (previous.address, previous.identity) == (artifact.address, artifact.zms_identity)
            }) {
                finished.push(previous.clone());
                continue;
            }

            if open_day
                .as_ref()
                .is_none_or(|(day, _directory)| *day != artifact.address.day)
            {
                open_day = Some((
                    artifact.address.day,
                    self.root
                        .day_directory(artifact.address.day)
                        .map_err(layout_io)?,
                ));
            }
            let day = open_day
                .as_ref()
                .map(|(_day, directory)| directory)
                .ok_or_else(|| io::Error::other("selected segment day is not open"))?;
            let file = match Self::open_pinned_zms_in(day, artifact.address, artifact.zms_identity)?
            {
                ZmsOpen::Open(file) => file,
                ZmsOpen::Invalid(failure) => {
                    push_warning_bounded(
                        &mut warnings,
                        invalid_zms_warning(
                            artifact.address,
                            artifact.zms_identity,
                            InvalidZmsReason::Io,
                            Some(failure),
                        ),
                        retained_during_build,
                        self.limits.max_metadata_bytes,
                    )?;
                    continue;
                }
            };
            let validation_retained =
                retained_metadata_with_warnings(retained_during_build, warnings.capacity())
                    .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?;
            let summary = read_zms_summary(
                &file,
                validation_retained,
                self.limits.max_metadata_bytes,
                validation,
            );
            match classify_zms_validation(&file, artifact.zms_identity, artifact.address, summary)?
            {
                Ok(summary) => {
                    ensure_retained_metadata(
                        retained_during_build,
                        summary_allocation_bytes(),
                        warnings.capacity(),
                        self.limits.max_metadata_bytes,
                    )?;
                    retained_during_build = retained_during_build
                        .checked_add(summary_allocation_bytes())
                        .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?;
                    finished.push(FinalUnit {
                        address: artifact.address,
                        identity: artifact.zms_identity,
                        summary: Arc::new(summary),
                    });
                }
                Err(ZmsInvalid { reason, failure }) => push_warning_bounded(
                    &mut warnings,
                    invalid_zms_warning(artifact.address, artifact.zms_identity, reason, failure),
                    retained_during_build,
                    self.limits.max_metadata_bytes,
                )?,
            }
        }

        let metadata_bytes = accounted_scan_metadata_bytes(
            0,
            journal.metadata_bytes,
            0,
            finished.capacity(),
            finished.len(),
        )
        .ok_or_else(|| metadata_limit_io(self.limits.max_metadata_bytes))?;
        ensure_retained_metadata(
            metadata_bytes,
            0,
            warnings.capacity(),
            self.limits.max_metadata_bytes,
        )?;

        Ok(LocalScan {
            finished: Arc::new(finished),
            active: journal.active,
            warnings,
            valid_len: journal.valid_len,
            committed_reset: journal.committed_reset,
            metadata_bytes,
        })
    }

    pub(super) fn active_journal_warning(&self, error: &io::Error) -> StoreWarning {
        let source = active_journal_source(error);
        let reason = if matches!(
            source.kind(),
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
        ) {
            ActiveJournalWarningReason::Corrupt
        } else {
            ActiveJournalWarningReason::Io
        };
        let identity = self
            .root
            .open_active_journal()
            .ok()
            .flatten()
            .and_then(|file| FileIdentity::from_file(&file).ok());
        StoreWarning {
            affected: StoreObject::ActiveJournal,
            reason: StoreWarningReason::ActiveJournal(reason),
            identity,
            failure: Some(StoreIoFailure::from_error(StoreIoOperation::Read, source)),
        }
    }
}
