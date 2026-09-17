//! Scheduled groups select real queries even when extension capabilities are cached.

use std::net::TcpListener;
use std::thread::JoinHandle;
use std::time::Instant;

use kronika_source_pg::Pool;
use kronika_source_pg::extension::ExtensionSchema;
use kronika_source_pg::query::BatchWrite;
use kronika_source_pg::statements::{StatementsCapability, StatementsVersion};
use kronika_source_pg::store_plans::{Flavour, StorePlansCapability};

use super::super::capabilities::DatabaseCapabilities;
use super::super::{PgBatch, PgObservation, PgSources, QueryOutcome};
use super::protocol::serve_enumeration_denied;
use crate::scheduler::{DueSet, SourceKind};

async fn connected_with_extensions() -> (PgSources, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind pacing fixture");
    let port = listener.local_addr().expect("fixture address").port();
    let server = std::thread::spawn(move || serve_enumeration_denied(&listener));
    let mut sources = PgSources {
        server: Some(
            Pool::new(&format!(
                "host=127.0.0.1 port={port} user=monitor dbname=metrics sslmode=disable"
            ))
            .expect("fixture DSN"),
        ),
        ..PgSources::default()
    };
    let probe = sources
        .read_probe(false, &mut |_| {})
        .await
        .expect("resolve the live primary generation");
    sources.capabilities.insert(
        probe.database,
        DatabaseCapabilities {
            generation: probe.generation,
            statements: Some(StatementsCapability {
                version: StatementsVersion::V4,
                schema: ExtensionSchema::new("monitoring"),
            }),
            store_plans: Some(StorePlansCapability {
                flavour: Flavour::OsscCompatible,
                schema: ExtensionSchema::new("monitoring"),
            }),
            ..DatabaseCapabilities::default()
        },
    );
    sources.last_discovery = Some(Instant::now());
    (sources, server)
}

#[tokio::test]
async fn statements_and_plans_tick_attempts_both_extensions_without_other_groups() {
    let (mut sources, server) = connected_with_extensions().await;
    let mut queries = Vec::new();
    sources
        .collect(
            &DueSet::for_test(vec![SourceKind::PgStatementsAndPlans]),
            &mut |event| {
                if let PgObservation::Query(query) = event {
                    queries.push((query.query_name, query.outcome));
                }
            },
            |_batch, _settings| -> Result<BatchWrite, ()> {
                panic!("the fixture rejects extension queries before returning rows")
            },
        )
        .await
        .expect("permission errors skip their sources");
    sources.close_connections();
    tokio::task::yield_now().await;
    server.join().expect("protocol fixture exits");

    assert_eq!(
        queries,
        [
            ("pg_stat_statements", QueryOutcome::Error),
            ("pg_store_plans", QueryOutcome::Error),
        ],
        "both cached readers must be attempted without activity, settings or server counters"
    );
}

#[tokio::test]
async fn activity_tick_does_not_read_cached_statement_or_plan_extensions() {
    let (mut sources, server) = connected_with_extensions().await;
    let mut queries = Vec::new();
    let mut admitted = Vec::new();
    sources
        .collect(
            &DueSet::for_test(vec![SourceKind::PgActivity]),
            &mut |event| {
                if let PgObservation::Query(query) = event {
                    queries.push(query.query_name);
                }
            },
            |batch, _settings| {
                match batch {
                    PgBatch::Activity(_, rows) => admitted.push(("activity", rows.len())),
                    PgBatch::Locks(_, rows) => admitted.push(("locks", rows.len())),
                    other => panic!("unexpected batch on activity tick: {other:?}"),
                }
                Ok::<_, ()>(BatchWrite::default())
            },
        )
        .await
        .expect("collect the activity group");
    sources.close_connections();
    tokio::task::yield_now().await;
    server.join().expect("protocol fixture exits");

    assert_eq!(admitted, [("activity", 1), ("locks", 1)]);
    assert_eq!(
        queries,
        ["pg_stat_activity", "pg_locks", "pg_stat_progress_vacuum"],
        "cached capabilities must not pull heavy queries into the activity tick"
    );
}

#[tokio::test]
async fn activity_precedes_counters_and_extensions_when_all_are_due() {
    let (mut sources, server) = connected_with_extensions().await;
    let mut queries = Vec::new();
    sources
        .collect(
            &DueSet::for_test(vec![
                SourceKind::PgInstance,
                SourceKind::PgStatementsAndPlans,
                SourceKind::PgActivity,
            ]),
            &mut |event| {
                if let PgObservation::Query(query) = event {
                    queries.push(query.query_name);
                }
            },
            |_batch, _settings| Ok::<_, ()>(BatchWrite::default()),
        )
        .await
        .expect("independent SQL errors do not stop the remaining groups");
    sources.close_connections();
    tokio::task::yield_now().await;
    server.join().expect("protocol fixture exits");

    assert_eq!(
        &queries[..3],
        ["pg_stat_activity", "pg_locks", "pg_stat_progress_vacuum"],
    );
    assert_eq!(queries[3], "pg_settings");
    assert!(queries.contains(&"pg_stat_database"));
    assert_eq!(
        &queries[queries.len() - 2..],
        ["pg_stat_statements", "pg_store_plans"]
    );
}
