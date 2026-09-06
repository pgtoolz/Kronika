//! Exclusive writer ownership and segment publication.

use super::index::prune_empty_calendar;
use super::*;

/// Lifetime token for the only collector allowed to mutate one data root.
#[derive(Debug)]
pub struct WriterOwner {
    pub(super) root: DataRoot,
    pub(super) owner_lock: File,
    pub(super) _root_lock: File,
}

/// Opaque clone of the writer lock held by a long-lived mutation handle.
///
/// The operating-system lock is released only after both the owner and every
/// lease cloned from it have been dropped.
#[derive(Debug)]
pub struct WriterLease {
    _lock: File,
}

impl WriterOwner {
    /// Returns read-only access to the same verified root.
    #[must_use]
    pub const fn root(&self) -> &DataRoot {
        &self.root
    }

    /// Clones the operating-system writer-lock lease.
    ///
    /// Long-lived mutation handles retain this value so dropping the original
    /// [`WriterOwner`] cannot release ownership while they are still usable.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the lock descriptor cannot be duplicated.
    pub fn try_clone_lease(&self) -> Result<WriterLease, LayoutError> {
        Ok(WriterLease {
            _lock: self.owner_lock.try_clone()?,
        })
    }

    /// Moves a journal the writer cannot read out of the way.
    ///
    /// The file keeps its bytes under [`DAMAGED_JOURNAL_NAME`] so an operator
    /// can look at it, and the caller can create a fresh `active.wal` and keep
    /// collecting. At most one is kept: a second damaged journal replaces the
    /// first, which bounds what this costs on disk without anything having to
    /// scan for them.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the rename or the directory sync fails.
    pub fn set_aside_damaged_journal(&self) -> Result<PathBuf, LayoutError> {
        let directory = &self.root.directory;
        let damaged = CString::new(DAMAGED_JOURNAL_NAME).map_err(|_interior_nul| {
            LayoutError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the damaged-journal name contains an interior NUL",
            ))
        })?;
        // A previous damaged journal is replaced, so at most one exists.
        if let Err(error) = rustix::fs::unlinkat(directory, damaged.as_c_str(), AtFlags::empty())
            && error != rustix::io::Errno::NOENT
        {
            return Err(LayoutError::Io(errno_to_io(error)));
        }
        let active = CString::new(ACTIVE_JOURNAL_NAME).map_err(|_interior_nul| {
            LayoutError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the active-journal name contains an interior NUL",
            ))
        })?;
        rename_noreplace(directory, active.as_c_str(), directory, damaged.as_c_str())
            .map_err(LayoutError::Io)?;
        sync_source_directory(directory).map_err(LayoutError::Io)?;
        Ok(self.root.diagnostic_path().join(DAMAGED_JOURNAL_NAME))
    }

    /// Opens or creates the root-level journal without following links.
    ///
    /// The boolean result is `true` only when this operation created the
    /// directory entry. A creator must write and synchronize the valid initial
    /// header before calling [`sync_root`](Self::sync_root).
    ///
    /// # Errors
    ///
    /// Returns an error when the journal entry is unsafe or inaccessible.
    pub fn open_or_create_journal(&self) -> Result<(File, bool), LayoutError> {
        open_or_create_regular(
            &self.root.directory,
            ACTIVE_JOURNAL_NAME,
            OFlags::RDWR,
            DATA_FILE_MODE,
        )
    }

    /// Synchronizes the data-root directory after a new control entry has been
    /// fully initialized.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the directory sync fails.
    pub fn sync_root(&self) -> Result<(), LayoutError> {
        self.root.directory.sync_all().map_err(LayoutError::Io)
    }

    /// Creates a new process-unique ZMS temporary in the segment's UTC day.
    ///
    /// # Errors
    ///
    /// Returns an error when the day cannot be safely created or the temporary
    /// cannot be opened exclusively.
    pub fn create_zms_temp(&self, address: SegmentAddress) -> Result<ZmsTemp<'_>, LayoutError> {
        let day = self.root.ensure_day(address.day)?;
        let temp_name = temporary_name(address, TemporaryKind::Zms);
        let file = create_regular_at(&day, &temp_name, OFlags::RDWR, DATA_FILE_MODE)?;
        Ok(ZmsTemp {
            _owner: self,
            day,
            file,
            prepared_identity: Cell::new(None),
            temp_name,
            final_name: address.zms_name(),
            address,
            published: false,
        })
    }

    /// Removes a verified stale writer temporary and synchronizes its day.
    ///
    /// # Errors
    ///
    /// Returns an error for the wrong owner kind, a changed file type, or I/O.
    pub fn remove_temporary(&self, temporary: &TemporaryObject) -> Result<(), LayoutError> {
        if temporary.kind != TemporaryKind::Zms {
            return Err(LayoutError::UnexpectedLeafEntry {
                name: temporary.file_name.clone(),
            });
        }
        remove_verified_regular(&self.root, temporary.address, temporary.file_name())
    }

    /// Removes a finished segment (its ZMS and sibling IDX) and prunes the day.
    ///
    /// Direct unlink is safe: live readers keep their own descriptors, the
    /// index owner revalidates the input file before publishing an IDX, and
    /// its GC rechecks device/inode, so no delayed two-step deletion is needed.
    /// A part of the pair that already vanished frees zero bytes instead of
    /// failing. The writer owns the root, so empty calendar ancestors are
    /// pruned in place.
    ///
    /// # Errors
    ///
    /// Returns an error if a present entry is an unsafe type or a filesystem
    /// operation other than a missing-file race fails.
    pub fn remove_finished_segment(
        &self,
        address: SegmentAddress,
    ) -> Result<SegmentRemoval, LayoutError> {
        let day = match self.root.open_day(address.day) {
            Ok(day) => day,
            Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SegmentRemoval::default());
            }
            Err(error) => return Err(error),
        };
        let zms_bytes = unlink_regular_capturing_size(&day, &address.zms_name())?;
        let idx_bytes = unlink_regular_capturing_size(&day, &address.idx_name())?;
        day.sync_all()?;
        prune_empty_calendar(&self.root, address.day)?;
        Ok(SegmentRemoval {
            zms_bytes: zms_bytes.unwrap_or(0),
            idx_bytes,
        })
    }

    /// Removes an index sidecar that has no sibling ZMS and prunes the day.
    ///
    /// Returns the bytes reclaimed, or zero if the sidecar was already gone.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry is an unsafe type or the unlink fails for
    /// a reason other than the file already being absent.
    pub fn remove_orphan_index(&self, address: SegmentAddress) -> Result<u64, LayoutError> {
        let day = match self.root.open_day(address.day) {
            Ok(day) => day,
            Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(0);
            }
            Err(error) => return Err(error),
        };
        let freed = unlink_regular_capturing_size(&day, &address.idx_name())?;
        day.sync_all()?;
        prune_empty_calendar(&self.root, address.day)?;
        Ok(freed.unwrap_or(0))
    }
}

/// Exclusive, crash-safe ZMS publication in one verified day directory.
#[derive(Debug)]
pub struct ZmsTemp<'owner> {
    _owner: &'owner WriterOwner,
    day: File,
    file: File,
    prepared_identity: Cell<Option<FileIdentity>>,
    pub(super) temp_name: String,
    final_name: String,
    address: SegmentAddress,
    published: bool,
}

impl ZmsTemp<'_> {
    /// Returns the temporary file to the segment encoder.
    ///
    /// The caller must flush buffered writers before [`publish`](Self::publish).
    pub const fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Clones the open temporary descriptor and freezes its exact identity for
    /// validation or exact recovery comparison.
    ///
    /// Mutating the temporary after this call makes publication fail.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the descriptor cannot be duplicated.
    pub fn try_clone_file(&self) -> Result<File, LayoutError> {
        let file = self.file.try_clone()?;
        self.prepared_identity
            .set(Some(FileIdentity::from_file(&file)?));
        Ok(file)
    }

    /// Synchronizes and publishes the ZMS without overwriting an existing one.
    ///
    /// The day directory is synchronized after adding the final link and again
    /// after removing the temporary name.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::SegmentAlreadyExists`] for an existing final ZMS,
    /// preserving it unchanged.
    pub fn publish(&mut self) -> Result<(), LayoutError> {
        self.file.sync_all()?;
        let current_identity = FileIdentity::from_file(&self.file)?;
        let expected_identity = self.prepared_identity.get().unwrap_or(current_identity);
        if current_identity != expected_identity {
            return Err(LayoutError::TemporaryChanged {
                name: self.temp_name.clone(),
            });
        }
        verify_named_identity(
            &self.day,
            &self.temp_name,
            expected_identity,
            &self.temp_name,
        )?;
        match link_open_file(&self.file, &self.day, &self.temp_name, &self.final_name) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::EXIST => {
                return Err(LayoutError::SegmentAlreadyExists {
                    id: self.address.id,
                });
            }
            Err(error) => return Err(LayoutError::Io(errno_to_io(error))),
        }
        // Adding a hard link changes the inode ctime, so compare the final name
        // with the exact post-link identity of the still-open descriptor.
        let linked_identity = FileIdentity::from_file(&self.file)?;
        verify_named_identity(
            &self.day,
            &self.final_name,
            linked_identity,
            &self.temp_name,
        )?;
        self.day.sync_all()?;
        rustix::fs::unlinkat(&self.day, &self.temp_name, AtFlags::empty())
            .map_err(errno_to_io)
            .map_err(LayoutError::Io)?;
        self.day.sync_all()?;
        self.published = true;
        Ok(())
    }

    /// Removes an unpublished temporary name and synchronizes its day.
    ///
    /// This is used after recovery proved that an already published final ZMS
    /// is byte-identical to the newly encoded journal contents.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if cleanup cannot be made durable.
    pub fn discard(mut self) -> Result<(), LayoutError> {
        let expected = FileIdentity::from_file(&self.file)?;
        if !unlink_named_if_identity(&self.day, &self.temp_name, expected)? {
            return Err(LayoutError::TemporaryChanged {
                name: self.temp_name.clone(),
            });
        }
        self.day.sync_all()?;
        self.published = true;
        Ok(())
    }
}

impl Drop for ZmsTemp<'_> {
    fn drop(&mut self) {
        if !self.published
            && let Ok(expected) = FileIdentity::from_file(&self.file)
        {
            drop(unlink_named_if_identity(
                &self.day,
                &self.temp_name,
                expected,
            ));
        }
    }
}
