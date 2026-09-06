//! Type `1_100_001`: hot per-process metrics from `/proc/PID/*`.

use crate::{Section, StrId, Ts};

/// Hot process identity, CPU, memory, and I/O counters.
///
/// Within one selected hour, `pid` alone identifies a process. `starttime`
/// remains an observed timestamp. `/proc/PID/io` fields are nullable because
/// Linux may deny them for foreign UIDs, `hidepid`, LSM policy, or PID namespace
/// boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_100_001,
    name = "os_process",
    semantics = snapshot_full,
    sort_key("pid", "ts"),
    identity("pid")
)]
pub struct OsProcess {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Process ID.
    #[column(l)]
    pub pid: i32,
    /// Process start timestamp, unix microseconds.
    #[column(l)]
    pub starttime: Ts,
    /// Parent process ID.
    #[column(l)]
    pub ppid: i32,
    /// Real user ID.
    #[column(l)]
    pub uid: u32,
    /// Effective user ID.
    #[column(l)]
    pub euid: u32,
    /// Real group ID.
    #[column(l)]
    pub gid: u32,
    /// Effective group ID.
    #[column(l)]
    pub egid: u32,
    /// Process state as an ASCII byte (`R`, `S`, `D`, `Z`, `T`, ...).
    #[column(l)]
    pub state: u8,
    /// Thread count.
    #[column(g, unit = count)]
    pub num_threads: u32,
    /// Controlling terminal.
    #[column(l)]
    pub tty: u16,
    /// Process name (`comm`) as a string dictionary reference.
    #[column(l)]
    pub comm: StrId,
    /// Command line as a string dictionary reference; `None` when unavailable or empty.
    #[column(l)]
    pub cmdline: Option<StrId>,
    /// User CPU time.
    #[column(c, unit = jiffies)]
    pub utime: i64,
    /// System CPU time.
    #[column(c, unit = jiffies)]
    pub stime: i64,
    /// Nice value.
    #[column(l)]
    pub nice: i8,
    /// Scheduler priority.
    #[column(l)]
    pub prio: i16,
    /// Real-time priority.
    #[column(l)]
    pub rtprio: i16,
    /// Scheduler policy.
    #[column(l)]
    pub policy: u8,
    /// Last/current CPU.
    #[column(g, unit = count)]
    pub curcpu: i32,
    /// Run-queue delay.
    #[column(c, unit = nanoseconds)]
    pub rundelay_ns: i64,
    /// Block I/O delay.
    #[column(c, unit = jiffies)]
    pub blkdelay_ticks: i64,
    /// Voluntary context switches.
    #[column(c, unit = count)]
    pub nvcsw: i64,
    /// Nonvoluntary context switches.
    #[column(c, unit = count)]
    pub nivcsw: i64,
    /// Minor page faults.
    #[column(c, unit = pages)]
    pub minflt: i64,
    /// Major page faults.
    #[column(c, unit = pages)]
    pub majflt: i64,
    /// Virtual memory size.
    #[column(g, unit = kib)]
    pub vmem_kb: i64,
    /// Resident memory size.
    #[column(g, unit = kib)]
    pub rmem_kb: i64,
    /// Swap used by process.
    #[column(g, unit = kib)]
    pub vswap_kb: i64,
    /// Read syscall count from `/proc/PID/io`.
    #[column(c, unit = count)]
    pub syscr: Option<i64>,
    /// Write syscall count from `/proc/PID/io`.
    #[column(c, unit = count)]
    pub syscw: Option<i64>,
    /// Characters read, including page-cache hits.
    #[column(c, unit = bytes)]
    pub rchar: Option<i64>,
    /// Characters written, including page-cache writes.
    #[column(c, unit = bytes)]
    pub wchar: Option<i64>,
    /// Bytes really read from storage.
    #[column(c, unit = bytes)]
    pub read_bytes: Option<i64>,
    /// Bytes really written to storage.
    #[column(c, unit = bytes)]
    pub write_bytes: Option<i64>,
    /// Cancelled write bytes.
    #[column(c, unit = bytes)]
    pub cancelled_write_bytes: Option<i64>,
    /// Signal sent to the parent when the process exits.
    #[column(l)]
    pub exit_signal: i32,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
pub(crate) fn test_row(ts: i64, pid: i32, io: bool) -> OsProcess {
    OsProcess {
        ts: Ts(ts),
        pid,
        starttime: Ts(1_700_000_000_000_000 + i64::from(pid)),
        ppid: 1,
        uid: 1000,
        euid: 1000,
        gid: 1000,
        egid: 1000,
        state: b'S',
        num_threads: 3,
        tty: 0,
        comm: StrId(10),
        cmdline: Some(StrId(11)),
        utime: 100,
        stime: 50,
        nice: 0,
        prio: 20,
        rtprio: 0,
        policy: 0,
        curcpu: 2,
        rundelay_ns: 1234,
        blkdelay_ticks: 5,
        nvcsw: 9,
        nivcsw: 1,
        minflt: 77,
        majflt: 3,
        vmem_kb: 2048,
        rmem_kb: 1024,
        vswap_kb: 0,
        syscr: io.then_some(1),
        syscw: io.then_some(2),
        rchar: io.then_some(3),
        wchar: io.then_some(4),
        read_bytes: io.then_some(5),
        write_bytes: io.then_some(6),
        cancelled_write_bytes: io.then_some(7),
        exit_signal: 17,
        scope: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{OsProcess, test_row};
    use crate::{Section, Unit, VerifiedSection, contract::lint};

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsProcess::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsProcess::CONTRACT;
        assert_eq!(c.type_id.get(), 1_100_001);
        assert_eq!(c.sort_key, ["pid", "ts"]);
        assert_eq!(c.identity, ["pid"]);
        for name in ["rchar", "wchar", "read_bytes", "write_bytes"] {
            assert_eq!(
                c.column(name).and_then(|column| column.unit),
                Some(Unit::Bytes)
            );
        }
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[test_row(1, 10, true), test_row(2, 11, false)]);
    }

    #[test]
    fn io_nulls_survive() {
        let bytes = OsProcess::encode(&[test_row(1, 10, false)]).expect("encode");
        let decoded = OsProcess::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(decoded[0].syscr, None);
        assert_eq!(decoded[0].read_bytes, None);
        assert_eq!(decoded[0].cancelled_write_bytes, None);
    }
}
