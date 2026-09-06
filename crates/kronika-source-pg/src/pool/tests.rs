use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_postgres::{Config, NoTls};

use crate::query::{QueryStats, SESSION_SETUP_SQL};

use super::{
    CONNECT_TIMEOUT, ConnectError, MAX_AGE, Open, Pool, application_name,
    collector_application_name,
};

/// A DSN nothing listens on; these tests never open a connection.
const UNUSED: &str = "host=/nonexistent dbname=kronika";

fn pool() -> Pool {
    Pool::new(UNUSED).expect("the DSN parses")
}

#[test]
fn a_fresh_pool_holds_no_connection_generation() {
    let mut pool = pool();
    assert_eq!(pool.generation(), None);
    assert!(pool.session_for_generation(1).is_none());
    assert!(pool.open.is_none(), "the generation guard must not connect");
}

#[test]
fn the_connect_deadline_is_small_and_finite() {
    assert!(CONNECT_TIMEOUT > Duration::ZERO);
    assert!(CONNECT_TIMEOUT <= Duration::from_secs(10));
}

#[test]
fn healthy_frontend_sessions_rotate_after_one_hour() {
    assert_eq!(MAX_AGE, Duration::from_hours(1));
}

#[test]
fn timeout_classification_is_explicit() {
    assert!(ConnectError::Timeout.is_timeout());
}

#[test]
fn closing_a_pool_that_never_opened_is_not_an_error() {
    let mut pool = pool();
    pool.close();
    assert_eq!(pool.generation(), None);
}

#[test]
fn a_dsn_that_is_not_a_connection_string_is_rejected() {
    assert!(Pool::new("host=").is_err());
}

#[test]
fn a_url_dsn_is_accepted_as_well_as_keywords() {
    let pool = Pool::new("postgres://reader@example:5433/appdb").expect("the URL parses");
    assert_eq!(pool.config.get_dbname(), Some("appdb"));
    assert_eq!(pool.database_label(), "appdb");
}

#[test]
fn another_database_keeps_server_configuration_without_an_open_session() {
    let mut primary = pool();
    primary.remember_resolved_identity("actual_role", "kronika");
    let other = primary.on_database("payments");
    assert_eq!(other.config.get_dbname(), Some("payments"));
    assert_eq!(other.generation(), None);
    assert!(other.connection_label(0).starts_with("actual_role@"));
}

#[tokio::test]
async fn session_setup_precedes_the_first_query_and_uses_only_simple_protocol() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the protocol probe");
    let port = listener
        .local_addr()
        .expect("read the probe address")
        .port();
    let (first_set_tx, first_set_rx) = tokio::sync::oneshot::channel();
    let (finish_setup_tx, finish_setup_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().expect("accept the frontend session");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bound the cleanup check");
        accept_startup(&mut stream);
        let setup = read_frontend(&mut stream);
        write_backend(&mut stream, b'C', b"SET\0");
        stream.flush().expect("flush the first SET completion");
        first_set_tx
            .send(())
            .expect("report the first SET completion");
        finish_setup_rx.recv().expect("release the remaining setup");
        write_command_ready(&mut stream, "SET");
        let first_query = read_frontend(&mut stream);
        write_command_ready(&mut stream, "SELECT 0");
        let reused_query = read_frontend(&mut stream);
        write_command_ready(&mut stream, "SELECT 0");
        [setup, first_query, reused_query]
    });
    let mut pool = Pool::new(&format!(
        "host=127.0.0.1 port={port} user=monitor dbname=metrics"
    ))
    .expect("the probe DSN parses");

    let mut opening = tokio::spawn(async move {
        let generation = pool
            .session()
            .await
            .expect("configure the session")
            .generation();
        (pool, generation)
    });
    first_set_rx.await.expect("the first SET completed");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut opening)
            .await
            .is_err(),
        "one completed SET must not expose the session"
    );
    finish_setup_tx.send(()).expect("finish both SET commands");
    let (mut pool, generation) = opening.await.expect("the setup task exits");
    assert_eq!(generation, 1);
    for _ in 0..2 {
        let session = pool.session().await.expect("reuse the configured session");
        assert_eq!(session.generation(), generation);
        let mut stats = QueryStats::default();
        crate::query::read_simple_rows(session, "SELECT 1", &mut stats, |_row| Ok(()))
            .await
            .expect("complete a monitoring query");
    }
    pool.close();

    let messages = server.join().expect("the protocol probe exits");
    assert_eq!(messages[0].0, b'Q');
    assert_eq!(frontend_sql(&messages[0].1), SESSION_SETUP_SQL);
    let statements: Vec<_> = SESSION_SETUP_SQL.split("; ").collect();
    assert_eq!(statements.len(), 2);
    assert!(statements.iter().all(|sql| sql.starts_with("/* kronika:")));
    assert!(statements[0].ends_with("SET statement_timeout = '30s'"));
    assert!(statements[1].ends_with("SET lock_timeout = '100ms'"));
    assert_eq!(messages[1].0, b'Q');
    assert_eq!(frontend_sql(&messages[1].1), "SELECT 1");
    assert_eq!(frontend_sql(&messages[2].1), "SELECT 1");
    assert!(messages.iter().all(|(tag, _body)| *tag != b'P'));
    assert!(messages.iter().all(|(tag, _body)| *tag != b'C'));
}

#[tokio::test]
async fn rejected_session_setup_is_never_exposed_and_closes_its_driver() {
    for completed_sets in 0..2 {
        rejected_setup(completed_sets).await;
    }
}

async fn rejected_setup(completed_sets: usize) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the protocol probe");
    let port = listener
        .local_addr()
        .expect("read the probe address")
        .port();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().expect("accept the frontend session");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bound the cleanup check");
        accept_startup(&mut stream);
        let setup = read_frontend(&mut stream);
        for _ in 0..completed_sets {
            write_backend(&mut stream, b'C', b"SET\0");
        }
        write_backend(
            &mut stream,
            b'E',
            b"SERROR\0C42501\0Mmonitoring timeout rejected\0\0",
        );
        write_backend(&mut stream, b'Z', b"I");
        stream.flush().expect("flush the setup error");
        let mut byte = [0_u8; 1];
        let closed = stream.read(&mut byte).is_ok_and(|read| read == 0);
        closed_tx.send(closed).expect("report frontend cleanup");
        setup.0 == b'Q'
    });
    let mut pool = Pool::new(&format!(
        "host=127.0.0.1 port={port} user=monitor dbname=metrics"
    ))
    .expect("the probe DSN parses");

    let error = pool
        .session()
        .await
        .expect_err("the SET must fail the session");
    assert!(!error.is_timeout());
    assert!(pool.open.is_none());
    assert_eq!(pool.next_generation, 1);
    drop(pool);
    assert!(
        closed_rx
            .await
            .expect("the driver cleanup reached the server")
    );
    assert!(server.join().expect("the protocol probe exits"));
}

#[tokio::test]
async fn stalled_session_setup_hits_its_deadline_without_exposing_a_generation() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the protocol probe");
    let port = listener
        .local_addr()
        .expect("read the probe address")
        .port();
    let (setup_seen_tx, setup_seen_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().expect("accept the frontend session");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("bound the cleanup check");
        accept_startup(&mut stream);
        let setup = read_frontend(&mut stream);
        write_backend(&mut stream, b'C', b"SET\0");
        stream.flush().expect("flush the first SET completion");
        setup_seen_tx.send(()).expect("report the setup request");
        let mut byte = [0_u8; 1];
        let closed = stream.read(&mut byte).is_ok_and(|read| read == 0);
        closed_tx.send(closed).expect("report frontend cleanup");
        setup.0 == b'Q' && frontend_sql(&setup.1) == SESSION_SETUP_SQL
    });
    let mut pool = Pool::new(&format!(
        "host=127.0.0.1 port={port} user=monitor dbname=metrics"
    ))
    .expect("the probe DSN parses");
    let opening = tokio::spawn(async move {
        let result = pool
            .session()
            .await
            .map(crate::Session::generation)
            .map_err(|error| error.is_timeout());
        (pool, result)
    });

    setup_seen_rx.await.expect("the SET reached the server");
    tokio::time::pause();
    tokio::time::advance(CONNECT_TIMEOUT).await;
    let (pool, result) = opening.await.expect("the opening task exits");
    tokio::time::resume();

    assert_eq!(result, Err(true));
    assert!(pool.open.is_none());
    assert_eq!(pool.next_generation, 1);
    drop(pool);
    assert!(
        closed_rx
            .await
            .expect("the driver cleanup reached the server")
    );
    assert!(server.join().expect("the protocol probe exits"));
}

#[tokio::test]
async fn secondary_rotates_at_the_age_boundary_and_increments_generation() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind the protocol probe");
    let port = listener
        .local_addr()
        .expect("read the probe address")
        .port();
    let (release_tx, release_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut setup_sql = Vec::new();
        let (mut first, _peer) = listener.accept().expect("accept the first session");
        let startup = accept_startup(&mut first);
        assert!(
            startup
                .windows(b"database\0payments\0".len())
                .any(|field| field == b"database\0payments\0")
        );
        let first_setup = read_frontend(&mut first);
        setup_sql.push(frontend_sql(&first_setup.1).to_owned());
        write_backend(&mut first, b'C', b"SET\0");
        write_command_ready(&mut first, "SET");
        let mut byte = [0_u8; 1];
        assert_eq!(first.read(&mut byte).expect("read first session close"), 0);

        let (mut second, _peer) = listener.accept().expect("accept the replacement session");
        let startup = accept_startup(&mut second);
        assert!(
            startup
                .windows(b"database\0payments\0".len())
                .any(|field| field == b"database\0payments\0")
        );
        let second_setup = read_frontend(&mut second);
        setup_sql.push(frontend_sql(&second_setup.1).to_owned());
        write_backend(&mut second, b'C', b"SET\0");
        write_command_ready(&mut second, "SET");
        release_rx.recv().expect("the test releases the session");
        setup_sql
    });
    let primary = Pool::new(&format!(
        "host=127.0.0.1 port={port} user=monitor dbname=postgres"
    ))
    .expect("the probe DSN parses");
    let mut secondary = primary.on_database("payments");

    assert_eq!(
        secondary
            .session()
            .await
            .expect("open the first session")
            .generation(),
        1
    );
    tokio::time::pause();
    let age = secondary
        .open
        .as_ref()
        .expect("the first session is open")
        .opened_at
        .elapsed();
    let remaining = MAX_AGE.saturating_sub(age);
    assert!(remaining > Duration::from_nanos(1));
    tokio::time::advance(
        remaining
            .checked_sub(Duration::from_nanos(1))
            .expect("the session is younger than the age boundary"),
    )
    .await;
    assert_eq!(secondary.generation(), Some(1));
    assert_eq!(
        secondary
            .session()
            .await
            .expect("reuse the younger session")
            .generation(),
        1
    );

    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(secondary.generation(), None);
    tokio::time::resume();
    assert!(secondary.session_for_generation(1).is_none());
    assert_eq!(
        secondary
            .session()
            .await
            .expect("open the replacement session")
            .generation(),
        2
    );
    assert_eq!(
        secondary
            .session_for_generation(2)
            .map(crate::Session::generation),
        Some(2)
    );

    release_tx.send(()).expect("release the protocol probe");
    secondary.close();
    let setup_sql = server.join().expect("the protocol probe exits");
    assert_eq!(setup_sql, [SESSION_SETUP_SQL, SESSION_SETUP_SQL]);
}

#[test]
fn labels_use_neutral_defaults_until_the_generation_probe_resolves_them() {
    let mut pool = Pool::new("host=db.example").expect("the DSN parses");
    assert_eq!(pool.database_label(), "server-default");
    assert_eq!(pool.connection_label(3), "server-default@db.example:5432");
    pool.remember_resolved_identity("monitor", "actual_db");
    assert_eq!(pool.connection_label(3), "monitor@db.example:5432");
    assert_eq!(pool.database_label(), "actual_db");
    assert!(
        pool.config
            .get_application_name()
            .is_some_and(|name| name.starts_with("kronika-collector-"))
    );
}

#[test]
fn application_name_is_process_stable_and_changes_with_process_identity() {
    assert_eq!(collector_application_name(), collector_application_name());
    assert_ne!(
        application_name(7, Duration::from_nanos(11)),
        application_name(7, Duration::from_nanos(12))
    );
    assert_ne!(
        application_name(7, Duration::from_nanos(11)),
        application_name(8, Duration::from_nanos(11))
    );
}

#[tokio::test]
async fn a_dead_client_is_not_reported_as_a_reusable_generation() {
    let (client_io, mut server_io) = tokio::io::duplex(4_096);
    let (close_server, wait_to_close) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let startup_len = server_io.read_u32().await.expect("read startup length");
        let mut startup = vec![0; usize::try_from(startup_len - 4).expect("startup fits")];
        server_io
            .read_exact(&mut startup)
            .await
            .expect("read startup body");
        server_io.write_u8(b'R').await.expect("write auth tag");
        server_io.write_u32(8).await.expect("write auth length");
        server_io.write_i32(0).await.expect("write auth-ok code");
        server_io.write_u8(b'Z').await.expect("write ready tag");
        server_io.write_u32(5).await.expect("write ready length");
        server_io.write_u8(b'I').await.expect("write idle status");
        server_io.flush().await.expect("flush startup response");
        let _closed = wait_to_close.await;
    });

    let mut config = Config::new();
    config.user("kronika-dead-client-test");
    let (client, connection) = config
        .connect_raw(client_io, NoTls)
        .await
        .expect("the probe performs a valid startup handshake");
    let driver = tokio::spawn(async move {
        let _ended = connection.await;
    });
    let mut pool = pool();
    pool.open = Some(Open {
        client,
        driver,
        generation: 7,
        opened_at: tokio::time::Instant::now(),
    });
    pool.next_generation = 8;
    assert_eq!(pool.generation(), Some(7));
    assert_eq!(
        pool.session_for_generation(7)
            .map(crate::Session::generation),
        Some(7)
    );
    assert!(pool.session_for_generation(8).is_none());
    assert_eq!(
        pool.generation(),
        Some(7),
        "a healthy generation mismatch must not churn the connection"
    );

    close_server.send(()).expect("the server is waiting");
    server.await.expect("the server closes cleanly");
    tokio::time::timeout(Duration::from_secs(1), async {
        while pool.generation().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the driver notices EOF");
    assert_eq!(pool.generation(), None);
    assert!(pool.session_for_generation(7).is_none());
    assert!(pool.open.is_none(), "the closed session must be discarded");

    assert!(pool.session().await.is_err());
    assert!(pool.open.is_none(), "a dead client must be discarded");
}

fn accept_startup(stream: &mut TcpStream) -> Vec<u8> {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).expect("read startup length");
    let body_len = usize::try_from(u32::from_be_bytes(len).saturating_sub(4))
        .expect("the startup body length fits usize");
    let mut body = vec![0_u8; body_len];
    stream.read_exact(&mut body).expect("read startup body");
    write_backend(stream, b'R', &0_i32.to_be_bytes());
    write_backend(stream, b'Z', b"I");
    stream.flush().expect("flush startup response");
    body
}

fn read_frontend(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut tag = [0_u8; 1];
    stream.read_exact(&mut tag).expect("read frontend tag");
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).expect("read frontend length");
    let body_len = usize::try_from(u32::from_be_bytes(len).saturating_sub(4))
        .expect("the frontend body length fits usize");
    let mut body = vec![0_u8; body_len];
    stream.read_exact(&mut body).expect("read frontend body");
    (tag[0], body)
}

fn frontend_sql(body: &[u8]) -> &str {
    let sql = body.strip_suffix(&[0]).expect("frontend SQL is terminated");
    std::str::from_utf8(sql).expect("frontend SQL is UTF-8")
}

fn write_command_ready(stream: &mut TcpStream, command: &str) {
    let mut body = command.as_bytes().to_vec();
    body.push(0);
    write_backend(stream, b'C', &body);
    write_backend(stream, b'Z', b"I");
    stream.flush().expect("flush command response");
}

fn write_backend(stream: &mut TcpStream, tag: u8, body: &[u8]) {
    stream.write_all(&[tag]).expect("write backend tag");
    let len = u32::try_from(body.len() + 4).expect("backend message fits");
    stream
        .write_all(&len.to_be_bytes())
        .expect("write backend length");
    stream.write_all(body).expect("write backend body");
}
