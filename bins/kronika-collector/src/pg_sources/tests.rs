use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kronika_source_pg::Pool;
use kronika_source_pg::databases::Database;
use kronika_source_pg::extension::{ExtensionSchema, InventoryEntry};
use kronika_source_pg::query::BatchError;
use kronika_source_pg::settings::SettingsRow;
use kronika_source_pg::statements::{StatementsCapability, StatementsVersion};
use kronika_source_pg::store_plans::{Flavour, StorePlansCapability};

use super::{
    BatchWrite, CachedSettings, DISCOVERY_INTERVAL, DatabaseCapabilities, GenerationProbe,
    PgObservation, PgSources, QueryCompletion, QueryFailure, QueryOutcome, SERVER_PROBE_SQL,
    cached_settings_for_generation, capabilities_from_inventory, capabilities_match_generation,
    capability_sqlstate, discovery_due, finish_batched_kind, fixed_source_can_continue, measure,
    selected_statements, selected_statements_info, selected_store_plans, selected_store_plans_info,
    session_for_generation, settings_equal_ignoring_ts, try_another_database,
};

fn statements(version: StatementsVersion) -> StatementsCapability {
    StatementsCapability {
        version,
        schema: ExtensionSchema::new("monitoring"),
    }
}

fn plans(flavour: Flavour) -> StorePlansCapability {
    StorePlansCapability {
        flavour,
        schema: ExtensionSchema::new("monitoring"),
    }
}

fn capabilities(
    statements: Option<StatementsCapability>,
    plans: Option<StorePlansCapability>,
) -> DatabaseCapabilities {
    DatabaseCapabilities {
        generation: 7,
        statements,
        store_plans: plans,
        ..DatabaseCapabilities::default()
    }
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
fn statements_choose_the_richest_layout_before_database_preference() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(Some(statements(StatementsVersion::V1)), None),
        ),
        (
            "beta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
        (
            "zeta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
    ]);

    assert_eq!(
        selected_statements(&capabilities, Some("alpha"), &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned()),
        "an older current-database installation must not hide newer counters"
    );
    assert_eq!(
        selected_statements(&capabilities, Some("zeta"), &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("zeta".to_owned()),
        "the current database wins between equal richest layouts"
    );
    assert_eq!(
        selected_statements(&capabilities, None, &BTreeSet::new()).map(|(database, _, _)| database),
        Some("beta".to_owned()),
        "the database name is the deterministic final tie-break"
    );
}

#[test]
fn statements_fallback_is_deterministic_across_databases_and_layouts() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(Some(statements(StatementsVersion::V1)), None),
        ),
        (
            "beta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
        (
            "zeta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
    ]);
    let mut excluded = BTreeSet::new();

    assert_eq!(
        selected_statements(&capabilities, None, &excluded).map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    excluded.insert("beta".to_owned());
    assert_eq!(
        selected_statements(&capabilities, None, &excluded).map(|(database, _, _)| database),
        Some("zeta".to_owned())
    );
    excluded.insert("zeta".to_owned());
    assert_eq!(
        selected_statements(&capabilities, None, &excluded).map(|(database, _, _)| database),
        Some("alpha".to_owned())
    );
}

#[test]
fn extension_fallback_stops_after_completion_or_an_admitted_batch() {
    assert!(!try_another_database(QueryCompletion::Complete, false));
    assert!(!try_another_database(QueryCompletion::Complete, true));
    assert!(!try_another_database(
        QueryCompletion::ServerTimedOut,
        false
    ));
    assert!(!try_another_database(QueryCompletion::ServerTimedOut, true));
    for completion in [
        QueryCompletion::SourceFailed,
        QueryCompletion::CapabilityChanged,
        QueryCompletion::ConnectionFailed,
        QueryCompletion::TimedOut,
    ] {
        assert!(try_another_database(completion, false));
        assert!(!try_another_database(completion, true));
    }
}

#[test]
fn fixed_sources_skip_capability_errors_without_dropping_the_generation() {
    assert!(fixed_source_can_continue(QueryCompletion::SourceFailed));
    assert!(fixed_source_can_continue(
        QueryCompletion::CapabilityChanged
    ));
    assert!(fixed_source_can_continue(QueryCompletion::ServerTimedOut));
    assert!(!fixed_source_can_continue(
        QueryCompletion::ConnectionFailed
    ));
    assert!(!fixed_source_can_continue(QueryCompletion::TimedOut));
}

#[test]
fn main_readers_and_info_views_are_selected_independently() {
    let schema = ExtensionSchema::new("monitoring");
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            DatabaseCapabilities {
                statements_info: Some(schema.clone()),
                store_plans_info: Some(schema),
                ..DatabaseCapabilities::default()
            },
        ),
        (
            "beta".to_owned(),
            capabilities(
                Some(statements(StatementsVersion::V1)),
                Some(plans(Flavour::OsscCompatible)),
            ),
        ),
    ]);

    assert_eq!(
        selected_statements(&capabilities, None, &BTreeSet::new()).map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_statements_info(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned())
    );
    assert_eq!(
        selected_store_plans(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_store_plans_info(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned())
    );
}

#[test]
fn statements_info_remains_available_without_full_statement_visibility() {
    let capabilities = capabilities_from_inventory(
        &[InventoryEntry {
            name: "pg_stat_statements".to_owned(),
            extversion: "1.12".to_owned(),
            schema: ExtensionSchema::new("monitoring"),
            schema_usable: true,
            full_visibility: false,
            statements_info: true,
            store_plans_info: false,
            statements_reader: true,
            store_plans_zero_arg: false,
            store_plans_bool_arg: false,
            store_plans_key_getter: false,
            store_plans_text_converter: false,
            store_plans_ossc_columns: false,
            store_plans_vadv_columns: false,
            store_plans_datasentinel_columns: false,
        }],
        7,
        18,
    );

    assert!(capabilities.statements.is_none());
    assert_eq!(
        capabilities
            .statements_info
            .as_ref()
            .map(ExtensionSchema::name),
        Some("monitoring")
    );
}

#[test]
fn store_plans_flavours_are_not_ranked() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(None, Some(plans(Flavour::OsscCompatible))),
        ),
        (
            "beta".to_owned(),
            capabilities(None, Some(plans(Flavour::Datasentinel))),
        ),
        (
            "gamma".to_owned(),
            capabilities(None, Some(plans(Flavour::Vadv))),
        ),
        (
            "zeta".to_owned(),
            capabilities(None, Some(plans(Flavour::Vadv))),
        ),
    ]);

    assert_eq!(
        selected_store_plans(&capabilities, Some("alpha"), &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned()),
        "the current database wins without ranking independent implementations"
    );
    assert_eq!(
        selected_store_plans(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned()),
        "the database name is the deterministic fallback"
    );
}

#[test]
fn capability_invalidation_preserves_independent_info_and_tracks_a_move() {
    let mut sources = PgSources::default();
    let schema = ExtensionSchema::new("monitoring");
    sources.capabilities.insert(
        "alpha".to_owned(),
        DatabaseCapabilities {
            generation: 7,
            statements: Some(statements(StatementsVersion::V1)),
            store_plans: Some(plans(Flavour::OsscCompatible)),
            statements_info: Some(schema.clone()),
            store_plans_info: Some(schema.clone()),
        },
    );
    sources.invalidate_statements("alpha");
    sources.invalidate_store_plans("alpha");
    assert!(selected_statements(&sources.capabilities, None, &BTreeSet::new()).is_none());
    assert!(selected_store_plans(&sources.capabilities, None, &BTreeSet::new()).is_none());
    assert!(selected_statements_info(&sources.capabilities, None, &BTreeSet::new()).is_some());
    assert!(selected_store_plans_info(&sources.capabilities, None, &BTreeSet::new()).is_some());

    sources.invalidate_statements_info("alpha");
    sources.invalidate_store_plans_info("alpha");
    sources.capabilities.insert(
        "beta".to_owned(),
        DatabaseCapabilities {
            statements_info: Some(schema.clone()),
            store_plans_info: Some(schema),
            ..DatabaseCapabilities::default()
        },
    );
    assert_eq!(
        selected_statements_info(&sources.capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_store_plans_info(&sources.capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
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
        capabilities(Some(statements(StatementsVersion::V6)), None),
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
        .collect_extensions_dynamic(&probe, &mut |_observation| {}, None, &mut |_, _| {
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

#[test]
fn runtime_capability_failures_schedule_rediscovery() {
    for code in ["42P01", "42883", "42704", "42703", "3F000", "42501"] {
        assert!(capability_sqlstate(code), "{code}");
    }
    assert!(!capability_sqlstate("22003"));
}

#[test]
fn timeout_observation_retains_partial_query_accounting() {
    let mut observations = Vec::new();
    let mut observe = |observation| observations.push(observation);
    let mut measured = measure(&mut observe, "probe", "monitor@db.example:5432", "postgres");
    measured.stats_mut().rows = 9;
    measured.timeout();

    let PgObservation::Query(observation) = observations.pop().expect("one observation") else {
        panic!("expected a query observation");
    };
    assert_eq!(observation.outcome, QueryOutcome::Timeout);
    assert_eq!(observation.stats.rows, 9);
}

#[test]
fn server_statement_timeout_is_timeout_telemetry_not_capability_loss() {
    assert_eq!(
        tokio_postgres::error::SqlState::QUERY_CANCELED.code(),
        "57014"
    );
    assert!(!capability_sqlstate("57014"));

    let mut observations = Vec::new();
    let mut observe = |observation| observations.push(observation);
    measure(&mut observe, "probe", "monitor@db.example:5432", "postgres")
        .server_timeout("canceling statement due to statement timeout".to_owned());

    let PgObservation::Query(observation) = observations.pop().expect("one observation") else {
        panic!("expected a query observation");
    };
    assert_eq!(observation.outcome, QueryOutcome::Timeout);
    assert_eq!(
        observation.error.as_deref(),
        Some("canceling statement due to statement timeout")
    );
}

#[tokio::test]
async fn lock_timeout_is_a_source_error_and_keeps_the_session_usable() {
    use futures_util::TryStreamExt as _;

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the protocol probe");
    let port = listener
        .local_addr()
        .expect("read the probe address")
        .port();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().expect("accept the connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bound the probe");
        accept_startup(&mut stream);
        for _ in 0..2 {
            let mut header = [0_u8; 5];
            stream
                .read_exact(&mut header)
                .expect("read the query header");
            assert_eq!(header[0], b'Q');
            let len = u32::from_be_bytes(header[1..].try_into().expect("length bytes"));
            let mut body = vec![0; usize::try_from(len - 4).expect("query fits")];
            stream.read_exact(&mut body).expect("read the query body");
            let error = b"SERROR\0C55P03\0Mcanceling statement due to lock timeout\0\0";
            stream.write_all(b"E").expect("write the error tag");
            stream
                .write_all(
                    &u32::try_from(error.len() + 4)
                        .expect("error fits")
                        .to_be_bytes(),
                )
                .expect("write the error length");
            stream.write_all(error).expect("write the lock timeout");
            stream
                .write_all(&[b'Z', 0, 0, 0, 5, b'I'])
                .expect("write ready");
            stream.flush().expect("flush the error");
        }
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).expect("read session close"), 0);
        closed_tx.send(()).expect("report closed session");
    });
    let mut pool = Pool::new(&format!(
        "host=127.0.0.1 port={port} user=monitor dbname=metrics"
    ))
    .expect("the probe DSN parses");
    let mut observations = Vec::new();
    for batched in [false, true] {
        let error = {
            let session = pool.session().await.expect("open or reuse the session");
            assert_eq!(session.generation(), 1);
            let mut stats = kronika_source_pg::query::QueryStats::default();
            let stream = session
                .simple_stream("SELECT 1", &mut stats)
                .await
                .expect("send the query");
            let mut stream = std::pin::pin!(stream);
            stream.try_next().await.expect_err("receive SQLSTATE 55P03")
        };
        assert_eq!(
            error.code(),
            Some(&tokio_postgres::error::SqlState::LOCK_NOT_AVAILABLE)
        );
        let mut observe = |observation| observations.push(observation);
        let measured = measure(&mut observe, "probe", "monitor@127.0.0.1", "metrics");
        let completion = if batched {
            finish_batched_kind(
                &mut pool,
                measured,
                Err(BatchError::<()>::PostgreSql(error)),
            )
            .expect("a server error is a source failure")
        } else {
            super::finish_failed::<()>(measured, Ok(Err(error.into())))
        };
        assert_eq!(completion, QueryCompletion::SourceFailed);
        assert!(fixed_source_can_continue(completion));
        assert_eq!(pool.generation(), Some(1));
    }
    assert_eq!(observations.len(), 2);
    for observation in observations {
        let PgObservation::Query(observation) = observation else {
            panic!("expected a query observation");
        };
        assert_eq!(observation.outcome, QueryOutcome::Error);
        assert!(
            observation
                .error
                .as_deref()
                .is_some_and(|message| message.contains("lock timeout"))
        );
    }
    pool.close();
    closed_rx.await.expect("the driver closes the session");
    server.join().expect("the protocol probe exits");
}

#[test]
fn dropping_an_inflight_measurement_emits_one_cancelled_observation() {
    let mut observations = Vec::new();
    {
        let mut observe = |observation| observations.push(observation);
        let mut measured = measure(
            &mut observe,
            "pg_stat_statements",
            "monitor@db.example:5432",
            "postgres",
        );
        measured.stats_mut().rows = 9;
    }

    assert_eq!(observations.len(), 1);
    let PgObservation::Query(observation) = observations.pop().expect("one observation") else {
        panic!("expected a query observation");
    };
    assert_eq!(observation.query_name, "pg_stat_statements");
    assert_eq!(observation.outcome, QueryOutcome::Error);
    assert_eq!(observation.stats.rows, 9);
    assert_eq!(
        observation.error.as_deref(),
        Some("collector stopped while the query was running")
    );
}

#[test]
fn completed_measurement_does_not_emit_from_drop() {
    let mut observations = Vec::new();
    {
        let mut observe = |observation| observations.push(observation);
        measure(&mut observe, "probe", "monitor@db.example:5432", "postgres").success();
    }
    assert_eq!(observations.len(), 1);
}

#[test]
fn unavailable_session_is_accounted_as_a_closed_connection() {
    let mut pool = Pool::new("host=127.0.0.1 dbname=metrics")
        .expect("syntactically valid connection settings");
    let mut observations = Vec::new();
    {
        let mut observe = |observation| observations.push(observation);
        assert!(matches!(
            session_for_generation(&mut pool, 1, &mut observe),
            Err(QueryFailure::Connection)
        ));
    }

    let PgObservation::Connection(observation) =
        observations.pop().expect("one connection observation")
    else {
        panic!("expected a connection observation");
    };
    assert_eq!(observation.database, "metrics");
    assert!(observation.closed);
    assert!(!observation.timeout);
}

#[tokio::test]
async fn a_batch_decode_error_forces_the_next_query_to_reconnect() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the protocol probe");
    let port = listener
        .local_addr()
        .expect("read the probe address")
        .port();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || serve_two_handshakes(listener, release_rx));
    let mut pool = Pool::new(&format!(
        "host=127.0.0.1 port={port} user=monitor dbname=metrics"
    ))
    .expect("the probe DSN parses");

    assert_eq!(
        pool.session()
            .await
            .expect("open the first connection")
            .generation(),
        1
    );
    let completion = {
        let mut observe = |_observation| {};
        finish_batched_kind(
            &mut pool,
            measure(&mut observe, "pg_locks", "monitor@127.0.0.1", "metrics"),
            Err(BatchError::<()>::Decode(anyhow::anyhow!(
                "unexpected row shape"
            ))),
        )
        .expect("a decode error is a source failure")
    };
    assert_eq!(completion, QueryCompletion::SourceFailed);
    assert_eq!(pool.generation(), None);

    assert_eq!(
        pool.session()
            .await
            .expect("open a replacement connection")
            .generation(),
        2
    );
    release_tx.send(()).expect("release the protocol probe");
    server.join().expect("the protocol probe exits");
}

fn serve_two_handshakes(listener: TcpListener, release: mpsc::Receiver<()>) {
    let (mut first, _peer) = listener.accept().expect("accept the first connection");
    accept_startup(&mut first);
    let (mut second, _peer) = listener.accept().expect("accept the second connection");
    accept_startup(&mut second);
    drop(listener);
    release.recv().expect("the test releases the connections");
    drop((first, second, release));
}

fn accept_startup(stream: &mut TcpStream) {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).expect("read startup length");
    let body_len = usize::try_from(u32::from_be_bytes(len).saturating_sub(4))
        .expect("the startup body length fits usize");
    let mut body = vec![0_u8; body_len];
    stream.read_exact(&mut body).expect("read startup body");
    stream
        .write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'Z', 0, 0, 0, 5, b'I'])
        .expect("write authentication and ready messages");
    stream.flush().expect("flush startup response");

    let mut tag = [0_u8; 1];
    stream.read_exact(&mut tag).expect("read setup tag");
    assert_eq!(tag[0], b'Q');
    stream.read_exact(&mut len).expect("read setup length");
    let body_len = usize::try_from(u32::from_be_bytes(len).saturating_sub(4))
        .expect("the setup body length fits usize");
    let mut body = vec![0_u8; body_len];
    stream.read_exact(&mut body).expect("read setup body");
    let sql = body.strip_suffix(&[0]).expect("setup SQL is terminated");
    let sql = std::str::from_utf8(sql).expect("setup SQL is UTF-8");
    assert!(sql.contains("SET statement_timeout = '30s'"));
    assert!(sql.contains("SET lock_timeout = '100ms'"));
    stream
        .write_all(&[
            b'C', 0, 0, 0, 8, b'S', b'E', b'T', 0, b'C', 0, 0, 0, 8, b'S', b'E', b'T', 0, b'Z', 0,
            0, 0, 5, b'I',
        ])
        .expect("write setup completion and ready messages");
    stream.flush().expect("flush setup response");
}
