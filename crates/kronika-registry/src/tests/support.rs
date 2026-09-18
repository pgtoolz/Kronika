use crate::os_process::OsProcess;
use crate::{Section, StrId, Ts, VerifiedSection};

/// Shared roundtrip assertion for generated codecs.
pub(crate) fn assert_roundtrips<T>(rows: &[T])
where
    T: Section + PartialEq + std::fmt::Debug,
{
    let bytes = T::encode(rows).expect("encode");
    let decoded = T::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded.as_slice(), rows);
}

pub(crate) fn process_row(ts: i64, pid: i32, io: bool) -> OsProcess {
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
