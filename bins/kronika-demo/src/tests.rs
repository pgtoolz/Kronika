use super::{collector_log_to_stderr, resolve_storage_dir};
use std::path::{Path, PathBuf};

#[test]
fn collector_log_destination_is_explicit() {
    assert!(!collector_log_to_stderr(None).expect("default file"));
    assert!(!collector_log_to_stderr(Some("file")).expect("file"));
    assert!(collector_log_to_stderr(Some("stderr")).expect("stderr"));
    assert!(collector_log_to_stderr(Some("stdout")).is_err());
}

#[test]
fn storage_directory_is_explicit_or_belongs_to_the_demo_run() {
    let root = Path::new("/demo");
    assert_eq!(resolve_storage_dir(root, None), root.join("segments"));
    assert_eq!(
        resolve_storage_dir(root, Some(PathBuf::from("/storage"))),
        PathBuf::from("/storage")
    );
}

#[test]
fn healthcheck_does_not_probe_a_database_named_after_the_monitor_role() {
    let healthcheck = include_str!("../../../scripts/demo-healthcheck.sh");
    let readiness = healthcheck
        .lines()
        .find(|line| line.contains("pg_isready"))
        .expect("PostgreSQL readiness command");
    assert!(readiness.contains("--dbname=postgres"));
}

#[test]
fn the_demo_entrypoint_enables_system_work_outside_collector_storage() {
    let entrypoint = include_str!("../../../scripts/demo-entrypoint.sh");
    assert!(entrypoint.contains(
        "KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED=\"${KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED-true}\""
    ));
    let directory = entrypoint
        .lines()
        .find(|line| line.contains("export KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR="))
        .expect("system workload directory export");
    assert!(directory.contains("$DEMO_ROOT/system-activity"));
    assert!(!directory.contains("$STORAGE_DIR"));
}

#[test]
fn compose_passes_every_system_workload_control_without_replacing_blanks() {
    let compose = include_str!("../../../compose.demo.yml");
    for (key, default) in [
        ("KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED", "true"),
        (
            "KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR",
            "/var/lib/kronika/data/system-activity",
        ),
        ("KRONIKA_DEMO_SYSTEM_CPU_PERCENT", "12"),
        ("KRONIKA_DEMO_SYSTEM_MEMORY_MIB", "32"),
        ("KRONIKA_DEMO_SYSTEM_FILE_MIB", "8"),
        ("KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S", "32"),
        ("KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S", "32"),
        ("KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S", "5"),
    ] {
        let contract = format!("${{{key}-{default}}}");
        assert!(
            compose.contains(&contract),
            "{key} does not preserve explicit blank values"
        );
    }
}
