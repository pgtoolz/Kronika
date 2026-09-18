#[cfg(target_os = "linux")]
use super::collect_os_instance_facts;
use super::parse_btime;

#[test]
fn explicit_proc_root_supplies_the_recorded_identity() {
    let root = tempfile::tempdir().expect("proc fixture");
    std::fs::create_dir_all(root.path().join("sys/kernel/random")).expect("kernel paths");
    for (path, value) in [
        ("stat", "cpu 1 2 3 4\nbtime 1700000000\n"),
        ("sys/kernel/hostname", "fixture-node\n"),
        ("sys/kernel/osrelease", "fixture-kernel\n"),
        ("sys/kernel/random/boot_id", "fixture-boot\n"),
    ] {
        std::fs::write(root.path().join(path), value).expect("proc file");
    }
    let fs = crate::ProcFs::new(root.path().to_owned());
    let facts = super::collect_os_instance_facts_from(&fs).expect("fixture identity");
    assert_eq!(facts.hostname, "fixture-node");
    assert_eq!(facts.kernel_version, "fixture-kernel");
    assert_eq!(facts.boot_id, "fixture-boot");
    assert_eq!(facts.btime, 1_700_000_000_000_000);
    assert!(facts.clock_ticks_per_sec > 0);
    assert!(facts.page_size_bytes > 0);
}

#[test]
fn parse_btime_finds_the_line_between_others() {
    let stat = "cpu  1 2 3 4\nintr 5\nctxt 6\nbtime 1700000000\nprocesses 7\n";
    assert_eq!(parse_btime(stat), Some(1_700_000_000_000_000));
}

#[test]
fn parse_btime_rejects_missing_or_garbled_lines() {
    assert_eq!(parse_btime("cpu 1 2 3\nprocesses 7\n"), None);
    assert_eq!(parse_btime("btime not-a-number\n"), None);
    assert_eq!(parse_btime("btimes 1700000000\n"), None);
    assert_eq!(parse_btime(""), None);
}

#[test]
fn parse_btime_rejects_values_that_overflow_microseconds() {
    assert_eq!(parse_btime("btime 9223372036854776\n"), None);
    assert_eq!(
        parse_btime("btime 9223372036854\n"),
        Some(9_223_372_036_854_000_000)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn collect_reads_the_live_host() {
    let facts = collect_os_instance_facts().expect("running on Linux with /proc");
    assert!(!facts.hostname.is_empty());
    assert!(!facts.kernel_version.is_empty());
    assert_eq!(
        facts.boot_id.len(),
        36,
        "boot_id is a UUID: {}",
        facts.boot_id
    );
    assert!(facts.btime > 0);
    assert!(facts.clock_ticks_per_sec > 0);
    assert!(facts.page_size_bytes >= 4096);
}
