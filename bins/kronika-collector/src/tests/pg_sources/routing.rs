use super::super::{PgObservation, collection_selection, open};
use crate::scheduler::{DueSet, SourceKind};
use kronika_source_pg::query::BatchWrite;
use std::net::TcpListener;

#[tokio::test]
async fn metrics_connection_failure_never_attempts_an_ignored_legacy_target() {
    const CHILD: &str = "KRONIKA_TEST_SINGLE_DSN_METRICS_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let config = crate::config::Config::from_env().expect("normalized PostgreSQL config");
        assert!(!config.mode.collect_os());
        let mut sources = open(&config).expect("selected metrics target");
        let mut observations = Vec::new();
        sources
            .collect(
                &collection_selection(&DueSet::for_test(vec![SourceKind::PgActivity])),
                &mut |observation| observations.push(observation),
                |_, _| -> Result<BatchWrite, ()> {
                    panic!("failed connection must not admit rows")
                },
            )
            .await
            .expect("connection failure skips acquisition");
        assert!(matches!(
            observations.as_slice(),
            [PgObservation::Connection(_)]
        ));
        return;
    }

    let directory = tempfile::tempdir().expect("metrics routing fixture");
    let selected = TcpListener::bind(("127.0.0.1", 0)).expect("selected target");
    let selected_port = selected.local_addr().expect("selected address").port();
    let rejected = std::thread::spawn(move || {
        let (stream, _) = selected
            .accept()
            .expect("selected target receives connection");
        stream
            .shutdown(std::net::Shutdown::Both)
            .expect("reject selected connection");
    });
    let ignored = TcpListener::bind(("127.0.0.1", 0)).expect("ignored target");
    ignored
        .set_nonblocking(true)
        .expect("check without waiting");
    let legacy = format!(
        "host=127.0.0.1 port={selected_port} user=monitor sslmode=disable;host=127.0.0.1 port={} user=monitor sslmode=disable;;host='unterminated",
        ignored.local_addr().expect("ignored address").port()
    );
    let mut command = std::process::Command::new(std::env::current_exe().expect("test binary"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("KRONIKA_") {
            command.env_remove(name);
        }
    }
    let output = command
        .args([
            "--exact",
            "pg_sources::tests::routing::metrics_connection_failure_never_attempts_an_ignored_legacy_target",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("KRONIKA_STORAGE_DIR", directory.path())
        .env("KRONIKA_COLLECTOR_MODE", "postgresql")
        .env("KRONIKA_PG_DSNS", legacy)
        .output()
        .expect("isolated metrics routing check");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    rejected
        .join()
        .expect("selected target rejected one connection");
    assert!(
        matches!(ignored.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "metrics never fall back to the ignored target"
    );
}
