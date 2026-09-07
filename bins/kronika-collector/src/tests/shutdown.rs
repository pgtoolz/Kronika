use std::future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kronika_format::{PartMeta, build_part};
use kronika_layout::{DataRoot, LayoutLimits, SegmentId};
use kronika_writer::{Journal, JournalConfig};

use crate::config::Config;
use crate::scheduler::Intervals;

use super::super::{complete_or_shutdown, initialize_collector};

struct PendingWork(Arc<AtomicBool>);

impl Drop for PendingWork {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn shutdown_drops_in_progress_collection() {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = PendingWork(Arc::clone(&dropped));
    let work = async move {
        let _guard = guard;
        future::pending::<()>().await;
    };

    assert!(
        complete_or_shutdown(work, future::ready(()))
            .await
            .is_none()
    );
    assert!(dropped.load(Ordering::Relaxed));
}

fn config(storage_dir: &Path) -> Config {
    Config {
        mode: crate::config::CollectorMode::Local,
        storage_dir: storage_dir.to_owned(),
        tick_secs: 1,
        intervals: Intervals::default(),
        segment_max_bytes: 64 * 1024 * 1024,
        segment_max_age_secs: 900,
        journal_max_bytes: 64 * 1024 * 1024,
        retention: None,
        pg_dsns: Vec::new(),
        postgres_effective_cpus: None,
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
    }
}

fn recovery_candidate(storage_dir: &Path) -> Vec<u8> {
    std::fs::create_dir_all(storage_dir).expect("create data root");
    let root = DataRoot::open(storage_dir).expect("open data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let part = build_part(
        &[],
        PartMeta {
            min_ts: i64::MAX,
            max_ts: i64::MIN,
        },
    );
    journal
        .append(SegmentId::new(100).expect("valid segment id"), &part)
        .expect("append recovery candidate");
    drop(journal);
    drop(owner);
    std::fs::read(storage_dir.join("active.wal")).expect("read recovery candidate")
}

#[test]
fn invalid_connections_stop_before_storage_recovery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let valid = "host=db.example user=monitor".to_owned();
    let invalid = "host='unterminated password=RAW_SECRET dbname=PRIVATE_DATABASE".to_owned();

    for (variable, storage_dir) in [
        ("KRONIKA_PG_DSNS", dir.path().join("postgresql")),
        ("KRONIKA_PGBOUNCER_DSNS", dir.path().join("pgbouncer")),
    ] {
        let mut config = config(&storage_dir);
        let wal_before = recovery_candidate(&storage_dir);
        match variable {
            "KRONIKA_PG_DSNS" => {
                config.pg_dsns = vec![valid.clone(), invalid.clone()];
            }
            "KRONIKA_PGBOUNCER_DSNS" => {
                config.pgbouncer_dsns = vec![valid.clone(), invalid.clone()];
            }
            _ => unreachable!(),
        }

        let error = initialize_collector(&config)
            .expect_err("an invalid connection must stop collector initialization");
        let message = format!("{error:#}");

        assert!(message.contains(&format!("{variable}[1]")));
        for secret in [&invalid, "RAW_SECRET", "PRIVATE_DATABASE"] {
            assert!(!message.contains(secret));
        }
        assert_eq!(
            std::fs::read(storage_dir.join("active.wal")).expect("read untouched journal"),
            wal_before
        );
        assert!(!storage_dir.join("1970/01/01/100.zms").exists());
    }
}

#[test]
fn postgres_ca_is_required_only_for_postgres_targets() {
    const CHILD_ENV: &str = "KRONIKA_TEST_PG_CA_STARTUP_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = std::process::Command::new(
            std::env::current_exe().expect("locate collector test binary"),
        )
        .args([
            "--exact",
            "tests::shutdown::postgres_ca_is_required_only_for_postgres_targets",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .env(
            "KRONIKA_PG_SSL_ROOT_CERT",
            dir.path().join("missing-ca.pem"),
        )
        .output()
        .expect("run isolated CA startup child");
        assert!(
            output.status.success(),
            "isolated CA startup failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let dsn = "host=127.0.0.1 port=6432 user=monitor sslmode=disable".to_owned();
    let mut pgbouncer = config(&dir.path().join("pgbouncer"));
    pgbouncer.pgbouncer_dsns = vec![dsn.clone()];
    drop(initialize_collector(&pgbouncer).expect("PgBouncer does not read PostgreSQL CA"));
    assert!(pgbouncer.storage_dir.join("active.wal").exists());

    let mut postgres = config(&dir.path().join("postgres"));
    postgres.pg_dsns = vec![dsn];
    let wal_before = recovery_candidate(&postgres.storage_dir);
    let error = initialize_collector(&postgres).expect_err("PostgreSQL requires a valid CA");
    assert!(
        format!("{error:#}")
            .contains("KRONIKA_PG_SSL_ROOT_CERT must name a readable, valid PEM CA bundle")
    );
    assert_eq!(
        std::fs::read(postgres.storage_dir.join("active.wal")).expect("read untouched journal"),
        wal_before,
    );
}
