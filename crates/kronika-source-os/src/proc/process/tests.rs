use super::*;

fn stat_line(pid: i32, comm: &str) -> String {
    format!(
        "{pid} ({comm}) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 -5 16 17 190 204800 12 21 22 23 24 25 26 27 28 29 30 31 32 33 15 2 7 8 9 10 11 12 13 14 15"
    )
}

#[test]
fn process_reader_reuses_scratch_without_leaking_optional_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    for pid in [1, 2] {
        let process = dir.path().join(pid.to_string());
        std::fs::create_dir(&process).expect("create process");
        std::fs::write(
            process.join("stat"),
            stat_line(pid, &format!("worker-{pid}")),
        )
        .expect("write stat");
        std::fs::write(
            process.join("status"),
            "Uid:\t1000\t1001\t1002\t1003\nGid:\t2000\t2001\t2002\t2003\n",
        )
        .expect("write status");
    }
    std::fs::write(dir.path().join("1/cgroup"), "0::/workers/one\n").expect("write cgroup");
    std::fs::write(dir.path().join("1/cmdline"), b"worker\0--first\0").expect("write cmdline");
    std::fs::write(dir.path().join("1/comm"), "first-worker\n").expect("write comm");

    let fs = ProcFs::new(dir.path().to_path_buf());
    let facts = ProcessFacts {
        btime_usec: 1_700_000_000_000_000,
        clock_ticks_per_sec: 100,
        page_size_bytes: 4096,
    };
    let mut reader = ProcessReader::new(&fs);
    let cgroup_path = reader.cgroup_membership(1).and_then(parse_cgroup_path);
    let first = reader
        .read(1, facts, 7, cgroup_path)
        .expect("first process");
    let second = reader.read(2, facts, 7, None).expect("second process");

    assert_eq!(first.hot.comm, "first-worker");
    assert_eq!(first.hot.cmdline.as_deref(), Some("worker --first"));
    assert_eq!(
        first.cgroup.as_ref().map(|row| row.cgroup_path.as_str()),
        Some("/workers/one")
    );
    assert_eq!(second.hot.comm, "worker-2");
    assert_eq!(second.hot.cmdline, None);
    assert_eq!(second.cgroup, None);
}
