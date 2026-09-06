use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use rustix::fs::RenameFlags;
use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags};

use crate::{LayoutError, LayoutLimits, OwnerKind, SegmentAddress, UtcDay};

/// Root-level active segment journal.
mod discovery;
mod entry;
mod fsops;
mod index;
mod names;
mod scan;
mod writer;

pub use entry::{
    EntryDiagnostic, EntryFileType, EntryScope, FileKind, FilesystemUsage, ForeignEntry,
    ForeignEntryReason, LayoutSnapshot, PathIdentity, SegmentArtifacts, SegmentRemoval,
    TemporaryKind, TemporaryObject,
};
pub use index::{IdxTemp, IndexOwner};
pub use writer::{WriterLease, WriterOwner, ZmsTemp};

use fsops::{
    create_regular_at, ensure_directory_at, errno_to_io, errno_to_layout, link_open_file,
    open_directory_at, open_or_create_regular, open_regular_at, remove_empty_directory_at,
    remove_regular_if_identity, remove_verified_regular, rename_noreplace, stat_no_follow,
    sync_source_directory, temporary_name, unlink_named_if_identity, unlink_regular_capturing_size,
    verify_named_identity,
};
use names::{hash_name, writer_lock_is_poisoned};

/// The raw journal the collector appends to.
pub const ACTIVE_JOURNAL_NAME: &str = "active.wal";
/// Permanent process-ownership lock for the collector.
pub const WRITER_OWNER_LOCK_NAME: &str = ".kronika-writer.owner.lock";
/// Where a journal the writer could not read is moved so collection can
/// continue. At most one exists at a time.
pub const DAMAGED_JOURNAL_NAME: &str = "active.wal.damaged";
/// Permanent process-ownership lock for index publication and GC.
pub const INDEX_OWNER_LOCK_NAME: &str = ".kronika-index.owner.lock";
/// Where the collector keeps the offset each followed log file was read to.
pub const LOG_OFFSETS_NAME: &str = "log.offsets";
/// The temporary the offsets file is renamed from.
pub const LOG_OFFSETS_TEMP_NAME: &str = "log.tmp";
const ENTRY_METADATA_BYTES: usize = 128;
const SCAN_RACE_ATTEMPTS: usize = 4;
const WRITER_LOCK_HANDOFF_TIMEOUT: Duration = Duration::from_millis(100);
const DIRECTORY_MODE: Mode = Mode::RUSR
    .union(Mode::WUSR)
    .union(Mode::XUSR)
    .union(Mode::RGRP)
    .union(Mode::XGRP);
const DATA_FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::RGRP);
const LOCK_FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR);

/// A validated, already-open data-directory root.
///
/// Every component is opened relative to this descriptor without following a
/// symbolic link, so a path swapped underneath cannot redirect the writer.
#[derive(Debug, Clone)]
pub struct DataRoot {
    directory: Arc<File>,
    diagnostic_path: Arc<Path>,
}

/// An already-open UTC day directory for a bounded batch of segment opens.
#[derive(Debug)]
pub struct DayDirectory {
    day: UtcDay,
    directory: File,
}

impl DayDirectory {
    /// Opens one ZMS whose address belongs to this day.
    ///
    /// # Errors
    ///
    /// Returns an error if the address names another day or the final file is
    /// missing, symbolic, or not regular.
    pub fn open_zms(&self, address: SegmentAddress) -> Result<File, LayoutError> {
        if address.day != self.day {
            return Err(LayoutError::MisbucketedSegment {
                id: address.id,
                actual: self.day,
                expected: address.day,
            });
        }
        open_regular_at(&self.directory, &address.zms_name(), OFlags::RDONLY)
    }
}

impl DataRoot {
    /// Opens an existing data root without following a symbolic link.
    ///
    /// This only opens the root itself. Call [`scan`](Self::scan) before using
    /// its contents.
    ///
    /// # Errors
    ///
    /// Returns a structural or filesystem error when the root cannot be opened
    /// as a real directory.
    pub fn open(path: &Path) -> Result<Self, LayoutError> {
        let directory = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|error| {
            if error == rustix::io::Errno::LOOP {
                LayoutError::SymlinkNotAllowed {
                    name: path.display().to_string(),
                }
            } else {
                LayoutError::Io(errno_to_io(error))
            }
        })?;
        Ok(Self {
            directory: Arc::new(directory),
            diagnostic_path: Arc::from(path),
        })
    }

    /// Returns the configured root path for diagnostics only.
    ///
    /// Filesystem access should use this type's descriptor-relative methods.
    #[must_use]
    pub fn diagnostic_path(&self) -> &Path {
        &self.diagnostic_path
    }

    /// Builds a path for logs, error reports, and test assertions only.
    ///
    /// The returned path is not a capability and production I/O must use
    /// [`open_zms`](Self::open_zms), [`open_idx`](Self::open_idx), or an owner
    /// token.
    #[must_use]
    pub fn diagnostic_file_path(&self, address: SegmentAddress, kind: FileKind) -> PathBuf {
        let name = match kind {
            FileKind::Zms => address.zms_name(),
            FileKind::Idx => address.idx_name(),
        };
        self.diagnostic_path
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component())
            .join(name)
    }

    /// Performs a tolerant, closed-grammar, three-level, bounded traversal.
    ///
    /// Entries outside the owned-store grammar are classified in
    /// [`LayoutSnapshot::foreign_entries`] and never followed or traversed.
    /// Valid entries remain in the returned inventory. Resource exhaustion and
    /// failures while reading the verified tree still fail the whole scan.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError`] for exhausted limits or filesystem failures
    /// that prevent bounded traversal of the verified tree.
    pub fn scan(&self, limits: LayoutLimits) -> Result<LayoutSnapshot, LayoutError> {
        let limits = limits.validate()?;
        for attempt in 0..SCAN_RACE_ATTEMPTS {
            // Fresh per attempt: a retried scan revisits the same entries, and
            // carrying the count over would fail valid trees near the limit.
            let mut visited_entries = 0_usize;
            match self.scan_once(limits, &mut visited_entries) {
                Err(LayoutError::Io(error))
                    if error.kind() == io::ErrorKind::NotFound
                        && attempt + 1 < SCAN_RACE_ATTEMPTS =>
                {
                    std::thread::yield_now();
                }
                result => return result,
            }
        }
        unreachable!("the bounded scan loop always returns on its final attempt")
    }

    /// Opens a verified ZMS relative to the root descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error if a calendar component or final file is missing,
    /// replaced by a symbolic link, or has the wrong type.
    pub fn open_zms(&self, address: SegmentAddress) -> Result<File, LayoutError> {
        self.day_directory(address.day)?.open_zms(address)
    }

    /// Opens one UTC day directory for a bounded batch of segment reads.
    ///
    /// # Errors
    ///
    /// Returns an error if a calendar component is missing, symbolic, or not
    /// a directory.
    pub fn day_directory(&self, day: UtcDay) -> Result<DayDirectory, LayoutError> {
        Ok(DayDirectory {
            day,
            directory: self.open_day(day)?,
        })
    }

    /// Opens a verified IDX relative to the root descriptor when it exists.
    ///
    /// # Errors
    ///
    /// Returns an error if a component is unsafe or the existing final object
    /// is not a regular file.
    pub fn open_idx(&self, address: SegmentAddress) -> Result<Option<File>, LayoutError> {
        let day = self.open_day(address.day)?;
        let name = address.idx_name();
        match open_regular_at(&day, &name, OFlags::RDONLY) {
            Ok(file) => Ok(Some(file)),
            Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Opens a temporary object returned by [`scan`](Self::scan).
    ///
    /// The verified leaf name and its typed day address are used directly;
    /// callers cannot substitute an arbitrary relative path.
    ///
    /// # Errors
    ///
    /// Returns an error if the object disappeared, changed type, or any
    /// calendar component became unsafe.
    pub fn open_temporary(&self, temporary: &TemporaryObject) -> Result<File, LayoutError> {
        let day = self.open_day(temporary.address.day)?;
        open_regular_at(&day, temporary.file_name(), OFlags::RDONLY)
    }

    /// Opens the active journal for reading when it exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the existing entry is unsafe or unreadable.
    pub fn open_active_journal(&self) -> Result<Option<File>, LayoutError> {
        match open_regular_at(&self.directory, ACTIVE_JOURNAL_NAME, OFlags::RDONLY) {
            Ok(file) => Ok(Some(file)),
            Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reads the byte occupancy of the partition backing this root.
    ///
    /// The measurement is a single `statvfs` of the already-open root
    /// descriptor, so no path is re-resolved and foreign data on a shared
    /// partition is included in [`FilesystemUsage::used_bytes`].
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the descriptor cannot be queried.
    pub fn filesystem_usage(&self) -> Result<FilesystemUsage, LayoutError> {
        let stat = rustix::fs::fstatvfs(&*self.directory).map_err(errno_to_layout)?;
        let block_bytes = stat.f_frsize;
        let total_bytes = stat.f_blocks.saturating_mul(block_bytes);
        let free_bytes = stat.f_bfree.saturating_mul(block_bytes);
        Ok(FilesystemUsage {
            total_bytes,
            used_bytes: total_bytes.saturating_sub(free_bytes),
            available_bytes: stat.f_bavail.saturating_mul(block_bytes),
        })
    }

    /// Establishes lifetime-exclusive writer ownership after two strict scans.
    ///
    /// # Errors
    ///
    /// Returns a structural error, I/O error, or
    /// [`LayoutError::OwnerContended`].
    pub fn acquire_writer(&self, limits: LayoutLimits) -> Result<WriterOwner, LayoutError> {
        self.scan(limits)?;
        let root_lock = self.acquire_writer_root_lock()?;
        let lock = match self.acquire_writer_lock() {
            Ok(lock) => lock,
            Err(_error) if writer_lock_is_poisoned(self) => root_lock.try_clone()?,
            Err(error) => return Err(error),
        };
        self.scan(limits)?;
        Ok(WriterOwner {
            root: self.clone(),
            owner_lock: lock,
            _root_lock: root_lock,
        })
    }

    /// Establishes lifetime-exclusive index ownership after two strict scans.
    ///
    /// # Errors
    ///
    /// Returns a structural error, I/O error, or
    /// [`LayoutError::OwnerContended`].
    pub fn acquire_index(&self, limits: LayoutLimits) -> Result<IndexOwner, LayoutError> {
        self.scan(limits)?;
        let lock = self.acquire_lock(INDEX_OWNER_LOCK_NAME, OwnerKind::Index)?;
        self.scan(limits)?;
        Ok(IndexOwner {
            root: self.clone(),
            _lock: lock,
        })
    }

    fn acquire_lock(&self, name: &str, owner: OwnerKind) -> Result<File, LayoutError> {
        let (lock, _created) =
            open_or_create_regular(&self.directory, name, OFlags::RDWR, LOCK_FILE_MODE)?;
        rustix::fs::fchmod(&lock, LOCK_FILE_MODE)
            .map_err(errno_to_io)
            .map_err(LayoutError::Io)?;
        lock.sync_all()?;
        // A previous creator may have initialized the inode and then failed
        // to synchronize its root entry. EEXIST alone does not mean durable.
        self.directory.sync_all()?;
        match rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(lock),
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                Err(LayoutError::OwnerContended { owner })
            }
            Err(error) => Err(LayoutError::Io(errno_to_io(error))),
        }
    }

    fn acquire_writer_lock(&self) -> Result<File, LayoutError> {
        let started = Instant::now();
        loop {
            match self.acquire_lock(WRITER_OWNER_LOCK_NAME, OwnerKind::Writer) {
                Err(LayoutError::OwnerContended {
                    owner: OwnerKind::Writer,
                }) if started.elapsed() < WRITER_LOCK_HANDOFF_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                result => return result,
            }
        }
    }

    fn acquire_writer_root_lock(&self) -> Result<File, LayoutError> {
        let lock = rustix::fs::openat(
            &*self.directory,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(errno_to_io)
        .map_err(LayoutError::Io)?;
        let started = Instant::now();
        loop {
            match rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => return Ok(lock),
                Err(error)
                    if error == rustix::io::Errno::WOULDBLOCK
                        && started.elapsed() < WRITER_LOCK_HANDOFF_TIMEOUT =>
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                    return Err(LayoutError::OwnerContended {
                        owner: OwnerKind::Writer,
                    });
                }
                Err(error) => return Err(LayoutError::Io(errno_to_io(error))),
            }
        }
    }

    fn open_day(&self, day: UtcDay) -> Result<File, LayoutError> {
        let year = open_directory_at(&self.directory, &day.year_component())?;
        let month = open_directory_at(&year, &day.month_component())?;
        open_directory_at(&month, &day.day_component())
    }

    fn ensure_day(&self, day: UtcDay) -> Result<File, LayoutError> {
        let year = ensure_directory_at(&self.directory, &day.year_component())?;
        let month = ensure_directory_at(&year, &day.month_component())?;
        ensure_directory_at(&month, &day.day_component())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Filesystem identity used to pin an immutable segment between discovery and
/// opening its contents.
pub struct FileIdentity {
    /// Filesystem device number.
    pub device: u64,
    /// Inode number on the device.
    pub inode: u64,
    /// File length in bytes.
    pub len: u64,
    /// Modification time, whole seconds since the Unix epoch.
    pub mtime_seconds: i64,
    /// Nanosecond part of the modification time.
    pub mtime_nanoseconds: i64,
    /// Metadata-change time, whole seconds since the Unix epoch.
    pub ctime_seconds: i64,
    /// Nanosecond part of the metadata-change time.
    pub ctime_nanoseconds: i64,
}

impl FileIdentity {
    #[allow(
        clippy::useless_conversion,
        reason = "rustix Stat integer field types differ across supported Unix targets"
    )]
    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            device: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
            inode: u64::try_from(stat.st_ino).unwrap_or(u64::MAX),
            len: u64::try_from(stat.st_size).unwrap_or(u64::MAX),
            mtime_seconds: stat.st_mtime,
            mtime_nanoseconds: i64::try_from(stat.st_mtime_nsec).unwrap_or(i64::MAX),
            ctime_seconds: stat.st_ctime,
            ctime_nanoseconds: i64::try_from(stat.st_ctime_nsec).unwrap_or(i64::MAX),
        }
    }

    /// Reads the identity from an already-open file descriptor.
    ///
    /// # Errors
    ///
    /// Returns the underlying `fstat` error.
    pub fn from_file(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            len: metadata.len(),
            mtime_seconds: metadata.mtime(),
            mtime_nanoseconds: metadata.mtime_nsec(),
            ctime_seconds: metadata.ctime(),
            ctime_nanoseconds: metadata.ctime_nsec(),
        })
    }

    const fn same_named_object(self, other: Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.len == other.len
            && self.mtime_seconds == other.mtime_seconds
            && self.mtime_nanoseconds == other.mtime_nanoseconds
            && self.ctime_seconds == other.ctime_seconds
            && self.ctime_nanoseconds == other.ctime_nanoseconds
    }
}

#[cfg(test)]
mod tests;
