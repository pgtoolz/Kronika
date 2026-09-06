//! Process rows and read errors.

use std::fmt;
use std::io;

use super::ParseError;

/// Linux time and page-size facts needed to convert process identity fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessFacts {
    /// Kernel boot time, unix microseconds.
    pub btime_usec: i64,
    /// Kernel scheduler ticks per second.
    pub clock_ticks_per_sec: i64,
    /// Kernel page size in bytes.
    pub page_size_bytes: i64,
}

/// Parsed data from `/proc/PID/stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcStat {
    /// PID from field 1.
    pub pid: i32,
    /// Process name from field 2, without surrounding parentheses.
    pub comm: String,
    /// Process state byte from field 3.
    pub state: u8,
    /// Parent PID.
    pub ppid: i32,
    /// Raw controlling terminal number.
    pub tty_nr: i32,
    /// Minor page faults.
    pub minflt: i64,
    /// Major page faults.
    pub majflt: i64,
    /// User CPU time, ticks.
    pub utime: i64,
    /// System CPU time, ticks.
    pub stime: i64,
    /// Scheduler priority.
    pub priority: i64,
    /// Nice value.
    pub nice: i64,
    /// Thread count.
    pub num_threads: i64,
    /// Process start time since boot, ticks.
    pub starttime_ticks: i64,
    /// Virtual memory size, bytes.
    pub vsize_bytes: i64,
    /// Resident set size, pages.
    pub rss_pages: i64,
    /// Exit signal.
    pub exit_signal: i64,
    /// Last/current CPU.
    pub processor: i64,
    /// Real-time priority.
    pub rt_priority: i64,
    /// Scheduler policy.
    pub policy: i64,
    /// Block I/O delay, ticks.
    pub delayacct_blkio_ticks: i64,
}

/// Parsed data from `/proc/PID/status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcStatus {
    /// Real UID.
    pub uid: u32,
    /// Effective UID.
    pub euid: u32,
    /// Real GID.
    pub gid: u32,
    /// Effective GID.
    pub egid: u32,
    /// `VmData`, kB.
    pub vm_data: i64,
    /// `VmStk`, kB.
    pub vm_stk: i64,
    /// `VmLib`, kB.
    pub vm_lib: i64,
    /// `VmSwap`, kB.
    pub vm_swap: i64,
    /// `VmLck`, kB.
    pub vm_lck: i64,
    /// `VmPTE`, kB.
    pub vm_pte: i64,
    /// `VmPeak`, kB.
    pub vm_peak: i64,
    /// `VmHWM`, kB.
    pub vm_hwm: i64,
    /// `Threads`.
    pub threads: u32,
    /// `FDSize`.
    pub fdsize: u32,
    /// Voluntary context switches.
    pub voluntary_ctxt_switches: i64,
    /// Nonvoluntary context switches.
    pub nonvoluntary_ctxt_switches: i64,
}

/// Parsed data from `/proc/PID/io`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcIo {
    /// Characters read, including page-cache hits.
    pub rchar: i64,
    /// Characters written, including page-cache writes.
    pub wchar: i64,
    /// Read syscalls.
    pub syscr: i64,
    /// Write syscalls.
    pub syscw: i64,
    /// Bytes really read from storage.
    pub read_bytes: i64,
    /// Bytes really written to storage.
    pub write_bytes: i64,
    /// Cancelled write bytes.
    pub cancelled_write_bytes: i64,
}

/// Process hot row before string interning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessHotRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Process ID.
    pub pid: i32,
    /// Process start timestamp, unix microseconds.
    pub starttime: i64,
    /// Parent PID.
    pub ppid: i32,
    /// Real UID.
    pub uid: u32,
    /// Effective UID.
    pub euid: u32,
    /// Real GID.
    pub gid: u32,
    /// Effective GID.
    pub egid: u32,
    /// Process state as ASCII byte.
    pub state: u8,
    /// Number of threads.
    pub num_threads: u32,
    /// Controlling terminal.
    pub tty: u16,
    /// Process name.
    pub comm: String,
    /// Command line with NUL separators converted to spaces.
    pub cmdline: Option<String>,
    /// User CPU time, ticks.
    pub utime: i64,
    /// System CPU time, ticks.
    pub stime: i64,
    /// Nice value.
    pub nice: i8,
    /// Scheduler priority.
    pub prio: i16,
    /// Real-time priority.
    pub rtprio: i16,
    /// Scheduler policy.
    pub policy: u8,
    /// Last/current CPU.
    pub curcpu: i32,
    /// Run-queue delay, ns.
    pub rundelay_ns: i64,
    /// Block I/O delay, ticks.
    pub blkdelay_ticks: i64,
    /// Voluntary context switches.
    pub nvcsw: i64,
    /// Nonvoluntary context switches.
    pub nivcsw: i64,
    /// Minor page faults.
    pub minflt: i64,
    /// Major page faults.
    pub majflt: i64,
    /// Virtual memory, kB.
    pub vmem_kb: i64,
    /// Resident memory, kB.
    pub rmem_kb: i64,
    /// Swap, kB.
    pub vswap_kb: i64,
    /// Optional process I/O counters.
    pub io: Option<ProcIo>,
    /// Exit signal.
    pub exit_signal: i32,
}

/// Extended `/proc/PID/status` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStatusRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Process ID.
    pub pid: i32,
    /// Process start timestamp, unix microseconds.
    pub starttime: i64,
    /// Parsed `/proc/PID/status` fields.
    pub status: ProcStatus,
}

/// PID to cgroup mapping row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCgroupRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Process ID.
    pub pid: i32,
    /// Process start timestamp, unix microseconds.
    pub starttime: i64,
    /// Normalized cgroup path.
    pub cgroup_path: String,
}

/// Result of reading one PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRead {
    /// Hot process row.
    pub hot: ProcessHotRow,
    /// Extended status row.
    pub status: ProcessStatusRow,
    /// Optional cgroup mapping.
    pub cgroup: Option<ProcessCgroupRow>,
}

/// Why one process could not be read.
#[derive(Debug)]
pub enum ProcessError {
    /// Process vanished before required files could be read.
    Gone(i32),
    /// Required process file could not be read.
    Read {
        /// Relative procfs path.
        path: String,
        /// I/O error.
        source: io::Error,
    },
    /// Required process file could not be parsed.
    Parse {
        /// Relative procfs path.
        path: String,
        /// Parse error.
        source: ParseError,
    },
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gone(pid) => write!(f, "process {pid} disappeared"),
            Self::Read { path, source } => write!(f, "{path}: {source}"),
            Self::Parse { path, source } => write!(f, "{path}: {}", source.0),
        }
    }
}

impl std::error::Error for ProcessError {}
