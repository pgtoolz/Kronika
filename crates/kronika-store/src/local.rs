//! Directory-backed storage implementation.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use kronika_format::JOURNAL_HEADER_LEN;
use kronika_layout::{DataRoot, DayDirectory, FileIdentity, LayoutError, LayoutLimits};

use crate::source::{
    ActivePart, ActiveSnapshot, FinalUnit, JournalScan, LocalScan, StoreError, StoreIoFailure,
    StoreIoOperation,
};

mod budget;
mod journal;
mod scan;
pub(crate) mod segment;
mod snapshot;

pub(crate) use budget::summary_allocation_bytes;
use budget::{
    layout_io, push_warning_bounded, require_file_identity, retained_metadata_with_warnings,
};
pub use journal::is_active_journal_scan_error;
use journal::{active_journal_io, degradable_active_journal_error, empty_journal_scan};
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
        self.complete_journal_scan(self.scan_journal(), FinishedValidation::Complete)
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
        self.complete_journal_scan(self.scan_journal(), FinishedValidation::Catalog)
    }

    fn complete_journal_scan(
        &self,
        journal: io::Result<JournalScan>,
        validation: FinishedValidation,
    ) -> io::Result<LocalScan> {
        match journal {
            Ok(journal) => self.complete_scan_cached_with_warnings(journal, &[], &[], validation),
            Err(error) if degradable_active_journal_error(&error) => {
                let warning = self.active_journal_warning(&error);
                let journal = empty_journal_scan(self.limits.max_metadata_bytes)?;
                self.complete_scan_cached_with_warnings(journal, &[], &[warning], validation)
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
        self.complete_journal_scan(
            self.scan_journal_from(last_valid_len, prev_active),
            FinishedValidation::Complete,
        )
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

    // Pin both the name and descriptor identity before trusting cached metadata.
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

#[cfg(test)]
#[path = "tests/local.rs"]
mod tests;
