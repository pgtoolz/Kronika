//! Reader for a `/proc` tree whose root is overridable for tests and for
//! host-mounted deployments.
//!
//! Also provides [`statvfs`] for filesystem capacity queries.

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// Filesystem capacity at a mount point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsSpace {
    /// Total filesystem size in bytes (`f_blocks * f_frsize`).
    pub total_bytes: i64,
    /// Available bytes for unprivileged writes (`f_bavail * f_frsize`).
    pub free_bytes: i64,
    /// Total filesystem inode/file-serial count (`f_files`).
    pub total_inodes: i64,
    /// Inodes/file serials available to unprivileged users (`f_favail`).
    pub available_inodes: i64,
}

/// One child directory under a configured filesystem root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntryName {
    /// File name only, never a path.
    pub name: String,
    /// Whether the entry is a directory according to `file_type`.
    pub is_dir: bool,
}

/// Convert raw `statvfs` fields to [`FsSpace`].
///
/// Saturates to `i64::MAX` when the product exceeds `i64::MAX` so that
/// very large filesystems never wrap or panic.
#[must_use]
pub fn space_from_raw(blocks: u64, bavail: u64, frsize: u64, files: u64, favail: u64) -> FsSpace {
    let saturating_mul =
        |a: u64, b: u64| -> i64 { a.saturating_mul(b).min(i64::MAX as u64).cast_signed() };
    FsSpace {
        total_bytes: saturating_mul(blocks, frsize),
        free_bytes: saturating_mul(bavail, frsize),
        total_inodes: files.min(i64::MAX as u64).cast_signed(),
        available_inodes: favail.min(i64::MAX as u64).cast_signed(),
    }
}

/// Query filesystem capacity for `mount_point`.
///
/// **Env fixture override:** if `KRONIKA_STATVFS_FIXTURE` is set, its value
/// is parsed as `path1=TOTAL:FREE:INODES:AVAILABLE_INODES;...` (decimal). The
/// entry whose path equals `mount_point` is returned; no entry returns `None`.
/// This lets BDD tests inject deterministic capacity without a real filesystem.
///
/// Otherwise calls `statvfs(2)` and maps success via [`space_from_raw`].
/// Returns `None` on any syscall error because a mount can vanish mid-scan.
#[must_use]
pub fn statvfs(mount_point: &str) -> Option<FsSpace> {
    if let Ok(fixture) = std::env::var("KRONIKA_STATVFS_FIXTURE") {
        return parse_fixture(&fixture, mount_point);
    }
    rustix::fs::statvfs(mount_point)
        .ok()
        .map(|s| space_from_raw(s.f_blocks, s.f_bavail, s.f_frsize, s.f_files, s.f_favail))
}

pub(crate) fn parse_fixture(fixture: &str, mount_point: &str) -> Option<FsSpace> {
    for entry in fixture.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((path, rest)) = entry.split_once('=') else {
            continue;
        };
        let fields = rest.split(':').collect::<Vec<_>>();
        let [total, free, total_inodes, available_inodes] = fields.as_slice() else {
            continue;
        };
        if path == mount_point {
            return Some(FsSpace {
                total_bytes: total.trim().parse().ok()?,
                free_bytes: free.trim().parse().ok()?,
                total_inodes: total_inodes.trim().parse().ok()?,
                available_inodes: available_inodes.trim().parse().ok()?,
            });
        }
    }
    None
}

/// Maximum bytes read from one procfs file.
///
/// Wave 1 files are small, but fixture roots and host-mounted procfs paths are
/// still external input. The collector rejects larger files before parsing.
pub const MAX_PROC_FILE_BYTES: usize = 4 * 1024 * 1024;

/// A `/proc` root. Real collection uses `/proc`; tests and host-mounted pods
/// point it elsewhere via `KRONIKA_PROC_ROOT`.
#[derive(Debug, Clone)]
pub struct ProcFs {
    root: PathBuf,
}

impl ProcFs {
    /// Root from `KRONIKA_PROC_ROOT`, defaulting to `/proc`.
    #[must_use]
    pub fn from_env() -> Self {
        let root = std::env::var_os("KRONIKA_PROC_ROOT")
            .map_or_else(|| PathBuf::from("/proc"), PathBuf::from);
        Self { root }
    }

    /// A reader rooted at `root`.
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Return the absolute path for a checked procfs-relative path.
    ///
    /// # Errors
    /// Returns an error when `rel` is empty or escapes the configured root.
    pub fn path(&self, rel: &str) -> io::Result<PathBuf> {
        Ok(self.root.join(checked_relative_path(rel)?))
    }

    /// Read `<root>/<rel>`, trimmed; empty content is an error.
    ///
    /// # Errors
    /// Returns the underlying `io::Error` (with the path) or an empty-file error.
    pub fn read(&self, rel: &str) -> io::Result<String> {
        let mut content = self.read_raw(rel)?;
        trim_string(&mut content);
        if content.is_empty() {
            return Err(io::Error::other(format!("{rel}: empty")));
        }
        Ok(content)
    }

    /// Read `<root>/<rel>` without trimming.
    ///
    /// # Errors
    /// Returns an error if `rel` is empty, escapes the root, exceeds
    /// [`MAX_PROC_FILE_BYTES`], or cannot be read as UTF-8.
    pub fn read_raw(&self, rel: &str) -> io::Result<String> {
        let mut path = PathBuf::new();
        let mut content = String::new();
        self.read_raw_into(rel, &mut path, &mut content)?;
        Ok(content)
    }

    pub(crate) fn read_raw_into(
        &self,
        rel: &str,
        path: &mut PathBuf,
        content: &mut String,
    ) -> io::Result<()> {
        let rel_path = checked_relative_path(rel)?;
        path.clear();
        path.push(&self.root);
        path.push(rel_path);
        let mut file = std::fs::File::open(&path).map_err(|err| tag_io_error(rel, &err))?;
        content.clear();
        file.by_ref()
            .take((MAX_PROC_FILE_BYTES + 1) as u64)
            .read_to_string(content)
            .map_err(|err| tag_io_error(rel, &err))?;
        if content.len() > MAX_PROC_FILE_BYTES {
            return Err(io::Error::other(format!(
                "{rel}: exceeds {MAX_PROC_FILE_BYTES} byte procfs read limit"
            )));
        }
        Ok(())
    }

    /// Every numeric `/proc` directory name, sorted ascending.
    ///
    /// # Errors
    /// Returns the underlying `read_dir` error for the proc root.
    pub fn pid_dirs(&self) -> io::Result<Vec<i32>> {
        let mut pids = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                continue;
            }
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
            else {
                continue;
            };
            pids.push(pid);
        }
        pids.sort_unstable();
        Ok(pids)
    }
}

/// A `/sys` root, overridable via `KRONIKA_SYS_ROOT`.
///
/// Used to recover the real `(major, minor)` of `major == 0` subvolume mounts
/// (btrfs, ZFS) from `class/block/<name>/dev`, and to let BDD fixture the
/// sysfs tree.
#[derive(Debug, Clone)]
pub struct SysFs {
    root: PathBuf,
}

impl SysFs {
    /// Root from `KRONIKA_SYS_ROOT`, defaulting to `/sys`.
    #[must_use]
    pub fn from_env() -> Self {
        let root = std::env::var_os("KRONIKA_SYS_ROOT")
            .map_or_else(|| PathBuf::from("/sys"), PathBuf::from);
        Self { root }
    }

    /// A reader rooted at `root`.
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Read `<root>/<rel>`, trimmed; empty content is an error.
    ///
    /// # Errors
    /// Returns the underlying `io::Error` (with the path), an empty-file error,
    /// or a path-escape error when `rel` leaves the configured root.
    pub fn read(&self, rel: &str) -> io::Result<String> {
        let rel_path = checked_relative_path(rel)?;
        let path = self.root.join(rel_path);
        let mut file = std::fs::File::open(&path).map_err(|err| tag_io_error(rel, &err))?;
        let mut content = String::new();
        file.by_ref()
            .take((MAX_PROC_FILE_BYTES + 1) as u64)
            .read_to_string(&mut content)
            .map_err(|err| tag_io_error(rel, &err))?;
        if content.len() > MAX_PROC_FILE_BYTES {
            return Err(io::Error::other(format!(
                "{rel}: exceeds {MAX_PROC_FILE_BYTES} byte sysfs read limit"
            )));
        }
        trim_string(&mut content);
        if content.is_empty() {
            return Err(io::Error::other(format!("{rel}: empty")));
        }
        Ok(content)
    }

    /// Return the absolute path for a checked relative sysfs path.
    ///
    /// # Errors
    /// Returns an error when `rel` is empty or escapes the configured root.
    pub fn path(&self, rel: &str) -> io::Result<PathBuf> {
        Ok(self.root.join(checked_relative_path(rel)?))
    }

    /// Resolve a checked sysfs symlink while keeping the result under this root.
    ///
    /// # Errors
    /// Returns an I/O error when either path cannot be resolved or the target
    /// leaves the configured sysfs root.
    pub fn canonical_path(&self, rel: &str) -> io::Result<PathBuf> {
        let root = std::fs::canonicalize(&self.root)?;
        let target = std::fs::canonicalize(self.root.join(checked_relative_path(rel)?))?;
        if !target.starts_with(&root) {
            return Err(io::Error::other(format!(
                "{rel}: resolved sysfs path leaves the configured root"
            )));
        }
        Ok(target)
    }

    /// Whether a checked sysfs path currently exists.
    #[must_use]
    pub fn exists(&self, rel: &str) -> bool {
        self.path(rel).is_ok_and(|path| path.exists())
    }

    /// Read immediate children under `<root>/<rel>`.
    ///
    /// # Errors
    /// Returns the underlying `read_dir` error or a path validation error.
    pub fn read_dir(&self, rel: &str) -> io::Result<Vec<DirEntryName>> {
        let path = self.path(rel)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            entries.push(DirEntryName {
                name,
                is_dir: entry.file_type()?.is_dir(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

fn trim_string(value: &mut String) {
    let trimmed = value.trim();
    let start = trimmed.as_ptr() as usize - value.as_ptr() as usize;
    let end = start + trimmed.len();
    value.truncate(end);
    if start != 0 {
        value.drain(..start);
    }
}

/// Parse a `MAJ:MIN` device string (the content of `class/block/<name>/dev`).
///
/// Returns `None` when the string is not exactly two colon-separated `i32`s.
#[must_use]
pub fn parse_dev_pair(content: &str) -> Option<(i32, i32)> {
    let (major, minor) = content.trim().split_once(':')?;
    Some((major.trim().parse().ok()?, minor.trim().parse().ok()?))
}

fn checked_relative_path(rel: &str) -> io::Result<&Path> {
    if rel.trim().is_empty() {
        return Err(io::Error::other("empty proc-relative path"));
    }
    let path = Path::new(rel);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(io::Error::other(format!(
            "{rel}: proc-relative path must stay under the configured root"
        )));
    }
    Ok(path)
}

fn tag_io_error(rel: &str, err: &io::Error) -> io::Error {
    io::Error::new(err.kind(), format!("{rel}: {err}"))
}

#[cfg(test)]
mod tests;
