//! Query completion, timeout accounting, and connection reuse after failures.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use kronika_source_pg::Pool;
use kronika_source_pg::query::BatchError;

use super::super::execution::{
    QueryCompletion, QueryFailure, capability_sqlstate, finish_batched_kind, finish_failed,
    fixed_source_can_continue, session_for_generation,
};
use super::super::extensions::try_another_database;
use super::super::measurement::measure;
use super::super::{PgObservation, QueryOutcome};
use super::protocol::{accept_startup, serve_two_handshakes};

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
        // read_exact retries interrupted reads; EOF must arrive before another byte.
        assert_eq!(
            stream
                .read_exact(&mut byte)
                .expect_err("session closes without another protocol message")
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
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
            finish_failed::<()>(measured, Ok(Err(error.into())))
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
