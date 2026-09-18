use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};
use tokio_postgres::{Config, NoTls};

use super::{
    BATCH_LOGICAL_BYTES, BatchError, BatchWrite, CANCEL_REQUEST_TIMEOUT, ColumnLookup,
    QUERY_FETCH_TIMEOUT, QueryStats, SERVER_STATEMENT_TIMEOUT, SESSION_SETUP_SQL, Session,
    TEXT_PREFIX_CHARS, is_query_cancelled, read_batched,
};

#[test]
fn column_lookup_keeps_the_first_index_for_duplicate_names() {
    let lookup = ColumnLookup::from_names(["first", "duplicate", "duplicate", "last"]);
    assert_eq!(lookup.indexes.get("first"), Some(&0));
    assert_eq!(lookup.indexes.get("duplicate"), Some(&1));
    assert_eq!(lookup.indexes.get("last"), Some(&3));
}

#[test]
fn one_maximum_utf8_text_field_is_smaller_than_the_batch_byte_target() {
    assert!(TEXT_PREFIX_CHARS.saturating_mul(4) < BATCH_LOGICAL_BYTES);
}

#[test]
fn multirow_metric_collectors_use_the_bounded_stream_helper() {
    for source in [
        include_str!("../database.rs"),
        include_str!("../io.rs"),
        include_str!("../prepared_xacts.rs"),
        include_str!("../progress_vacuum.rs"),
    ] {
        assert!(source.contains("query::read_batched("));
        assert!(!source.contains("query::read_all("));
    }
}

#[test]
fn write_accounting_keeps_storage_and_fetch_time_separate() {
    let mut stats = QueryStats::default();
    stats.record_batch_write(
        Duration::from_millis(11),
        BatchWrite {
            encode_elapsed: Duration::from_millis(3),
            append_elapsed: Duration::from_millis(5),
            encoded_bytes: 700,
            wal_bytes_appended: 900,
        },
    );
    assert_eq!(stats.batches, 1);
    assert_eq!(stats.encode_elapsed, Duration::from_millis(3));
    assert_eq!(stats.append_elapsed, Duration::from_millis(5));
    assert_eq!(stats.encoded_bytes, 700);
    assert_eq!(stats.wal_bytes_appended, 900);
    assert_eq!(
        stats.fetch_elapsed(Duration::from_millis(19)),
        Duration::from_millis(8)
    );
}

#[test]
fn public_failed_batch_hook_separates_sink_time_from_fetch_time() {
    let mut stats = QueryStats::default();
    stats.record_failed_batch(Duration::from_millis(7));

    assert_eq!(stats.batches, 1);
    assert_eq!(
        stats.fetch_elapsed(Duration::from_millis(19)),
        Duration::from_millis(12)
    );
    assert_eq!(stats.encoded_bytes, 0);
    assert_eq!(stats.wal_bytes_appended, 0);
}

#[test]
fn server_timeout_precedes_the_client_backstop() {
    assert_eq!(SERVER_STATEMENT_TIMEOUT, Duration::from_secs(30));
    assert_eq!(QUERY_FETCH_TIMEOUT, Duration::from_secs(35));
    assert!(QUERY_FETCH_TIMEOUT > SERVER_STATEMENT_TIMEOUT);
    assert_eq!(CANCEL_REQUEST_TIMEOUT, Duration::from_secs(1));
    assert!(SESSION_SETUP_SQL.contains("SET statement_timeout = '30s'"));
}

#[tokio::test(start_paused = true)]
async fn a_query_timeout_runs_the_cancel_step_before_returning() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let mark = Arc::clone(&cancelled);
    let result = timeout_at_with_cancel(
        tokio::time::Instant::now() + Duration::from_secs(30),
        std::future::pending::<()>(),
        move || async move {
            mark.store(true, Ordering::SeqCst);
        },
    )
    .await;

    assert!(result.is_err());
    assert!(cancelled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_successful_query_does_not_construct_a_cancel_step() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let mark = Arc::clone(&cancelled);
    let result = timeout_at_with_cancel(
        tokio::time::Instant::now() + Duration::from_secs(30),
        std::future::ready(7),
        move || {
            mark.store(true, Ordering::SeqCst);
            std::future::ready(())
        },
    )
    .await;

    assert_eq!(result.expect("future completes"), 7);
    assert!(!cancelled.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn a_stalled_typed_stream_returns_the_timeout_variant() {
    let (client, driver, server) = connect_to_probe_mode(false).await;
    let mut stats = QueryStats::default();
    let result = read_batched(
        Session::new(&client, 1),
        "SELECT 1::int8",
        std::iter::empty::<(String, tokio_postgres::types::Type)>(),
        0,
        &mut stats,
        |_row| Ok(()),
        |_row| 0,
        |_batch| Ok::<BatchWrite, ()>(BatchWrite::default()),
    )
    .await;
    assert!(matches!(result, Err(BatchError::Timeout)));
    assert_eq!(stats.rows, 0);
    assert_eq!(stats.batches, 0);
    assert!(stats.application_payload_to_postgres_bytes > 0);
    server.abort();
    driver.abort();
}

#[tokio::test]
async fn simple_stream_sends_only_a_simple_query_message() {
    let (client, driver, server) = connect_to_probe().await;
    let mut stats = QueryStats::default();
    let stream = Session::new(&client, 1)
        .simple_stream("SHOW server_version_num", &mut stats)
        .await
        .expect("the simple request is accepted");
    drop(stream);
    drop(client);

    let messages = server.await.expect("the probe task completes");
    driver.abort();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, b'Q');
    assert!(messages.iter().all(|(tag, _body)| *tag != b'P'));
    assert!(messages.iter().all(|(tag, _body)| *tag != b'C'));
}

#[tokio::test]
async fn typed_stream_uses_unnamed_parse_bind_without_statement_close() {
    let (client, driver, server) = connect_to_probe().await;
    let mut stats = QueryStats::default();
    let session = Session::new(&client, 9);
    assert_eq!(
        session.generation(),
        9,
        "generation-scoped caches must see the connection generation"
    );
    let stream = session
        .typed_stream(
            "SELECT $1::int8",
            std::iter::once((7_i64, tokio_postgres::types::Type::INT8)),
            size_of::<i64>(),
            &mut stats,
        )
        .await
        .expect("the typed request is accepted");
    drop(stream);
    drop(client);

    let messages = server.await.expect("the probe task completes");
    driver.abort();
    assert_eq!(
        messages.iter().map(|(tag, _body)| *tag).collect::<Vec<_>>(),
        [b'P', b'B', b'D', b'E', b'S']
    );
    let parse = &messages[0].1;
    assert_eq!(
        parse.first(),
        Some(&0),
        "Parse statement name must be empty"
    );
    let bind = &messages[1].1;
    assert_eq!(bind.first(), Some(&0), "Bind portal name must be empty");
    assert_eq!(bind.get(1), Some(&0), "Bind statement name must be empty");
    assert!(messages.iter().all(|(tag, _body)| *tag != b'C'));
}

#[tokio::test]
async fn query_canceled_sqlstate_is_detected_without_message_matching() {
    let (client_io, mut server_io) = tokio::io::duplex(4_096);
    let server = tokio::spawn(async move {
        let startup_len = server_io.read_u32().await.expect("read startup length");
        let mut startup = vec![0; usize::try_from(startup_len - 4).expect("startup fits")];
        server_io
            .read_exact(&mut startup)
            .await
            .expect("read startup body");
        write_backend(&mut server_io, b'R', &0_i32.to_be_bytes()).await;
        write_backend(&mut server_io, b'Z', b"I").await;

        assert_eq!(server_io.read_u8().await.expect("read query tag"), b'Q');
        let query_len = server_io.read_u32().await.expect("read query length");
        let mut query = vec![0; usize::try_from(query_len - 4).expect("query fits")];
        server_io
            .read_exact(&mut query)
            .await
            .expect("read query body");
        write_backend(
            &mut server_io,
            b'E',
            b"SERROR\0C57014\0Mlocalized cancellation text\0\0",
        )
        .await;
        write_backend(&mut server_io, b'Z', b"I").await;
    });
    let mut config = Config::new();
    config.user("kronika-timeout-classification-test");
    let (client, connection) = config
        .connect_raw(client_io, NoTls)
        .await
        .expect("the probe performs a valid startup handshake");
    let driver = tokio::spawn(connection);

    let error = client
        .batch_execute("SELECT pg_sleep(60)")
        .await
        .expect_err("the server cancels the query");
    assert!(is_query_cancelled(&error));
    server.await.expect("the protocol probe exits");
    driver.abort();
}

async fn connect_to_probe() -> (
    tokio_postgres::Client,
    tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
    tokio::task::JoinHandle<Vec<(u8, Vec<u8>)>>,
) {
    connect_to_probe_mode(true).await
}

async fn connect_to_probe_mode(
    respond: bool,
) -> (
    tokio_postgres::Client,
    tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
    tokio::task::JoinHandle<Vec<(u8, Vec<u8>)>>,
) {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server = tokio::spawn(protocol_probe(server_io, respond));
    let mut config = Config::new();
    config.user("kronika-protocol-test");
    config.dbname("postgres");
    let (client, connection) = config
        .connect_raw(client_io, NoTls)
        .await
        .expect("the probe performs a valid startup handshake");
    let driver = tokio::spawn(connection);
    (client, driver, server)
}

async fn protocol_probe(mut io: DuplexStream, respond: bool) -> Vec<(u8, Vec<u8>)> {
    let startup_len = io.read_u32().await.expect("read startup length");
    let mut startup = vec![0; usize::try_from(startup_len - 4).expect("startup fits")];
    io.read_exact(&mut startup)
        .await
        .expect("read startup body");

    write_backend(&mut io, b'R', &0_i32.to_be_bytes()).await;
    write_backend(&mut io, b'Z', b"I").await;

    let mut messages = Vec::new();
    loop {
        let tag = io.read_u8().await.expect("read frontend tag");
        let len = io.read_u32().await.expect("read frontend length");
        let mut body = vec![0; usize::try_from(len - 4).expect("message fits")];
        io.read_exact(&mut body).await.expect("read frontend body");
        messages.push((tag, body));
        if matches!(tag, b'Q' | b'S') {
            break;
        }
    }

    if messages.first().is_some_and(|(tag, _body)| *tag == b'P') {
        if !respond {
            std::future::pending::<()>().await;
        }
        write_backend(&mut io, b'1', b"").await;
        write_backend(&mut io, b'2', b"").await;
        write_backend(&mut io, b'n', b"").await;
        write_backend(&mut io, b'C', b"SELECT 0\0").await;
        write_backend(&mut io, b'Z', b"I").await;
    }
    messages
}

async fn write_backend(io: &mut DuplexStream, tag: u8, body: &[u8]) {
    io.write_u8(tag).await.expect("write backend tag");
    let len = u32::try_from(body.len() + 4).expect("test message fits");
    io.write_u32(len).await.expect("write backend length");
    io.write_all(body).await.expect("write backend body");
    io.flush().await.expect("flush backend message");
}

async fn timeout_at_with_cancel<T, C>(
    deadline: tokio::time::Instant,
    future: impl Future<Output = T>,
    cancel: impl FnOnce() -> C,
) -> anyhow::Result<T, tokio::time::error::Elapsed>
where
    C: Future<Output = ()>,
{
    match tokio::time::timeout_at(deadline, future).await {
        Ok(value) => Ok(value),
        Err(elapsed) => {
            cancel().await;
            Err(elapsed)
        }
    }
}
