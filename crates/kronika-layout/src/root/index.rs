//! Index-sidecar ownership and publication.

use super::*;

/// Lifetime token for the only process allowed to publish index sidecars.
#[derive(Debug)]
pub struct IndexOwner {
    pub(super) root: DataRoot,
    pub(super) _lock: File,
}

impl IndexOwner {
    /// Returns read-only access to the same verified root.
    #[must_use]
    pub const fn root(&self) -> &DataRoot {
        &self.root
    }

    /// Creates an IDX temporary and captures the input ZMS file identity.
    ///
    /// # Errors
    ///
    /// Returns an error if the source ZMS or destination day is unsafe.
    pub fn create_idx_temp(&self, address: SegmentAddress) -> Result<IdxTemp<'_>, LayoutError> {
        self.create_index_temp(address, TemporaryKind::Idx)
    }

    /// Creates a writeability-probe temporary beside the addressed ZMS.
    ///
    /// # Errors
    ///
    /// Returns an error if the source ZMS or destination day is unsafe.
    pub fn create_probe_temp(&self, address: SegmentAddress) -> Result<IdxTemp<'_>, LayoutError> {
        self.create_index_temp(address, TemporaryKind::IndexProbe)
    }

    pub(super) fn create_index_temp(
        &self,
        address: SegmentAddress,
        kind: TemporaryKind,
    ) -> Result<IdxTemp<'_>, LayoutError> {
        let source = self.root.open_zms(address)?;
        let input_file_identity = FileIdentity::from_file(&source)?;
        let day = self.root.open_day(address.day)?;
        let temp_name = temporary_name(address, kind);
        let file = create_regular_at(&day, &temp_name, OFlags::RDWR, DATA_FILE_MODE)?;
        Ok(IdxTemp {
            _owner: self,
            root: self.root.clone(),
            day,
            file,
            prepared_identity: Cell::new(None),
            temp_name,
            final_name: address.idx_name(),
            address,
            input_file_identity,
            kind,
            completed: false,
        })
    }

    /// Removes a verified IDX and synchronizes its day.
    ///
    /// # Errors
    ///
    /// Returns an error if the file changed to an unsafe type or unlink fails.
    pub fn remove_idx(&self, address: SegmentAddress) -> Result<(), LayoutError> {
        remove_verified_regular(&self.root, address, &address.idx_name())
    }

    /// Removes an IDX only if the currently named object still has the
    /// previously observed filesystem identity.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry is unsafe or the unlink cannot be
    /// synchronized. A changed or missing entry returns `Ok(false)`.
    pub fn remove_idx_if_identity(
        &self,
        address: SegmentAddress,
        device: u64,
        inode: u64,
    ) -> Result<bool, LayoutError> {
        remove_regular_if_identity(&self.root, address, &address.idx_name(), device, inode)
    }

    /// Removes a verified stale IDX or probe temporary.
    ///
    /// # Errors
    ///
    /// Returns an error for a writer temporary, changed type, or I/O failure.
    pub fn remove_temporary(&self, temporary: &TemporaryObject) -> Result<(), LayoutError> {
        if temporary.kind == TemporaryKind::Zms {
            return Err(LayoutError::UnexpectedLeafEntry {
                name: temporary.file_name.clone(),
            });
        }
        remove_verified_regular(&self.root, temporary.address, temporary.file_name())
    }

    /// Removes an index-owned temporary only if its filesystem identity is
    /// unchanged since the strict inventory.
    ///
    /// # Errors
    ///
    /// Returns an error for a writer temporary or unsafe entry. A changed or
    /// missing object returns `Ok(false)`.
    pub fn remove_temporary_if_identity(
        &self,
        temporary: &TemporaryObject,
        device: u64,
        inode: u64,
    ) -> Result<bool, LayoutError> {
        if temporary.kind == TemporaryKind::Zms {
            return Err(LayoutError::UnexpectedLeafEntry {
                name: temporary.file_name.clone(),
            });
        }
        remove_regular_if_identity(
            &self.root,
            temporary.address,
            temporary.file_name(),
            device,
            inode,
        )
    }

    /// Removes an empty UTC day and then its empty month/year ancestors.
    ///
    /// A non-empty or concurrently removed directory is a successful no-op.
    /// Pruning is also a no-op while a writer owns the root, so a collector
    /// cannot publish through a descriptor for a directory removed underneath
    /// it.
    /// Every removed directory entry is synchronized in its parent.
    ///
    /// # Errors
    ///
    /// Returns an error when an existing calendar component is unsafe or a
    /// filesystem operation other than the expected empty-directory races
    /// fails.
    pub fn prune_empty_day(&self, day: UtcDay) -> Result<(), LayoutError> {
        let _writer_quiescence = match self
            .root
            .acquire_lock(WRITER_OWNER_LOCK_NAME, OwnerKind::Writer)
        {
            Ok(lock) => lock,
            Err(LayoutError::OwnerContended {
                owner: OwnerKind::Writer,
            }) => return Ok(()),
            Err(error) => return Err(error),
        };
        prune_empty_calendar(&self.root, day)
    }
}

/// Removes an empty UTC day directory and its now-empty month/year ancestors.
///
/// The caller must already hold writer quiescence: [`IndexOwner`] takes the
/// writer lock first, while [`WriterOwner`] owns it for its whole lifetime. A
/// non-empty or concurrently removed directory ends the walk as a no-op.
pub(super) fn prune_empty_calendar(root: &DataRoot, day: UtcDay) -> Result<(), LayoutError> {
    let year_name = day.year_component();
    let month_name = day.month_component();
    let day_name = day.day_component();
    let year = match open_directory_at(&root.directory, &year_name) {
        Ok(directory) => directory,
        Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let month = match open_directory_at(&year, &month_name) {
        Ok(directory) => directory,
        Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    match open_directory_at(&month, &day_name) {
        Ok(_directory) => {}
        Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(error),
    }
    if !remove_empty_directory_at(&month, &day_name)? {
        return Ok(());
    }
    if !remove_empty_directory_at(&year, &month_name)? {
        return Ok(());
    }
    remove_empty_directory_at(&root.directory, &year_name)?;
    Ok(())
}

/// Exclusive IDX or probe temporary tied to one stable source ZMS.
#[derive(Debug)]
pub struct IdxTemp<'owner> {
    _owner: &'owner IndexOwner,
    root: DataRoot,
    day: File,
    file: File,
    prepared_identity: Cell<Option<FileIdentity>>,
    pub(super) temp_name: String,
    final_name: String,
    address: SegmentAddress,
    input_file_identity: FileIdentity,
    kind: TemporaryKind,
    completed: bool,
}

impl IdxTemp<'_> {
    /// Returns the temporary file to the index encoder.
    pub const fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Clones the open temporary descriptor and freezes its exact identity for
    /// validation before publication.
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

    /// Returns the verified process-unique leaf name for diagnostics and
    /// qualification barriers.
    #[must_use]
    pub fn temp_name(&self) -> &str {
        &self.temp_name
    }

    /// Synchronizes and atomically replaces the final IDX after source
    /// revalidation.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::SourceChanged`] if the ZMS changed while the IDX
    /// was built.
    pub fn publish(mut self) -> Result<(), LayoutError> {
        if self.kind != TemporaryKind::Idx {
            return Err(LayoutError::UnexpectedLeafEntry {
                name: self.temp_name.clone(),
            });
        }
        self.file.sync_all()?;
        let current_identity = FileIdentity::from_file(&self.file)?;
        let expected_temporary = self.prepared_identity.get().unwrap_or(current_identity);
        if current_identity != expected_temporary {
            return Err(LayoutError::TemporaryChanged {
                name: self.temp_name.clone(),
            });
        }
        verify_named_identity(
            &self.day,
            &self.temp_name,
            expected_temporary,
            &self.temp_name,
        )?;
        let current_source = self.root.open_zms(self.address)?;
        if FileIdentity::from_file(&current_source)? != self.input_file_identity {
            return Err(LayoutError::SourceChanged {
                id: self.address.id,
            });
        }
        match stat_no_follow(&self.day, &self.final_name) {
            Ok(stat) => {
                let kind = FileType::from_raw_mode(stat.st_mode);
                if kind == FileType::Symlink {
                    return Err(LayoutError::SymlinkNotAllowed {
                        name: self.final_name.clone(),
                    });
                }
                if kind != FileType::RegularFile {
                    return Err(LayoutError::UnexpectedLeafEntryType {
                        name: self.final_name.clone(),
                    });
                }
            }
            Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        rustix::fs::renameat(&self.day, &self.temp_name, &self.day, &self.final_name)
            .map_err(errno_to_io)
            .map_err(LayoutError::Io)?;
        // Renaming may change inode ctime. Pin the final name to the exact
        // post-rename identity of the descriptor that was validated above.
        let renamed_identity = FileIdentity::from_file(&self.file)?;
        verify_named_identity(
            &self.day,
            &self.final_name,
            renamed_identity,
            &self.temp_name,
        )?;
        self.day.sync_all()?;
        self.completed = true;
        Ok(())
    }

    /// Synchronizes and removes a writeability probe without publishing an IDX.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-probe temporary or failed persistence step.
    pub fn finish_probe(mut self) -> Result<(), LayoutError> {
        if self.kind != TemporaryKind::IndexProbe {
            return Err(LayoutError::UnexpectedLeafEntry {
                name: self.temp_name.clone(),
            });
        }
        self.file.sync_all()?;
        let expected = FileIdentity::from_file(&self.file)?;
        if !unlink_named_if_identity(&self.day, &self.temp_name, expected)? {
            return Err(LayoutError::TemporaryChanged {
                name: self.temp_name.clone(),
            });
        }
        self.day.sync_all()?;
        self.completed = true;
        Ok(())
    }
}

impl Drop for IdxTemp<'_> {
    fn drop(&mut self) {
        if !self.completed
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
