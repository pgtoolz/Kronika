use super::{SystemActivity, SystemActivityConfig};
use crate::system_activity::scratch::SCRATCH_FILE_NAME;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn config(root: &std::path::Path) -> SystemActivityConfig {
    let storage_directory = root.join("segments");
    std::fs::create_dir_all(&storage_directory).unwrap();
    SystemActivityConfig {
        directory: root.join("system-activity"),
        storage_directory,
        cpu_percent: 1,
        memory_mib: 8,
        file_mib: 1,
        disk_kib_per_s: 4,
        network_kib_per_s: 4,
        flush_interval_s: 1,
    }
}

#[test]
fn immediate_shutdown_joins_named_workers_and_removes_scratch() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let scratch = config.directory.join(SCRATCH_FILE_NAME);
    let stop = Arc::new(AtomicBool::new(false));
    let activity = SystemActivity::start(&config, Arc::clone(&stop));
    let mut names: Vec<&str> = activity.workers.iter().map(|worker| worker.name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "krn-demo-cpu",
            "krn-demo-disk",
            "krn-demo-loop",
            "krn-demo-memory"
        ]
    );
    assert!(scratch.is_file());

    activity.stop();

    assert!(stop.load(Ordering::SeqCst));
    assert!(!scratch.exists());
}

#[test]
fn dropping_the_owner_also_cleans_the_scratch_file() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let scratch = config.directory.join(SCRATCH_FILE_NAME);
    let activity = SystemActivity::start(&config, Arc::new(AtomicBool::new(false)));
    assert!(scratch.is_file());

    drop(activity);

    assert!(!scratch.exists());
}
