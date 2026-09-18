use super::*;

fn stat_line(comm: &str) -> String {
    format!(
        "123 ({comm}) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 -5 16 17 190 204800 12 21 22 23 24 25 26 27 28 29 30 31 32 33 15 2 7 8 9 10 11 12 13 14 15"
    )
}

#[test]
fn stat_comm_uses_the_last_parenthesis() {
    let row = parse_stat(&stat_line("worker (bg) 1")).expect("stat");
    assert_eq!(row.comm, "worker (bg) 1");
    assert_eq!(row.pid, 123);
    assert_eq!(row.state, b'S');
    assert_eq!(row.ppid, 1);
    assert_eq!(row.minflt, 7);
    assert_eq!(row.majflt, 9);
    assert_eq!(row.utime, 11);
    assert_eq!(row.stime, 12);
    assert_eq!(row.nice, -5);
    assert_eq!(row.starttime_ticks, 190);
    assert_eq!(row.exit_signal, 15);
    assert_eq!(row.processor, 2);
    assert_eq!(row.rt_priority, 7);
    assert_eq!(row.policy, 8);
    assert_eq!(row.delayacct_blkio_ticks, 9);
}

#[test]
fn status_parses_identity_memory_and_switches() {
    let status = parse_status(
        "Uid:\t1000\t1001\t1002\t1003\n\
             Gid:\t2000\t2001\t2002\t2003\n\
             VmData:\t10 kB\nVmStk:\t11 kB\nVmLib:\t12 kB\nVmSwap:\t13 kB\n\
             VmLck:\t14 kB\nVmPTE:\t15 kB\nVmPeak:\t16 kB\nVmHWM:\t17 kB\n\
             Threads:\t3\nFDSize:\t64\n\
             voluntary_ctxt_switches:\t20\nnonvoluntary_ctxt_switches:\t21\n",
    )
    .expect("status");
    assert_eq!((status.uid, status.euid), (1000, 1001));
    assert_eq!((status.gid, status.egid), (2000, 2001));
    assert_eq!(status.vm_swap, 13);
    assert_eq!(status.vm_pte, 15);
    assert_eq!(status.vm_hwm, 17);
    assert_eq!(status.threads, 3);
    assert_eq!(status.fdsize, 64);
    assert_eq!(status.nonvoluntary_ctxt_switches, 21);
}

#[test]
fn io_parser_keeps_all_seven_fields() {
    let io = parse_io(
        "rchar: 1\nwchar: 2\nsyscr: 3\nsyscw: 4\nread_bytes: 5\n\
             write_bytes: 6\ncancelled_write_bytes: 7\n",
    );
    assert_eq!(
        io,
        ProcIo {
            rchar: 1,
            wchar: 2,
            syscr: 3,
            syscw: 4,
            read_bytes: 5,
            write_bytes: 6,
            cancelled_write_bytes: 7,
        }
    );
}

#[test]
fn cgroup_path_accepts_only_a_single_valid_unified_membership() {
    for content in [
        "0::/kubepods/pod-a/container\n",
        "1:name=systemd:/x\n4:pids:/docker/abc\n0::/kubepods/pod-a/container\n",
    ] {
        assert_eq!(
            parse_cgroup_path(content),
            Some("/kubepods/pod-a/container".to_owned())
        );
    }
    for content in [
        "1:name=systemd:/x\n4:pids:/docker/abc\n",
        "7::/workload\n",
        "0::/workload\n0::/workload\n",
        "0::/workload\n0::/other\n0::/workload\n",
        "0::relative\n",
        "0::/workload/../other\n",
        "0::\n",
    ] {
        assert_eq!(parse_cgroup_path(content), None, "{content}");
    }
    assert_eq!(parse_cgroup_path("0::/\n"), Some("/".to_owned()));
}

#[test]
fn starttime_uses_boot_time_and_hz() {
    let facts = ProcessFacts {
        btime_usec: 1_700_000_000_000_000,
        clock_ticks_per_sec: 250,
        page_size_bytes: 8192,
    };
    assert_eq!(process_starttime_usec(facts, 500), 1_700_000_002_000_000);
    assert_eq!(rss_kb(2, facts.page_size_bytes), 16);
}

#[test]
fn cmdline_trims_raw_boundaries_before_replacing_separators() {
    assert_eq!(
        normalize_cmdline(" \tpostgres\0-D\0/data\0\n"),
        Some("postgres -D /data".to_owned())
    );
    assert_eq!(normalize_cmdline("\0 \t\n\0"), None);
}
