use super::{OsSources, ProcFs, SysFs, collect_core_metrics};
use std::path::Path;

const CPU: &str = "cpu 30 0 6 90\ncpu0 10 0 2 40\ncpu1 20 0 4 50\n";
const STAT: &str = "ctxt 12\nprocesses 8\nprocs_running 2\nprocs_blocked 1\nbtime 100\n";

fn write_core_files(root: &Path, stat: &str) {
    for (name, content) in [
        ("stat", stat),
        ("meminfo", "MemTotal: 4096 kB\nMemFree: 1024 kB\n"),
        ("loadavg", "1.25 0.50 0.25 2/20 123\n"),
        ("vmstat", "pgfault 42\npswpin 3\n"),
    ] {
        std::fs::write(root.join(name), content).expect("write core proc fixture");
    }
}

#[test]
fn core_metrics_collect_all_cpus_and_system_rows_with_tick_identity() {
    let dir = tempfile::tempdir().expect("proc root");
    write_core_files(dir.path(), &format!("{CPU}{STAT}"));
    let fs = ProcFs::new(dir.path().to_path_buf());
    let sys = SysFs::new(dir.path().join("sys"));
    let mut os = OsSources::default();

    collect_core_metrics(&fs, &sys, 4, 7, false, None, &mut os);

    assert_eq!(
        os.cpu
            .iter()
            .map(|row| (row.cpu_id, row.user))
            .collect::<Vec<_>>(),
        [(-1, 30), (0, 10), (1, 20)]
    );
    for row in &os.cpu {
        assert_eq!((row.ts.0, row.scope), (7, 4));
    }
    let stat = os.stat.expect("stat row");
    assert_eq!(
        (stat.ctxt, stat.processes, stat.btime.0),
        (12, 8, 100_000_000)
    );
    let memory = os.meminfo.expect("memory row");
    assert_eq!((memory.mem_total, memory.mem_free), (4096, Some(1024)));
    let load = os.loadavg.expect("load row");
    assert_eq!(load.load1.to_bits(), 1.25_f64.to_bits());
    assert_eq!((load.running, load.total), (2, 20));
    let paging = os.vmstat.expect("paging row");
    assert_eq!((paging.pgfault, paging.pswpin), (Some(42), Some(3)));
    for identity in [
        (stat.ts.0, stat.scope),
        (memory.ts.0, memory.scope),
        (load.ts.0, load.scope),
        (paging.ts.0, paging.scope),
    ] {
        assert_eq!(identity, (7, 4));
    }
}

#[test]
fn cpu_and_stat_parse_failures_do_not_block_each_other_or_later_files() {
    for (stat, cpu_rows, has_stat) in [
        (format!("cpu invalid\n{STAT}"), 0, true),
        (format!("{CPU}ctxt invalid\n"), 3, false),
    ] {
        let dir = tempfile::tempdir().expect("proc root");
        write_core_files(dir.path(), &stat);
        let fs = ProcFs::new(dir.path().to_path_buf());
        let sys = SysFs::new(dir.path().join("sys"));
        let mut os = OsSources::default();

        collect_core_metrics(&fs, &sys, 0, 9, false, None, &mut os);

        assert_eq!(os.cpu.len(), cpu_rows);
        assert_eq!(os.stat.is_some(), has_stat);
        assert_eq!(os.meminfo.expect("memory still collected").mem_total, 4096);
        assert_eq!(os.loadavg.expect("load still collected").total, 20);
        assert_eq!(os.vmstat.expect("paging still collected").pgfault, Some(42));
    }
}

#[test]
fn failed_required_files_preserve_prior_rows_while_other_files_advance() {
    for (name, malformed) in [
        ("stat", "cpu invalid\nctxt invalid\n"),
        ("meminfo", "MemTotal: invalid kB\n"),
        ("loadavg", "invalid 0 0 1/1\n"),
        ("vmstat", "pgfault invalid\n"),
    ] {
        for replacement in [None, Some(""), Some(malformed)] {
            let dir = tempfile::tempdir().expect("proc root");
            write_core_files(dir.path(), &format!("{CPU}{STAT}"));
            let fs = ProcFs::new(dir.path().to_path_buf());
            let sys = SysFs::new(dir.path().join("sys"));
            let mut os = OsSources::default();
            collect_core_metrics(&fs, &sys, 4, 7, false, None, &mut os);
            let prior = (os.cpu.clone(), os.stat, os.meminfo, os.loadavg, os.vmstat);
            if let Some(content) = replacement {
                std::fs::write(dir.path().join(name), content).expect("replace proc file");
            } else {
                std::fs::remove_file(dir.path().join(name)).expect("remove proc file");
            }

            collect_core_metrics(&fs, &sys, 0, 9, false, None, &mut os);

            match name {
                "stat" => assert_eq!((&os.cpu, os.stat), (&prior.0, prior.1)),
                "meminfo" => assert_eq!(os.meminfo, prior.2),
                "loadavg" => assert_eq!(os.loadavg, prior.3),
                "vmstat" => assert_eq!(os.vmstat, prior.4),
                _ => unreachable!("fixture names a required core file"),
            }
            for (source, ts, scope) in [
                ("stat", os.cpu[0].ts.0, os.cpu[0].scope),
                ("stat", os.stat.unwrap().ts.0, os.stat.unwrap().scope),
                (
                    "meminfo",
                    os.meminfo.unwrap().ts.0,
                    os.meminfo.unwrap().scope,
                ),
                (
                    "loadavg",
                    os.loadavg.unwrap().ts.0,
                    os.loadavg.unwrap().scope,
                ),
                ("vmstat", os.vmstat.unwrap().ts.0, os.vmstat.unwrap().scope),
            ] {
                let expected = if source == name { (7, 4) } else { (9, 0) };
                assert_eq!((ts, scope), expected, "{name}: {replacement:?}; {source}");
            }
        }
    }
}
