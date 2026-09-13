//! One pass over exposed cgroup v2 directories, independent of process visibility.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::MetadataExt;

use rustix::fs::{Dir, FileType, Mode, OFlags, openat};

use super::selected::{Mount, directory_identity, exposed_mounts};
use super::{CpuQuota, ProcFs, SysFs, parse_cpu_max_strict, parse_cpuset_count};

mod model;
mod primary;
mod read;
pub use model::*;

const MAX_METRIC_BYTES: usize = 64 * 1024;
const MAX_IO_LINE_BYTES: usize = 4096;

struct Frame {
    directory: File,
    entries: Dir,
    path: String,
    identity: String,
}

/// Visit every accessible directory under exposed cgroup2 mounts once.
///
/// Directory descriptors anchor child and metric reads. Symlinks are not followed.
/// Memory holds one traversal stack, object identities for alias deduplication,
/// and one group's scalar readings; device rows are emitted individually.
///
/// # Errors
/// Returns mount-information failures or the callback's error. Inaccessible and
/// disappearing directories are reported in the returned statistics; readable
/// peers are still collected.
pub fn walk_visible_v2(
    procfs: &ProcFs,
    _sys: &SysFs,
    ts: i64,
    emit: impl FnMut(DiscoveryRow<'_>) -> io::Result<()>,
) -> io::Result<DiscoveryStats> {
    walk(procfs, ts, &primary::Primary::default(), emit)
}

/// Discover groups while reusing the primary ancestor chain's recorded limits.
///
/// # Errors
/// Returns mount-information or callback errors.
pub fn walk_visible_v2_with_primary(
    procfs: &ProcFs,
    sys: &SysFs,
    ts: i64,
    selected: &super::AncestorContext,
    mut emit: impl FnMut(DiscoveryRow<'_>, &super::AncestorContext) -> io::Result<()>,
) -> io::Result<DiscoveryStats> {
    let primary = primary::Primary::read(procfs, sys, selected)?;
    walk(procfs, ts, &primary, |row| emit(row, &primary.selected))
}

#[allow(
    clippy::too_many_lines,
    reason = "the depth-first pass keeps one descriptor stack and shared object set"
)]
fn walk(
    procfs: &ProcFs,
    ts: i64,
    primary: &primary::Primary,
    mut emit: impl FnMut(DiscoveryRow<'_>) -> io::Result<()>,
) -> io::Result<DiscoveryStats> {
    let mut exposed = exposed_mounts(procfs)?;
    exposed.sort_by(|left, right| {
        left.root
            .len()
            .cmp(&right.root.len())
            .then(left.base.cmp(&right.base))
    });
    let mount_points = exposed
        .iter()
        .map(|mount| mount.point.clone())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut stats = primary.stats.clone();
    let mut traverse = |root: io::Result<File>, path: String, mount: &Mount| -> io::Result<()> {
        let root = match root {
            Ok(root) => root,
            Err(error) => {
                stats.directory_error(&mount.base, &error);
                return Ok(());
            }
        };
        let root_device = match root.metadata() {
            Ok(metadata) => metadata.dev(),
            Err(error) => {
                stats.directory_error(&mount.base, &error);
                return Ok(());
            }
        };
        let Some(frame) = visit(
            root, path, None, mount, ts, &mut seen, &mut stats, primary, &mut emit,
        )?
        else {
            return Ok(());
        };
        let mut stack = vec![frame];
        while let Some(parent) = stack.last_mut() {
            let entry = match parent.entries.next() {
                Some(Ok(entry)) => entry,
                Some(Err(error)) => {
                    stats.directory_error(&parent.path, &io::Error::from(error));
                    stack.pop();
                    continue;
                }
                None => {
                    stack.pop();
                    continue;
                }
            };
            let name = entry.file_name();
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            if !matches!(entry.file_type(), FileType::Directory | FileType::Unknown) {
                continue;
            }
            let Ok(name_text) = name.to_str() else {
                stats.directory_error(&parent.path, &io::Error::other("non-UTF-8 directory name"));
                continue;
            };
            let path = if parent.path == "/" {
                format!("/{name_text}")
            } else {
                format!("{}/{name_text}", parent.path)
            };
            if mount_points.contains(&mount.point.join(path.trim_start_matches('/'))) {
                continue;
            }
            let directory = match openat(&parent.directory, name, directory_flags(), Mode::empty())
            {
                Ok(directory) => File::from(directory),
                Err(error) => {
                    stats.directory_error(&path, &io::Error::from(error));
                    continue;
                }
            };
            // A separately mounted hierarchy is visited from its own mountinfo root.
            if directory
                .metadata()
                .is_ok_and(|metadata| metadata.dev() != root_device)
            {
                continue;
            }
            let parent_identity = Some(parent.identity.clone());
            if let Some(frame) = visit(
                directory,
                path,
                parent_identity,
                mount,
                ts,
                &mut seen,
                &mut stats,
                primary,
                &mut emit,
            )? {
                stack.push(frame);
            }
        }
        Ok(())
    };
    for mount in exposed {
        let root = rustix::fs::open(&mount.point, directory_flags(), Mode::empty())
            .map(File::from)
            .map_err(io::Error::from);
        traverse(root, "/".to_owned(), &mount)?;
    }
    if let Some((directory, path, mount)) = &primary.target {
        traverse(Ok(directory.try_clone()?), path.clone(), mount)?;
    }
    Ok(stats)
}

pub(super) fn charged_devices(
    sys: &SysFs,
    group: &super::SelectedCgroup,
) -> io::Result<Vec<(i32, i32)>> {
    let path = sys.canonical_path(&format!(
        "{}/{}",
        group.base,
        group.path.trim_start_matches('/')
    ))?;
    let directory = File::from(rustix::fs::open(path, directory_flags(), Mode::empty())?);
    let mut devices = Vec::new();
    read::io_rows(
        &directory,
        0,
        &group.path,
        &group.identity,
        &mut DiscoveryStats::default(),
        &mut |row| {
            if let DiscoveryRow::Io(row) = row
                && let (Ok(major), Ok(minor)) = (i32::try_from(row.major), i32::try_from(row.minor))
            {
                devices.push((major, minor));
            }
            Ok(())
        },
    )?;
    Ok(devices)
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

#[allow(
    clippy::too_many_arguments,
    reason = "one directory shares the pass identity set, primary limits, and append callback"
)]
fn visit(
    directory: File,
    path: String,
    parent_identity: Option<String>,
    mount: &Mount,
    ts: i64,
    seen: &mut HashSet<(u64, u64)>,
    stats: &mut DiscoveryStats,
    primary: &primary::Primary,
    emit: &mut impl FnMut(DiscoveryRow<'_>) -> io::Result<()>,
) -> io::Result<Option<Frame>> {
    let metadata = match directory.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            stats.directory_error(&path, &error);
            return Ok(None);
        }
    };
    if !seen.insert((metadata.dev(), metadata.ino())) {
        return Ok(None);
    }
    let identity = directory_identity(&mount.base, &mount.root, &metadata);
    let group = read::group(
        &directory,
        DiscoveredGroup {
            ts,
            cgroup_path: path.clone(),
            cgroup_identity: identity.clone(),
            mount_root: mount.root.clone(),
            parent_identity,
            device: metadata.dev(),
            inode: metadata.ino(),
            memory_localevents: mount.memory_localevents,
            pids_localevents: mount.pids_localevents,
            cpu: DiscoveredCpu::default(),
            memory: DiscoveredMemory::default(),
            pids: DiscoveredPids::default(),
        },
        stats,
        primary.limits.get(&(metadata.dev(), metadata.ino())),
    );
    emit(DiscoveryRow::Group(&group))?;
    stats.groups += 1;
    read::io(&directory, &group, stats, emit)?;
    let entries =
        match openat(&directory, c".", directory_flags(), Mode::empty()).and_then(Dir::new) {
            Ok(entries) => entries,
            Err(error) => {
                stats.directory_error(&path, &io::Error::from(error));
                return Ok(None);
            }
        };
    Ok(Some(Frame {
        directory,
        entries,
        path,
        identity,
    }))
}

impl DiscoveryStats {
    fn directory_error(&mut self, path: &str, error: &io::Error) {
        self.skipped_directories += 1;
        self.first_error
            .get_or_insert_with(|| format!("{path}: {error}"));
    }

    fn metric_error(&mut self, path: &str, file: &str, error: &io::Error) {
        self.metric_errors += 1;
        self.first_error
            .get_or_insert_with(|| format!("{path}/{file}: {error}"));
    }
}

#[cfg(test)]
mod tests;
