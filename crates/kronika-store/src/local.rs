//! Directory-backed storage implementation.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use kronika_format::{JOURNAL_HEADER_LEN, MAX_PART_LEN, ReadAt};
use kronika_layout::{DataRoot, DayDirectory, FileIdentity, LayoutError, LayoutLimits};

use crate::source::{
    ActivePart, ActiveSnapshot, FinalUnit, JournalScan, LocalScan, StoreError, StoreIoFailure,
    StoreIoOperation,
};

mod budget;
mod journal;
mod scan;
pub(crate) mod segment;

pub(crate) use budget::summary_allocation_bytes;
use budget::{
    layout_io, push_warning_bounded, require_file_identity, retained_metadata_with_warnings,
};
pub use journal::is_active_journal_scan_error;
use journal::{
    active_journal_io, degradable_active_journal_error, empty_journal_scan,
    validate_active_part_reference, validate_active_snapshot,
};
pub use segment::read_catalog;
use segment::{
    FinishedValidation, ZmsOpen, classify_zms_validation, invalid_zms_warning, read_zms_summary,
    stale_finished_zms,
};

const ARC_ALLOCATION_OVERHEAD: usize = 2 * size_of::<usize>();
const ACTIVE_ARC_ALLOCATION_BYTES: usize = size_of::<Vec<ActivePart>>() + ARC_ALLOCATION_OVERHEAD;

#[cfg(test)]
std::thread_local! {
    static CATALOG_SUMMARY_READS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    pub(crate) static ACTIVE_JOURNAL_SCANS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

/// A storage directory containing finished `.zms` segments and an `active.wal`
/// journal.
#[derive(Debug, Clone)]
pub struct LocalDir {
    root: DataRoot,
    limits: LayoutLimits,
}

impl LocalDir {
    /// Open a local directory as a segment store.
    ///
    /// This opens only the root directory descriptor. The complete owned-tree
    /// grammar and segment contents are validated during
    /// [`scan`](Self::scan) or [`complete_scan`](Self::complete_scan).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if `root` is not a directory or cannot be accessed.
    pub fn open(root: &Path) -> io::Result<Self> {
        let root = DataRoot::open(root).map_err(layout_io)?;
        let limits = LayoutLimits::default();
        Ok(Self { root, limits })
    }

    pub(crate) const fn metadata_limit(&self) -> usize {
        self.limits.max_metadata_bytes
    }

    /// Scan the directory for finished segments and active journal parts.
    ///
    /// Every admitted `.zms` is completely validated. Invalid finished files and
    /// an unavailable or corrupt `active.wal` journal are excluded with
    /// typed warnings so valid final data remains queryable. The strict
    /// journal API remains available through [`scan_journal`](Self::scan_journal).
    ///
    /// # Errors
    ///
    /// Returns an I/O error only when bounded root traversal or a global
    /// resource limit prevents a safe result.
    pub fn scan(&self) -> io::Result<LocalScan> {
        match self.scan_journal() {
            Ok(journal) => self.complete_scan(journal),
            Err(error) if degradable_active_journal_error(&error) => {
                let warning = self.active_journal_warning(&error);
                let journal = empty_journal_scan(self.limits.max_metadata_bytes)?;
                self.complete_scan_cached_with_warnings(
                    journal,
                    &[],
                    &[warning],
                    FinishedValidation::Complete,
                )
            }
            Err(error) => Err(error),
        }
    }

    /// Scan the directory while reading only finished-segment catalogs.
    ///
    /// This is the discovery path for range readers. Section bodies remain
    /// unread until the caller selects a segment and invokes
    /// [`validate_finished`](Self::validate_finished).
    ///
    /// # Errors
    ///
    /// Returns an I/O error only when bounded root traversal or a global
    /// resource limit prevents a safe result.
    pub fn scan_catalogs(&self) -> io::Result<LocalScan> {
        match self.scan_journal() {
            Ok(journal) => self.complete_scan_cached_with_warnings(
                journal,
                &[],
                &[],
                FinishedValidation::Catalog,
            ),
            Err(error) if degradable_active_journal_error(&error) => {
                let warning = self.active_journal_warning(&error);
                let journal = empty_journal_scan(self.limits.max_metadata_bytes)?;
                self.complete_scan_cached_with_warnings(
                    journal,
                    &[],
                    &[warning],
                    FinishedValidation::Catalog,
                )
            }
            Err(error) => Err(error),
        }
    }

    /// Scan only finished-segment catalogs for the immutable resource adapter.
    ///
    /// The active journal is neither opened nor scanned. The empty journal
    /// value supplies the existing bounded finished-tree traversal with no
    /// live state.
    pub(crate) fn scan_finished_catalogs(&self) -> io::Result<LocalScan> {
        let journal = empty_journal_scan(self.limits.max_metadata_bytes)?;
        self.complete_scan_cached_with_warnings(journal, &[], &[], FinishedValidation::Catalog)
    }

    /// Scan only `active.wal`.
    ///
    /// The returned journal view is complete and can be captured inside a
    /// short file-identity handshake. Finished-tree traversal is deliberately
    /// deferred to [`complete_scan`](Self::complete_scan).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the journal is malformed, changes during the
    /// scan, or cannot be read.
    pub fn scan_journal(&self) -> io::Result<JournalScan> {
        #[cfg(test)]
        ACTIVE_JOURNAL_SCANS.with(|scans| scans.set(scans.get().saturating_add(1)));
        let journal_path = self.root.diagnostic_path().join("active.wal");
        let journal = self
            .root
            .open_active_journal()
            .map_err(layout_io)
            .map_err(active_journal_io)?;
        journal.map_or_else(
            || empty_journal_scan(self.limits.max_metadata_bytes),
            |file| {
                self.scan_journal_reader_from(
                    &file,
                    JOURNAL_HEADER_LEN as u64,
                    Arc::new(Vec::new()),
                    &journal_path,
                )
                .map_err(active_journal_io)
            },
        )
    }

    /// Incrementally re-scan the store from a known journal offset.
    ///
    /// `last_valid_len` is the end of the last valid frame seen by the previous
    /// scan; `prev_active` are the active parts that scan already validated.
    /// The journal is stat-gated:
    ///
    /// - size `== last_valid_len`: unchanged; `prev_active` is kept as is and the
    ///   journal body is not re-read.
    /// - size `> last_valid_len`: only `[last_valid_len, size)` is scanned and the
    ///   new parts are appended to `prev_active`.
    /// - size `< last_valid_len` or the file is gone: a reset;
    ///   `prev_active` is dropped and the journal is scanned from its v1 header.
    ///
    /// Finished `.zms` files are always re-listed. A journal that cannot provide
    /// an exact incremental view degrades to an empty live generation plus a
    /// typed warning; callers needing retry/fail semantics can invoke
    /// [`scan_journal_from`](Self::scan_journal_from) directly.
    ///
    /// # Errors
    ///
    /// Returns an I/O error only when bounded root traversal or a global
    /// resource limit prevents a safe result.
    pub fn scan_from<A>(&self, last_valid_len: u64, prev_active: A) -> io::Result<LocalScan>
    where
        A: Into<Arc<Vec<ActivePart>>>,
    {
        match self.scan_journal_from(last_valid_len, prev_active) {
            Ok(journal) => self.complete_scan(journal),
            Err(error) if degradable_active_journal_error(&error) => {
                let warning = self.active_journal_warning(&error);
                let journal = empty_journal_scan(self.limits.max_metadata_bytes)?;
                self.complete_scan_cached_with_warnings(
                    journal,
                    &[],
                    &[warning],
                    FinishedValidation::Complete,
                )
            }
            Err(error) => Err(error),
        }
    }

    /// Incrementally scan only `active.wal`.
    ///
    /// This has the same journal resume semantics as [`scan_from`](Self::scan_from)
    /// without traversing the finished tree.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the journal is malformed, changes during the
    /// scan, or cannot be read.
    pub fn scan_journal_from<A>(
        &self,
        last_valid_len: u64,
        prev_active: A,
    ) -> io::Result<JournalScan>
    where
        A: Into<Arc<Vec<ActivePart>>>,
    {
        let prev_active = prev_active.into();
        let journal_path = self.root.diagnostic_path().join("active.wal");
        let journal = self
            .root
            .open_active_journal()
            .map_err(layout_io)
            .map_err(active_journal_io)?;
        journal.map_or_else(
            || empty_journal_scan(self.limits.max_metadata_bytes),
            |file| {
                self.scan_journal_reader_from(&file, last_valid_len, prev_active, &journal_path)
                    .map_err(active_journal_io)
            },
        )
    }

    /// Complete a captured journal scan with one strict finished-tree traversal.
    ///
    /// The journal is not opened again. This keeps potentially long finished
    /// traversal outside a journal identity handshake while retaining the
    /// exact journal state already validated by [`scan_journal`](Self::scan_journal).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the owned tree or a finished segment is invalid, or if
    /// directory plus active-result metadata exceed the shared layout budget.
    pub fn complete_scan(&self, journal: JournalScan) -> io::Result<LocalScan> {
        self.complete_scan_cached(journal, &[])
    }

    /// Complete a journal scan while reusing unchanged finished catalog summaries.
    ///
    /// `previous_finished` must be the sorted finished result of an earlier scan of
    /// this store. A summary is reused only when both its address and complete
    /// filesystem identity match the current strict layout result. All other
    /// ZMS catalogs are reopened and validated.
    ///
    /// The merge is linear in the current and previous finished counts and does
    /// not build an auxiliary map.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the tree or a ZMS is invalid or changes while
    /// being scanned, or if retained scan metadata would exceed the shared
    /// layout budget. No partial scan is returned.
    pub fn complete_scan_cached(
        &self,
        journal: JournalScan,
        previous_finished: &[FinalUnit],
    ) -> io::Result<LocalScan> {
        self.complete_scan_cached_with_warnings(
            journal,
            previous_finished,
            &[],
            FinishedValidation::Complete,
        )
    }

    /// Verify every section body of one catalog-discovered finished segment.
    ///
    /// Returns `true` for a valid segment. A stable invalid file returns
    /// `false` and appends its normal bounded warning to `scan`. A file that
    /// changes under validation returns a retryable I/O error.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the discovered file changed or a global
    /// resource limit prevents validation.
    pub fn validate_finished(&self, scan: &mut LocalScan, unit: &FinalUnit) -> io::Result<bool> {
        let retained_metadata = scan.metadata_bytes;
        let validation_retained =
            retained_metadata_with_warnings(retained_metadata, scan.warnings.capacity())
                .ok_or_else(|| budget::metadata_limit_io(self.limits.max_metadata_bytes))?;
        let file = match self.open_pinned_zms(unit.address, unit.identity)? {
            ZmsOpen::Open(file) => file,
            ZmsOpen::Invalid(failure) => {
                push_warning_bounded(
                    &mut scan.warnings,
                    invalid_zms_warning(
                        unit.address,
                        unit.identity,
                        crate::InvalidZmsReason::Io,
                        Some(failure),
                    ),
                    retained_metadata,
                    self.limits.max_metadata_bytes,
                )?;
                return Ok(false);
            }
        };
        let validation = read_zms_summary(
            &file,
            validation_retained,
            self.limits.max_metadata_bytes,
            FinishedValidation::Complete,
        );
        match classify_zms_validation(&file, unit.identity, unit.address, validation)? {
            Ok(summary) if summary == *unit.summary => Ok(true),
            Ok(_changed) => Err(stale_finished_zms(
                unit.address,
                "while its sections were validated",
            )),
            Err(invalid) => {
                push_warning_bounded(
                    &mut scan.warnings,
                    invalid_zms_warning(
                        unit.address,
                        unit.identity,
                        invalid.reason,
                        invalid.failure,
                    ),
                    retained_metadata,
                    self.limits.max_metadata_bytes,
                )?;
                Ok(false)
            }
        }
    }

    /// Open a finished segment file for raw byte access.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the file cannot be opened or no longer has the
    /// filesystem identity pinned by the scan.
    pub fn open_finished(&self, u: &FinalUnit) -> io::Result<File> {
        let file = self.root.open_zms(u.address).map_err(layout_io)?;
        self.validate_finished_file(&file, u)?;
        Ok(file)
    }

    /// Validate an already-open finished segment against its pinned scan identity.
    ///
    /// Readers call this again after parsing the catalog so an in-place change
    /// during positional reads becomes a retryable stale-snapshot error without
    /// reopening the path.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Interrupted`] when the descriptor no longer
    /// names the exact file captured by `u`.
    #[expect(
        clippy::unused_self,
        reason = "identity validation belongs to the LocalDir finished-open contract"
    )]
    pub fn validate_finished_file(&self, file: &File, u: &FinalUnit) -> io::Result<()> {
        require_file_identity(file, u.identity, u.address, "while it was open")
    }

    /// Opens the root-level active journal for snapshot identity and prefix
    /// checks.
    ///
    /// # Errors
    ///
    /// Returns an error when the existing journal is unsafe or unreadable.
    pub fn open_active(&self) -> io::Result<Option<File>> {
        self.root.open_active_journal().map_err(layout_io)
    }

    /// Open the exact active prefix described by `scan`.
    ///
    /// Later appends to the same generation are allowed but remain outside the
    /// snapshot. A reset or generation change makes subsequent reads stale.
    ///
    /// # Errors
    ///
    /// Returns an error when the captured generation is no longer readable.
    pub fn open_active_snapshot(
        &self,
        scan: &LocalScan,
    ) -> Result<Option<ActiveSnapshot>, StoreError> {
        let Some(first) = scan.active.first() else {
            return Ok(None);
        };
        let file = self
            .root
            .open_active_journal()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "active.wal is absent"))?;
        validate_active_snapshot(&file, first.segment_id, scan.valid_len)?;
        Ok(Some(ActiveSnapshot {
            file: Arc::new(file),
            active: Arc::clone(&scan.active),
            part_count: scan.active.len(),
            valid_len: scan.valid_len,
            segment_id: first.segment_id,
        }))
    }

    /// Read the bytes of one active part from the journal.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ActivePartTooLarge`] if the cached part reference
    /// exceeds the active part cap, or [`StoreError::Io`] if the journal file
    /// cannot be opened or the part bytes cannot be read.
    pub fn read_active_part(&self, p: &ActivePart) -> Result<Vec<u8>, StoreError> {
        let part_len = u64::try_from(p.part.len).unwrap_or(u64::MAX);
        if part_len > MAX_PART_LEN {
            return Err(StoreError::ActivePartTooLarge {
                len: p.part.len,
                max: MAX_PART_LEN,
            });
        }
        let file = self
            .root
            .open_active_journal()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "active.wal is absent"))?;
        validate_active_part_reference(&file, p)?;
        let mut buf = vec![0_u8; p.part.len];
        file.read_exact_at(&mut buf, p.part.offset as u64)?;
        Ok(buf)
    }

    /// Scan an already-open `active.wal` journal from `start_at` into active
    /// parts, damage regions, and the resumable `valid_len`.
    ///
    /// Newly validated parts are appended to `prev_active`; pass an empty vector
    /// for a full scan. Frame offsets from the scan are absolute, so a caller
    /// resuming at `start_at > 0` gets parts that reference their true journal
    /// position.
    ///
    /// A concurrent completed-segment publication followed by `Journal::reset`
    /// can shrink the journal during scanning. Such an
    /// `UnexpectedEof`/`NotFound` becomes a retryable `Interrupted` error; no
    /// partial active or finished set is returned. Other I/O errors propagate.
    fn open_pinned_zms(
        &self,
        address: kronika_layout::SegmentAddress,
        expected: FileIdentity,
    ) -> io::Result<ZmsOpen> {
        Self::classify_pinned_zms(self.root.open_zms(address), address, expected)
    }

    fn open_pinned_zms_in(
        day: &DayDirectory,
        address: kronika_layout::SegmentAddress,
        expected: FileIdentity,
    ) -> io::Result<ZmsOpen> {
        Self::classify_pinned_zms(day.open_zms(address), address, expected)
    }

    fn classify_pinned_zms(
        opened: Result<File, LayoutError>,
        address: kronika_layout::SegmentAddress,
        expected: FileIdentity,
    ) -> io::Result<ZmsOpen> {
        let file = match opened {
            Ok(file) => file,
            Err(LayoutError::Io(source))
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof
                ) =>
            {
                return Err(stale_finished_zms(address, "before it could be opened"));
            }
            Err(LayoutError::Io(source)) if source.kind() == io::ErrorKind::OutOfMemory => {
                return Err(source);
            }
            Err(LayoutError::Io(source)) => {
                return Ok(ZmsOpen::Invalid(StoreIoFailure::from_error(
                    StoreIoOperation::Open,
                    &source,
                )));
            }
            Err(
                LayoutError::SymlinkNotAllowed { .. }
                | LayoutError::UnexpectedCalendarEntryType { .. }
                | LayoutError::UnexpectedLeafEntryType { .. },
            ) => {
                return Err(stale_finished_zms(address, "before it could be opened"));
            }
            Err(other) => return Err(layout_io(other)),
        };
        match FileIdentity::from_file(&file) {
            Ok(actual) if actual == expected => Ok(ZmsOpen::Open(file)),
            Ok(_changed) => Err(stale_finished_zms(address, "after opening it")),
            Err(source)
                if matches!(
                    source.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof
                ) =>
            {
                Err(stale_finished_zms(address, "after opening it"))
            }
            Err(source) if source.kind() == io::ErrorKind::OutOfMemory => Err(source),
            Err(source) => Ok(ZmsOpen::Invalid(StoreIoFailure::from_error(
                StoreIoOperation::Metadata,
                &source,
            ))),
        }
    }
}

impl ActiveSnapshot {
    /// Identity of the logical segment captured from the journal header.
    #[must_use]
    pub const fn segment_id(&self) -> kronika_layout::SegmentId {
        self.segment_id
    }

    /// Valid journal parts in captured order.
    #[must_use]
    pub fn parts(&self) -> &[ActivePart] {
        &self.active[..self.part_count]
    }

    /// Return the same journal generation truncated at an earlier committed
    /// frame boundary.
    ///
    /// This lets a paged active reader keep the exact prefix named by its
    /// cursor even after later frames have been appended.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OutOfBounds`] when `position` is later than this
    /// capture or does not end at the journal header or a complete frame.
    pub fn at_position(&self, position: u64) -> Result<Self, StoreError> {
        if position > self.valid_len {
            return Err(StoreError::OutOfBounds);
        }
        let header_end = JOURNAL_HEADER_LEN as u64;
        let mut boundary = header_end;
        let mut count = 0_usize;
        for part in self.parts() {
            let offset =
                u64::try_from(part.part.offset).map_err(|_overflow| StoreError::OutOfBounds)?;
            let len = u64::try_from(part.part.len).map_err(|_overflow| StoreError::OutOfBounds)?;
            let end = offset.checked_add(len).ok_or(StoreError::OutOfBounds)?;
            if end > position {
                break;
            }
            boundary = end;
            count = count.saturating_add(1);
        }
        if position != boundary {
            return Err(StoreError::OutOfBounds);
        }
        Ok(Self {
            file: Arc::clone(&self.file),
            active: Arc::clone(&self.active),
            part_count: count,
            valid_len: position,
            segment_id: self.segment_id,
        })
    }

    /// Read a byte range relative to one captured part.
    ///
    /// The range is bounded by both the part and the captured journal prefix.
    /// Later appends are ignored. A reset or generation change makes the read
    /// stale instead of mixing generations.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OutOfBounds`] for a range outside the captured
    /// part, or an I/O error when the journal generation changed.
    pub fn read_part_range(
        &self,
        part_index: usize,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, StoreError> {
        let part = self
            .parts()
            .get(part_index)
            .ok_or(StoreError::OutOfBounds)?;
        if part.segment_id != self.segment_id {
            return Err(StoreError::OutOfBounds);
        }
        let part_len = u64::try_from(part.part.len).unwrap_or(u64::MAX);
        if part_len > MAX_PART_LEN {
            return Err(StoreError::ActivePartTooLarge {
                len: part.part.len,
                max: MAX_PART_LEN,
            });
        }
        let relative_end = offset.checked_add(len).ok_or(StoreError::OutOfBounds)?;
        if relative_end > part_len {
            return Err(StoreError::OutOfBounds);
        }
        let part_offset = u64::try_from(part.part.offset).map_err(|_overflow| {
            StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "active part offset does not fit u64",
            ))
        })?;
        let absolute_offset = part_offset
            .checked_add(offset)
            .ok_or(StoreError::OutOfBounds)?;
        let absolute_end = absolute_offset
            .checked_add(len)
            .ok_or(StoreError::OutOfBounds)?;
        if absolute_end > self.valid_len {
            return Err(StoreError::OutOfBounds);
        }
        let buffer_len = usize::try_from(len).map_err(|_overflow| {
            StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "active range length does not fit usize",
            ))
        })?;
        validate_active_snapshot(self.file.as_ref(), self.segment_id, self.valid_len)?;
        let mut buffer = vec![0_u8; buffer_len];
        self.file.read_exact_at(&mut buffer, absolute_offset)?;
        validate_active_snapshot(self.file.as_ref(), self.segment_id, self.valid_len)?;
        Ok(buffer)
    }
}

#[cfg(test)]
mod tests;
