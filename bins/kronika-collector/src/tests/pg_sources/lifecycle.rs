//! Discovery, connection generations, cached settings, and collection across failures.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kronika_source_pg::Pool;
use kronika_source_pg::databases::Database;
use kronika_source_pg::extension::ExtensionSchema;
use kronika_source_pg::settings::SettingsRow;
use kronika_source_pg::statements::{StatementsCapability, StatementsVersion};

use super::super::capabilities::{DatabaseCapabilities, capabilities_match_generation};
use super::super::discovery::{DISCOVERY_INTERVAL, discovery_due};
use super::super::probe::{GenerationProbe, SERVER_PROBE_SQL};
use super::super::settings::{
    CachedSettings, cached_settings_for_generation, settings_equal_ignoring_ts,
};
use super::super::{PgBatch, PgObservation, PgSources, QueryOutcome};
use super::protocol::serve_enumeration_denied;
use kronika_source_pg::query::BatchWrite;

#[tokio::test]
async fn metrics_connection_failure_never_attempts_an_ignored_legacy_target() {
    const CHILD: &str = "KRONIKA_TEST_SINGLE_DSN_METRICS_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let config = crate::config::Config::from_env().expect("normalized PostgreSQL config");
        assert!(!config.mode.collect_os());
        let mut sources = PgSources::open(&config).expect("selected metrics target");
        let mut observations = Vec::new();
        assert!(
            sources
                .read_probe(true, &mut |observation| observations.push(observation))
                .await
                .is_none()
        );
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
            "pg_sources::tests::lifecycle::metrics_connection_failure_never_attempts_an_ignored_legacy_target",
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

#[test]
fn discovery_runs_immediately_then_every_five_minutes() {
    let now = Instant::now();
    assert!(discovery_due(None, now, false));
    assert!(!discovery_due(
        Some(now),
        now + DISCOVERY_INTERVAL.saturating_sub(Duration::from_millis(1)),
        false
    ));
    assert!(discovery_due(Some(now), now + DISCOVERY_INTERVAL, false));
}

#[test]
fn a_forced_tick_refreshes_discovery_before_the_deadline() {
    let now = Instant::now();
    assert!(discovery_due(Some(now), now, true));
}

#[test]
fn settings_cache_is_scoped_to_one_connection_generation() {
    let cached = Some(CachedSettings {
        generation: 7,
        rows: Arc::from([]),
    });
    assert!(cached_settings_for_generation(cached.as_ref(), 7).is_some());
    assert!(cached_settings_for_generation(cached.as_ref(), 8).is_none());
}

#[test]
fn settings_equality_ignores_only_the_collection_timestamp() {
    let original = SettingsRow {
        ts: 1,
        datid: 16_384,
        datname: "app".to_owned(),
        usesysid: 16_385,
        usename: "monitor".to_owned(),
        name: "work_mem".to_owned(),
        setting: "4096".to_owned(),
        unit: Some("kB".to_owned()),
        source: "configuration file".to_owned(),
        sourcefile: Some("/etc/postgresql/postgresql.conf".to_owned()),
        sourceline: Some(42),
        pending_restart: false,
        context: "user".to_owned(),
        vartype: "integer".to_owned(),
        boot_val: Some("4096".to_owned()),
        reset_val: Some("4096".to_owned()),
    };
    let mut refreshed = original.clone();
    refreshed.ts = 2;
    assert!(settings_equal_ignoring_ts(
        std::slice::from_ref(&original),
        std::slice::from_ref(&refreshed)
    ));

    refreshed.setting = "8192".to_owned();
    assert!(!settings_equal_ignoring_ts(
        std::slice::from_ref(&original),
        std::slice::from_ref(&refreshed)
    ));
}

#[test]
fn extension_inventory_cache_is_scoped_to_one_connection_generation() {
    assert!(capabilities_match_generation(Some(7), Some(7)));
    assert!(!capabilities_match_generation(None, Some(7)));
    assert!(!capabilities_match_generation(Some(7), Some(8)));
    assert!(!capabilities_match_generation(Some(7), None));
}

#[test]
fn visibility_refresh_on_the_same_connection_keeps_generation_scoped_state() {
    let mut sources = PgSources::default();
    let now = Instant::now();
    sources.settings = Some(CachedSettings {
        generation: 7,
        rows: Arc::from([]),
    });
    sources
        .capabilities
        .insert("app".to_owned(), DatabaseCapabilities::default());
    sources.last_discovery = Some(now);
    sources.probe = Some(GenerationProbe {
        generation: 7,
        major: 18,
        datid: 16_384,
        database: "app".to_owned(),
        usesysid: 16_385,
        user: "monitor".to_owned(),
        full_visibility: false,
    });

    sources.update_probe_cache(
        GenerationProbe {
            generation: 7,
            major: 18,
            datid: 16_384,
            database: "app".to_owned(),
            usesysid: 16_385,
            user: "monitor".to_owned(),
            full_visibility: true,
        },
        true,
    );

    assert!(sources.settings.is_some());
    assert!(sources.capabilities.contains_key("app"));
    assert_eq!(sources.last_discovery, Some(now));
    assert!(
        sources
            .probe
            .as_ref()
            .is_some_and(|probe| probe.full_visibility)
    );
}

#[test]
fn a_new_primary_generation_discards_secondary_pools() {
    let mut sources = PgSources::default();
    sources.databases.insert(
        "other".to_owned(),
        Pool::new("host=127.0.0.1 dbname=other").expect("the DSN parses"),
    );

    sources.update_probe_cache(
        GenerationProbe {
            generation: 8,
            major: 18,
            datid: 16_384,
            database: "app".to_owned(),
            usesysid: 16_385,
            user: "monitor".to_owned(),
            full_visibility: true,
        },
        false,
    );

    assert!(sources.databases.is_empty());
}

#[tokio::test]
async fn losing_the_primary_during_extension_collection_ends_the_cycle() {
    let mut sources = PgSources {
        server: Some(Pool::new("host=127.0.0.1 dbname=app").expect("the primary DSN parses")),
        server_database: Some("app".to_owned()),
        ..PgSources::default()
    };
    sources.databases.insert(
        "other".to_owned(),
        Pool::new("host=127.0.0.1 dbname=other").expect("the DSN parses"),
    );
    sources.discovered.push(Database {
        oid: 7,
        name: "other".to_owned(),
        is_current: false,
    });
    sources.capabilities.insert(
        "app".to_owned(),
        DatabaseCapabilities {
            generation: 7,
            statements: Some(StatementsCapability {
                version: StatementsVersion::V6,
                schema: ExtensionSchema::new("monitoring"),
            }),
            ..DatabaseCapabilities::default()
        },
    );
    let probe = GenerationProbe {
        generation: 7,
        major: 18,
        datid: 16_384,
        database: "app".to_owned(),
        usesysid: 16_385,
        user: "monitor".to_owned(),
        full_visibility: true,
    };
    let mut admitted = false;
    let continued = sources
        .collect_extensions(&probe, &mut |_observation| {}, None, &mut |_, _| {
            admitted = true;
            Ok::<_, ()>(BatchWrite::default())
        })
        .await
        .expect("no collector sink error");

    assert!(!continued);
    assert!(!admitted);
    assert!(sources.databases.is_empty());
    assert!(sources.discovered.is_empty());
    assert!(sources.capabilities.is_empty());
}

#[test]
fn server_and_extension_visibility_use_immediately_usable_role_privileges() {
    assert!(SERVER_PROBE_SQL.contains("pg_has_role('pg_read_all_stats', 'USAGE')"));
}

#[test]
fn server_probe_reads_metric_session_identity_with_session_user() {
    assert!(SERVER_PROBE_SQL.contains("d.oid::text"));
    assert!(SERVER_PROBE_SQL.contains("d.datname::text"));
    assert!(SERVER_PROBE_SQL.contains("r.oid::text"));
    assert!(SERVER_PROBE_SQL.contains("r.rolname::text"));
    assert!(SERVER_PROBE_SQL.contains("r.rolname = session_user"));
    assert!(!SERVER_PROBE_SQL.contains("current_user"));
}

#[tokio::test]
async fn enumeration_permission_error_still_admits_activity_and_locks() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind monitoring fixture");
    let port = listener.local_addr().expect("fixture address").port();
    let server = std::thread::spawn(move || serve_enumeration_denied(&listener));
    let mut sources = PgSources {
        server: Some(
            Pool::new(&format!(
                "host=127.0.0.1 port={port} user=monitor dbname=metrics sslmode=disable"
            ))
            .expect("valid fixture DSN"),
        ),
        ..PgSources::default()
    };
    let mut observations = Vec::new();
    let mut admitted = Vec::new();
    sources
        .collect(
            &crate::scheduler::DueSet::for_test(vec![crate::scheduler::SourceKind::PgActivity]),
            &mut |event| observations.push(event),
            |batch, _settings| {
                match batch {
                    PgBatch::Activity(_, rows) => admitted.push(("activity", rows.len())),
                    PgBatch::Locks(_, rows) => admitted.push(("locks", rows.len())),
                    _ => {}
                }
                Ok::<_, ()>(BatchWrite::default())
            },
        )
        .await
        .expect("independent SQL collection continues");
    assert!(admitted.contains(&("activity", 1)));
    assert!(admitted.contains(&("locks", 1)));
    assert_eq!(sources.server.as_ref().and_then(Pool::generation), Some(1));
    assert_eq!(
        sources.discovered,
        vec![Database {
            oid: 1,
            name: "metrics".to_owned(),
            is_current: true
        }]
    );
    assert!(observations.iter().any(|event| matches!(event,
        PgObservation::Query(query) if query.query_name == "databases" && query.outcome == QueryOutcome::Error
    )));
    for query_name in [
        "pg_settings",
        "pg_stat_database",
        "pg_stat_statements",
        "pg_store_plans",
    ] {
        assert!(
            !observations.iter().any(|event| matches!(event,
                PgObservation::Query(query) if query.query_name == query_name
            )),
            "fast activity tick must not run {query_name}"
        );
    }
    sources.close_connections();
    tokio::task::yield_now().await;
    server.join().expect("protocol fixture exits");
}

#[tokio::test]
async fn instance_tick_excludes_activity_locks_and_progress() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind monitoring fixture");
    let port = listener.local_addr().expect("fixture address").port();
    let server = std::thread::spawn(move || serve_enumeration_denied(&listener));
    let mut sources = PgSources {
        server: Some(
            Pool::new(&format!(
                "host=127.0.0.1 port={port} user=monitor dbname=metrics sslmode=disable"
            ))
            .expect("valid fixture DSN"),
        ),
        ..PgSources::default()
    };
    let mut queries = Vec::new();
    sources
        .collect(
            &crate::scheduler::DueSet::for_test(vec![crate::scheduler::SourceKind::PgInstance]),
            &mut |event| {
                if let PgObservation::Query(query) = event {
                    queries.push(query.query_name);
                }
            },
            |_batch, _settings| Ok::<_, ()>(BatchWrite::default()),
        )
        .await
        .expect("SQL errors leave independent sources usable");
    sources.close_connections();
    tokio::task::yield_now().await;
    server.join().expect("protocol fixture exits");
    assert!(
        queries.contains(&"pg_stat_database"),
        "server counters were attempted"
    );
    for query in ["pg_stat_activity", "pg_locks", "pg_stat_progress_vacuum"] {
        assert!(
            !queries.contains(&query),
            "{query} belongs to the fast group: {queries:?}"
        );
    }
}
