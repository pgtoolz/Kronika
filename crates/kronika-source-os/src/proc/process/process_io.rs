//! Cached filesystem credentials for `/proc/PID/io` reads.

use std::collections::{BTreeMap, HashMap};
use std::io;

use super::{ProcIo, ProcessReader};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FsCredentials {
    uid: u32,
    gid: u32,
}

/// One live process whose I/O counters are due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIoTarget {
    pid: i32,
    uid: u32,
    gid: u32,
}

impl ProcessIoTarget {
    /// Create a target from the process identity recorded in `/proc/PID/status`.
    #[must_use]
    pub const fn new(pid: i32, uid: u32, gid: u32) -> Self {
        Self { pid, uid, gid }
    }

    const fn credentials(self) -> FsCredentials {
        FsCredentials {
            uid: self.uid,
            gid: self.gid,
        }
    }
}

/// Successful filesystem credentials remembered by numeric PID.
#[derive(Debug)]
pub struct ProcessIoCredentials {
    by_pid: HashMap<i32, FsCredentials>,
    baseline: FsCredentials,
}

impl Default for ProcessIoCredentials {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessIoCredentials {
    /// Create an empty collector-lifetime credential cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_pid: HashMap::new(),
            baseline: current_fs_credentials(),
        }
    }

    /// Drop entries whose PID is absent from one successful procfs enumeration.
    pub fn retain_live(&mut self, sorted_pids: &[i32]) {
        self.by_pid
            .retain(|pid, _| sorted_pids.binary_search(pid).is_ok());
    }

    /// Drop one PID after a required procfs file reports that it disappeared.
    pub fn forget(&mut self, pid: i32) {
        self.by_pid.remove(&pid);
    }

    /// Read every target and pass each successful value to its target index.
    ///
    /// Warm reads are grouped by remembered credentials. Unknown PIDs try the
    /// identity recorded in `/proc/PID/status`, then the collector's original
    /// filesystem credentials. The return value counts only PIDs that remained
    /// present but could not be read under any distinct identity tried.
    pub fn read(
        &mut self,
        reader: &mut ProcessReader<'_>,
        targets: &[ProcessIoTarget],
        mut set_value: impl FnMut(usize, ProcIo),
    ) -> usize {
        let mut groups: BTreeMap<FsCredentials, Vec<(usize, i32)>> = BTreeMap::new();
        let mut discover = Vec::new();
        for (index, target) in targets.iter().copied().enumerate() {
            if let Some(credentials) = self.by_pid.get(&target.pid).copied() {
                groups
                    .entry(credentials)
                    .or_default()
                    .push((index, target.pid));
            } else {
                discover.push((index, None));
            }
        }

        for (credentials, entries) in groups {
            with_credentials(
                reader,
                credentials,
                self.baseline,
                false,
                |reader, active| {
                    for (index, pid) in entries {
                        match read_io(reader, pid) {
                            IoRead::Value(value) => set_value(index, value),
                            IoRead::Gone => {
                                self.by_pid.remove(&pid);
                            }
                            IoRead::Unavailable => {
                                self.by_pid.remove(&pid);
                                discover.push((index, Some(active)));
                            }
                        }
                    }
                },
            );
        }

        let mut unavailable = 0_usize;
        for (index, cached_attempt) in discover {
            let target = targets[index];
            match discover_io(reader, target, cached_attempt, self.baseline) {
                Discovered::Value(value, credentials) => {
                    self.by_pid.insert(target.pid, credentials);
                    set_value(index, value);
                }
                Discovered::Gone => {
                    self.by_pid.remove(&target.pid);
                }
                Discovered::Unavailable => {
                    unavailable = unavailable.saturating_add(1);
                }
            }
        }
        unavailable
    }
}

enum IoRead {
    Value(ProcIo),
    Gone,
    Unavailable,
}

impl IoRead {
    const fn discovered(self, credentials: FsCredentials) -> Discovered {
        match self {
            Self::Value(value) => Discovered::Value(value, credentials),
            Self::Gone => Discovered::Gone,
            Self::Unavailable => Discovered::Unavailable,
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_SWITCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_READS: std::cell::RefCell<std::collections::VecDeque<IoRead>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

enum Discovered {
    Value(ProcIo, FsCredentials),
    Gone,
    Unavailable,
}

fn discover_io(
    reader: &mut ProcessReader<'_>,
    target: ProcessIoTarget,
    cached_attempt: Option<FsCredentials>,
    baseline: FsCredentials,
) -> Discovered {
    let observed = target.credentials();
    let mut attempted = cached_attempt;
    if cached_attempt != Some(observed) {
        let mut outcome = None;
        with_credentials(reader, observed, baseline, true, |reader, active| {
            if cached_attempt != Some(active) {
                attempted = Some(active);
                outcome = Some(read_io(reader, target.pid).discovered(active));
            }
        });
        match outcome {
            Some(result @ (Discovered::Value(..) | Discovered::Gone)) => return result,
            Some(Discovered::Unavailable) | None => {}
        }
    }

    if attempted != Some(baseline) && cached_attempt != Some(baseline) {
        return read_io(reader, target.pid).discovered(baseline);
    }
    Discovered::Unavailable
}

fn read_io(reader: &mut ProcessReader<'_>, pid: i32) -> IoRead {
    #[cfg(test)]
    if let Some(result) = TEST_READS.with(|reads| reads.borrow_mut().pop_front()) {
        return result;
    }
    match reader.read_io_raw(pid) {
        Ok(value) => IoRead::Value(value),
        Err(error) if error.kind() == io::ErrorKind::NotFound => IoRead::Gone,
        Err(_) => IoRead::Unavailable,
    }
}

fn with_credentials(
    reader: &mut ProcessReader<'_>,
    requested: FsCredentials,
    baseline: FsCredentials,
    verify: bool,
    read: impl FnOnce(&mut ProcessReader<'_>, FsCredentials),
) {
    if requested == baseline {
        read(reader, baseline);
        return;
    }
    let guard = FsCredGuard::switch(requested);
    let active = if verify {
        current_fs_credentials()
    } else {
        requested
    };
    read(reader, active);
    drop(guard);
}

#[cfg(target_os = "linux")]
fn current_fs_credentials() -> FsCredentials {
    #[cfg(test)]
    TEST_QUERIES.with(|queries| queries.set(queries.get().saturating_add(1)));
    FsCredentials {
        uid: nix::unistd::setfsuid(nix::unistd::Uid::from_raw(u32::MAX)).as_raw(),
        gid: nix::unistd::setfsgid(nix::unistd::Gid::from_raw(u32::MAX)).as_raw(),
    }
}

#[cfg(not(target_os = "linux"))]
fn current_fs_credentials() -> FsCredentials {
    #[cfg(test)]
    TEST_QUERIES.with(|queries| queries.set(queries.get().saturating_add(1)));
    FsCredentials {
        uid: rustix::process::geteuid().as_raw(),
        gid: rustix::process::getegid().as_raw(),
    }
}

#[cfg(target_os = "linux")]
struct FsCredGuard {
    uid: nix::unistd::Uid,
    gid: nix::unistd::Gid,
}

#[cfg(target_os = "linux")]
impl FsCredGuard {
    fn switch(credentials: FsCredentials) -> Self {
        #[cfg(test)]
        TEST_SWITCHES.with(|switches| switches.set(switches.get().saturating_add(1)));
        let gid = nix::unistd::setfsgid(nix::unistd::Gid::from_raw(credentials.gid));
        let uid = nix::unistd::setfsuid(nix::unistd::Uid::from_raw(credentials.uid));
        Self { uid, gid }
    }
}

#[cfg(target_os = "linux")]
impl Drop for FsCredGuard {
    fn drop(&mut self) {
        nix::unistd::setfsuid(self.uid);
        nix::unistd::setfsgid(self.gid);
    }
}

#[cfg(not(target_os = "linux"))]
struct FsCredGuard;

#[cfg(not(target_os = "linux"))]
impl FsCredGuard {
    fn switch(_credentials: FsCredentials) -> Self {
        #[cfg(test)]
        TEST_SWITCHES.with(|switches| switches.set(switches.get().saturating_add(1)));
        Self
    }
}

#[cfg(test)]
fn reset_test_io(reads: impl IntoIterator<Item = IoRead>) {
    TEST_SWITCHES.with(|switches| switches.set(0));
    TEST_QUERIES.with(|queries| queries.set(0));
    TEST_READS.with(|script| {
        let mut script = script.borrow_mut();
        script.clear();
        script.extend(reads);
    });
}

#[cfg(test)]
fn test_io_counts() -> (usize, usize) {
    (
        TEST_SWITCHES.with(std::cell::Cell::get),
        TEST_QUERIES.with(std::cell::Cell::get),
    )
}

#[cfg(test)]
mod tests;
