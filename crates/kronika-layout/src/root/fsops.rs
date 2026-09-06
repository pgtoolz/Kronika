//! Descriptor-relative filesystem operations.

use super::*;

pub(super) fn stat_no_follow(
    directory: &File,
    name: &str,
) -> Result<rustix::fs::Stat, LayoutError> {
    let name = CString::new(name).map_err(|_error| {
        LayoutError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "layout leaf contains a NUL byte",
        ))
    })?;
    stat_no_follow_name(directory, &name)
}

pub(super) fn stat_no_follow_name(
    directory: &File,
    name: &CStr,
) -> Result<rustix::fs::Stat, LayoutError> {
    rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(errno_to_io)
        .map_err(LayoutError::Io)
}

pub(super) fn open_directory_at(directory: &File, name: &str) -> Result<File, LayoutError> {
    rustix::fs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| {
        if error == rustix::io::Errno::LOOP {
            LayoutError::SymlinkNotAllowed {
                name: name.to_owned(),
            }
        } else {
            LayoutError::Io(errno_to_io(error))
        }
    })
}

pub(super) fn ensure_directory_at(directory: &File, name: &str) -> Result<File, LayoutError> {
    match rustix::fs::mkdirat(directory, name, DIRECTORY_MODE) {
        Ok(()) => {}
        Err(error) if error == rustix::io::Errno::EXIST => {}
        Err(error) => return Err(LayoutError::Io(errno_to_io(error))),
    }
    let child = open_directory_at(directory, name)?;
    // An earlier attempt may have completed mkdirat but failed before its
    // parent fsync. EEXIST therefore cannot prove that the entry is durable.
    directory.sync_all()?;
    Ok(child)
}

pub(super) fn open_regular_at(
    directory: &File,
    name: &str,
    access: OFlags,
) -> Result<File, LayoutError> {
    let name = CString::new(name).map_err(|_error| {
        LayoutError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "layout leaf contains a NUL byte",
        ))
    })?;
    open_regular_name_at(directory, &name, access)
}

pub(super) fn open_regular_name_at(
    directory: &File,
    name: &CStr,
    access: OFlags,
) -> Result<File, LayoutError> {
    let file = rustix::fs::openat(
        directory,
        name,
        access | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| {
        if error == rustix::io::Errno::LOOP {
            LayoutError::SymlinkNotAllowed {
                name: format!("opaque:{:016x}", hash_name(name.to_bytes())),
            }
        } else {
            LayoutError::Io(errno_to_io(error))
        }
    })?;
    if !file.metadata()?.is_file() {
        return Err(LayoutError::UnexpectedLeafEntryType {
            name: format!("opaque:{:016x}", hash_name(name.to_bytes())),
        });
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
pub(super) fn link_open_file(
    file: &File,
    directory: &File,
    _temporary_name: &str,
    final_name: &str,
) -> rustix::io::Result<()> {
    let descriptor_path = format!("/proc/self/fd/{}", file.as_raw_fd());
    match rustix::fs::linkat(
        rustix::fs::CWD,
        descriptor_path,
        directory,
        final_name,
        AtFlags::SYMLINK_FOLLOW,
    ) {
        Ok(()) => Ok(()),
        Err(error) if error == rustix::io::Errno::NOENT => {
            rustix::fs::linkat(file, "", directory, final_name, AtFlags::EMPTY_PATH)
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn link_open_file(
    _file: &File,
    directory: &File,
    temporary_name: &str,
    final_name: &str,
) -> rustix::io::Result<()> {
    rustix::fs::linkat(
        directory,
        temporary_name,
        directory,
        final_name,
        AtFlags::empty(),
    )
}

pub(super) fn create_regular_at(
    directory: &File,
    name: &str,
    access: OFlags,
    mode: Mode,
) -> Result<File, LayoutError> {
    let name = CString::new(name).map_err(|_error| {
        LayoutError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "layout leaf contains a NUL byte",
        ))
    })?;
    create_regular_name_at(directory, &name, access, mode)
}

pub(super) fn create_regular_name_at(
    directory: &File,
    name: &CStr,
    access: OFlags,
    mode: Mode,
) -> Result<File, LayoutError> {
    rustix::fs::openat(
        directory,
        name,
        access | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        mode,
    )
    .map(File::from)
    .map_err(errno_to_io)
    .map_err(LayoutError::Io)
}

pub(super) fn open_or_create_regular(
    directory: &File,
    name: &str,
    access: OFlags,
    mode: Mode,
) -> Result<(File, bool), LayoutError> {
    match open_regular_at(directory, name, access) {
        Ok(file) => Ok((file, false)),
        Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            match create_regular_at(directory, name, access, mode) {
                Ok(file) => Ok((file, true)),
                Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                    open_regular_at(directory, name, access).map(|file| (file, false))
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
pub(super) fn rename_noreplace(
    source_directory: &File,
    source_name: &CStr,
    destination_directory: &File,
    destination_name: &CStr,
) -> io::Result<()> {
    rustix::fs::renameat_with(
        source_directory,
        source_name,
        destination_directory,
        destination_name,
        RenameFlags::NOREPLACE,
    )
    .map_err(errno_to_io)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn rename_noreplace(
    _source_directory: &File,
    _source_name: &CStr,
    _destination_directory: &File,
    _destination_name: &CStr,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unsupported on this platform",
    ))
}

pub(super) fn sync_source_directory(directory: &File) -> io::Result<()> {
    directory.sync_all()
}

pub(super) fn remove_verified_regular(
    root: &DataRoot,
    address: SegmentAddress,
    name: &str,
) -> Result<(), LayoutError> {
    let day = root.open_day(address.day)?;
    let stat = stat_no_follow(&day, name)?;
    let kind = FileType::from_raw_mode(stat.st_mode);
    if kind == FileType::Symlink {
        return Err(LayoutError::SymlinkNotAllowed {
            name: name.to_owned(),
        });
    }
    if kind != FileType::RegularFile {
        return Err(LayoutError::UnexpectedLeafEntryType {
            name: name.to_owned(),
        });
    }
    rustix::fs::unlinkat(&day, name, AtFlags::empty())
        .map_err(errno_to_io)
        .map_err(LayoutError::Io)?;
    day.sync_all()?;
    Ok(())
}

/// Unlinks a regular leaf and reports the bytes it held.
///
/// A missing name returns `Ok(None)`; the day directory is not synchronized
/// here so a caller removing several siblings can batch one `sync_all`.
pub(super) fn unlink_regular_capturing_size(
    directory: &File,
    name: &str,
) -> Result<Option<u64>, LayoutError> {
    let stat = match stat_no_follow(directory, name) {
        Ok(stat) => stat,
        Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let kind = FileType::from_raw_mode(stat.st_mode);
    if kind == FileType::Symlink {
        return Err(LayoutError::SymlinkNotAllowed {
            name: name.to_owned(),
        });
    }
    if kind != FileType::RegularFile {
        return Err(LayoutError::UnexpectedLeafEntryType {
            name: name.to_owned(),
        });
    }
    let bytes = u64::try_from(stat.st_size).unwrap_or(u64::MAX);
    rustix::fs::unlinkat(directory, name, AtFlags::empty())
        .map_err(errno_to_io)
        .map_err(LayoutError::Io)?;
    Ok(Some(bytes))
}

pub(super) fn verify_named_identity(
    directory: &File,
    file_name: &str,
    expected: FileIdentity,
    temporary_name: &str,
) -> Result<(), LayoutError> {
    let file = open_regular_at(directory, file_name, OFlags::RDONLY)?;
    if !FileIdentity::from_file(&file)?.same_named_object(expected) {
        return Err(LayoutError::TemporaryChanged {
            name: temporary_name.to_owned(),
        });
    }
    Ok(())
}

pub(super) fn unlink_named_if_identity(
    directory: &File,
    name: &str,
    expected: FileIdentity,
) -> Result<bool, LayoutError> {
    let named = match open_regular_at(directory, name, OFlags::RDONLY) {
        Ok(file) => file,
        Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    if !FileIdentity::from_file(&named)?.same_named_object(expected) {
        return Ok(false);
    }
    rustix::fs::unlinkat(directory, name, AtFlags::empty())
        .map_err(errno_to_io)
        .map_err(LayoutError::Io)?;
    Ok(true)
}

pub(super) fn remove_regular_if_identity(
    root: &DataRoot,
    address: SegmentAddress,
    name: &str,
    device: u64,
    inode: u64,
) -> Result<bool, LayoutError> {
    let day = root.open_day(address.day)?;
    let file = match open_regular_at(&day, name, OFlags::RDONLY) {
        Ok(file) => file,
        Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if metadata.dev() != device || metadata.ino() != inode {
        return Ok(false);
    }
    rustix::fs::unlinkat(&day, name, AtFlags::empty())
        .map_err(errno_to_io)
        .map_err(LayoutError::Io)?;
    day.sync_all()?;
    Ok(true)
}

pub(super) fn remove_empty_directory_at(parent: &File, name: &str) -> Result<bool, LayoutError> {
    match rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR) {
        Ok(()) => {
            parent.sync_all()?;
            Ok(true)
        }
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTEMPTY | rustix::io::Errno::EXIST) => {
            Ok(false)
        }
        Err(error) => Err(LayoutError::Io(errno_to_io(error))),
    }
}

pub(super) fn temporary_name(address: SegmentAddress, kind: TemporaryKind) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    match kind {
        TemporaryKind::Zms => format!("{}.zms.{pid}.{sequence}.tmp", address.id),
        TemporaryKind::Idx => format!("{}.idx.{pid}.{sequence}.tmp", address.id),
        TemporaryKind::IndexProbe => {
            format!("{}.idx.probe.{pid}.{sequence}.tmp", address.id)
        }
    }
}

pub(super) fn errno_to_layout(error: rustix::io::Errno) -> LayoutError {
    LayoutError::Io(errno_to_io(error))
}

pub(super) fn errno_to_io(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}
