use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use kronika_layout::{DataRoot, LayoutLimits, SegmentId};
use kronika_registry::Ts;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_source_log::pgbouncer::PgBouncerLog;
use kronika_source_log::{Offsets, Position};
use kronika_writer::{Journal, JournalConfig, SectionBuffers};

use crate::scheduler::{DueSet, SourceKind};

use super::{
    LogSources, MAX_READ_BYTES, MAX_SOURCE_READ_BYTES, PostgresTarget, key, next_batch_bytes,
    parse_connections,
};

#[derive(Clone)]
struct LogFacts {
    path: String,
    prefix: &'static str,
}

enum Reply<T> {
    Value(T),
    Error,
}

struct FakePostgres {
    dsn: String,
    queries: Arc<Mutex<Vec<String>>>,
    thread: thread::JoinHandle<()>,
}

impl FakePostgres {
    fn start(facts: Vec<Reply<LogFacts>>, identities: Vec<Reply<i64>>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake PostgreSQL");
        let address = listener.local_addr().expect("fake PostgreSQL address");
        let connections = facts.len();
        let queries = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&queries);
        let thread = thread::spawn(move || {
            let mut facts = VecDeque::from(facts);
            let mut identities = VecDeque::from(identities);
            for _ in 0..connections {
                let (mut stream, _) = listener.accept().expect("accept PostgreSQL client");
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("set fake PostgreSQL timeout");
                read_startup(&mut stream);
                write_backend(&mut stream, b'R', &0_u32.to_be_bytes());
                write_backend(&mut stream, b'Z', b"I");
                stream.flush().expect("flush startup response");
                let mut configured = false;
                while let Some(query) = read_query(&mut stream) {
                    if !configured {
                        assert!(
                            query.contains("SET statement_timeout = '30s'")
                                && query.contains("SET lock_timeout = '100ms'"),
                            "the session timeout must precede monitoring queries"
                        );
                        write_backend(&mut stream, b'C', b"SET\0");
                        write_backend(&mut stream, b'C', b"SET\0");
                        write_backend(&mut stream, b'Z', b"I");
                        stream.flush().expect("flush session setup response");
                        configured = true;
                        continue;
                    }
                    recorded
                        .lock()
                        .expect("lock recorded queries")
                        .push(query.clone());
                    if query.contains("pg_control_system") {
                        match identities.pop_front().expect("identity reply") {
                            Reply::Value(identifier) => write_row(
                                &mut stream,
                                &[("system_identifier", Some(identifier.to_string()))],
                            ),
                            Reply::Error => write_error(&mut stream),
                        }
                    } else {
                        assert!(
                            query.contains("pg_current_logfile"),
                            "unexpected simple query: {query}"
                        );
                        match facts.pop_front().expect("log facts reply") {
                            Reply::Value(facts) => write_row(
                                &mut stream,
                                &[
                                    ("user_name", Some("monitor".to_owned())),
                                    ("database_name", Some("postgres".to_owned())),
                                    ("line_prefix", Some(facts.prefix.to_owned())),
                                    ("data_directory", Some("/unused".to_owned())),
                                    ("log_path", Some(facts.path)),
                                ],
                            ),
                            Reply::Error => write_error(&mut stream),
                        }
                    }
                    stream.flush().expect("flush query response");
                }
                assert!(configured, "the frontend session must be configured");
            }
            assert!(facts.is_empty(), "unused log facts replies");
            assert!(identities.is_empty(), "unused identity replies");
        });
        Self {
            dsn: format!(
                "host=127.0.0.1 port={} user=monitor dbname=postgres sslmode=disable",
                address.port()
            ),
            queries,
            thread,
        }
    }

    fn finish(self) -> Vec<String> {
        self.thread.join().expect("fake PostgreSQL thread");
        Arc::try_unwrap(self.queries)
            .expect("one query recorder owner")
            .into_inner()
            .expect("unlock recorded queries")
    }
}

fn read_startup(stream: &mut TcpStream) {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).expect("read startup length");
    let length = u32::from_be_bytes(length) as usize;
    let mut body = vec![0_u8; length.checked_sub(4).expect("valid startup length")];
    stream.read_exact(&mut body).expect("read startup body");
}

fn read_query(stream: &mut TcpStream) -> Option<String> {
    let mut tag = [0_u8; 1];
    match stream.read_exact(&mut tag) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
            ) =>
        {
            return None;
        }
        Err(error) => panic!("read frontend tag: {error}"),
    }
    let mut length = [0_u8; 4];
    stream
        .read_exact(&mut length)
        .expect("read frontend message length");
    let length = u32::from_be_bytes(length) as usize;
    assert!(length >= 4, "valid frontend length");
    let mut body = vec![0_u8; length - 4];
    stream
        .read_exact(&mut body)
        .expect("read frontend message body");
    if tag[0] == b'X' {
        return None;
    }
    assert_eq!(tag[0], b'Q', "only Simple Query Protocol is allowed");
    assert_eq!(body.pop(), Some(0), "query is null terminated");
    Some(String::from_utf8(body).expect("query is UTF-8"))
}

fn write_backend(stream: &mut TcpStream, tag: u8, body: &[u8]) {
    stream.write_all(&[tag]).expect("write backend tag");
    let length = u32::try_from(body.len() + 4).expect("backend response length");
    stream
        .write_all(&length.to_be_bytes())
        .expect("write backend length");
    stream.write_all(body).expect("write backend body");
}

fn write_row(stream: &mut TcpStream, fields: &[(&str, Option<String>)]) {
    let count = i16::try_from(fields.len()).expect("field count");
    let mut description = Vec::new();
    description.extend_from_slice(&count.to_be_bytes());
    for (name, _) in fields {
        description.extend_from_slice(name.as_bytes());
        description.push(0);
        description.extend_from_slice(&0_u32.to_be_bytes());
        description.extend_from_slice(&0_i16.to_be_bytes());
        description.extend_from_slice(&25_u32.to_be_bytes());
        description.extend_from_slice(&(-1_i16).to_be_bytes());
        description.extend_from_slice(&(-1_i32).to_be_bytes());
        description.extend_from_slice(&0_i16.to_be_bytes());
    }
    write_backend(stream, b'T', &description);

    let mut row = Vec::new();
    row.extend_from_slice(&count.to_be_bytes());
    for (_, value) in fields {
        match value {
            Some(value) => {
                let length = i32::try_from(value.len()).expect("field length");
                row.extend_from_slice(&length.to_be_bytes());
                row.extend_from_slice(value.as_bytes());
            }
            None => row.extend_from_slice(&(-1_i32).to_be_bytes()),
        }
    }
    write_backend(stream, b'D', &row);
    write_backend(stream, b'C', b"SELECT 1\0");
    write_backend(stream, b'Z', b"I");
}

fn write_error(stream: &mut TcpStream) {
    let mut body = Vec::new();
    for (field, value) in [(b'S', "ERROR"), (b'C', "42501"), (b'M', "denied")] {
        body.push(field);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    write_backend(stream, b'E', &body);
    write_backend(stream, b'Z', b"I");
}

fn postgres_sources(root: &std::path::Path, dsn: &str) -> LogSources {
    let connection =
        super::settings::ConnectionTarget::parse(dsn, 0).expect("parse fake connection");
    LogSources {
        offsets: Offsets::load(root).expect("load offsets"),
        pg_dsns: vec![PostgresTarget::new(connection)],
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
        postgres: Vec::new(),
        pgbouncer: Vec::new(),
        next_scan: None,
    }
}

fn facts(path: &std::path::Path, prefix: &'static str) -> Reply<LogFacts> {
    Reply::Value(LogFacts {
        path: path.display().to_string(),
        prefix,
    })
}

fn query_counts(queries: &[String]) -> (usize, usize) {
    let identities = queries
        .iter()
        .filter(|query| query.contains("pg_control_system"))
        .count();
    (queries.len() - identities, identities)
}

fn pgbouncer_line(message: &str) -> String {
    format!("2026-08-07 12:34:56.789 MSK [12345] ERROR {message}\n")
}

fn sources(root: &std::path::Path, path: std::path::PathBuf) -> LogSources {
    LogSources {
        offsets: Offsets::load(root).expect("load offsets"),
        pg_dsns: Vec::new(),
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
        postgres: Vec::new(),
        pgbouncer: vec![PgBouncerLog::new(path, Position::default())],
        next_scan: None,
    }
}

fn one_wal_part() -> Vec<u8> {
    let mut buffers = SectionBuffers::new();
    buffers
        .push(OsLoadavg {
            ts: Ts(1),
            load1: 1.0,
            load5: 1.0,
            load15: 1.0,
            running: 1,
            total: 1,
            scope: 0,
        })
        .expect("buffer one row");
    buffers
        .flush(&[])
        .expect("encode one row")
        .expect("one row yields a part")
}

#[test]
fn configured_connections_retain_no_raw_dsn_or_secret() {
    let raw = "postgresql://monitor:RAW_SECRET@db.example:6432/PRIVATE_DATABASE";
    let configured = vec![raw.to_owned()];

    let parsed = parse_connections("KRONIKA_PG_DSNS", &configured)
        .expect("the configured connection parses");

    assert_eq!(parsed.len(), 1);
    let debug = format!("{:?}", parsed[0]);
    assert!(debug.contains("monitor@db.example:6432"));
    for secret in [raw, "RAW_SECRET", "PRIVATE_DATABASE"] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn invalid_connection_error_contains_only_variable_and_index() {
    let raw = "host='unterminated password=RAW_SECRET dbname=PRIVATE_DATABASE";
    let configured = vec!["host=db.example user=monitor".to_owned(), raw.to_owned()];

    let error = parse_connections("KRONIKA_PG_DSNS", &configured)
        .expect_err("the second connection is invalid");
    let message = format!("{error:#}");

    assert_eq!(
        message,
        "KRONIKA_PG_DSNS[1] is not a valid connection string"
    );
    for secret in [raw, "RAW_SECRET", "PRIVATE_DATABASE"] {
        assert!(!message.contains(secret));
    }
}

#[tokio::test]
async fn two_successful_rescans_read_identity_once_and_refresh_log_facts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    std::fs::write(&path, "").expect("create PostgreSQL log");
    let server = FakePostgres::start(
        vec![facts(&path, "%m "), facts(&path, "%t ")],
        vec![Reply::Value(42)],
    );
    let mut sources = postgres_sources(dir.path(), &server.dsn);
    let mut observe = |_observation| {};

    sources.rescan_postgres(&mut observe).await;
    sources.rescan_postgres(&mut observe).await;

    assert_eq!(sources.pg_dsns[0].system_identifier, Some(42));
    assert_eq!(
        sources.pg_dsns[0]
            .last_log
            .as_ref()
            .map(|(_, prefix)| prefix.as_str()),
        Some("%t ")
    );
    assert_eq!(sources.postgres[0].system_identifier, Some(42));
    assert_eq!(query_counts(&server.finish()), (2, 1));
}

#[tokio::test]
async fn failed_first_identity_read_is_retried_on_the_next_rescan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    std::fs::write(&path, "").expect("create PostgreSQL log");
    let server = FakePostgres::start(
        vec![facts(&path, "%m "), facts(&path, "%m ")],
        vec![Reply::Error, Reply::Value(43)],
    );
    let mut sources = postgres_sources(dir.path(), &server.dsn);
    let mut observe = |_observation| {};

    sources.rescan_postgres(&mut observe).await;
    assert_eq!(sources.pg_dsns[0].system_identifier, None);
    assert_eq!(sources.postgres[0].system_identifier, None);

    sources.rescan_postgres(&mut observe).await;
    assert_eq!(sources.pg_dsns[0].system_identifier, Some(43));
    assert_eq!(sources.postgres[0].system_identifier, Some(43));
    assert_eq!(query_counts(&server.finish()), (2, 2));
}

#[tokio::test]
async fn cached_identity_and_followed_source_survive_a_later_refresh_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    std::fs::write(&path, "").expect("create PostgreSQL log");
    let server = FakePostgres::start(
        vec![facts(&path, "%m "), Reply::Error],
        vec![Reply::Value(44)],
    );
    let mut sources = postgres_sources(dir.path(), &server.dsn);
    let mut observe = |_observation| {};

    sources.rescan_postgres(&mut observe).await;
    sources.rescan_postgres(&mut observe).await;

    assert_eq!(sources.pg_dsns[0].system_identifier, Some(44));
    assert_eq!(sources.postgres.len(), 1);
    assert_eq!(sources.postgres[0].log.path(), path);
    assert_eq!(sources.postgres[0].system_identifier, Some(44));
    assert_eq!(query_counts(&server.finish()), (2, 1));
}

#[test]
fn each_file_gets_an_independent_256_mib_budget_in_4_mib_batches() {
    fn consume_budget() -> (usize, usize) {
        let mut read = 0_usize;
        let mut batches = 0_usize;
        while next_batch_bytes(read) != 0 {
            let batch = next_batch_bytes(read);
            assert!(batch <= MAX_READ_BYTES);
            read += batch;
            batches += 1;
        }
        (read, batches)
    }

    assert_eq!(consume_budget(), (MAX_SOURCE_READ_BYTES, 64));
    assert_eq!(consume_budget(), (MAX_SOURCE_READ_BYTES, 64));
    assert_eq!(
        next_batch_bytes(MAX_SOURCE_READ_BYTES - 1),
        0,
        "a full batch is not started when it could cross the ceiling"
    );
}

#[test]
fn wal_append_precedes_offset_ack_and_a_retry_replays_the_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    let first = pgbouncer_line("kernel file descriptor limit: 1024");
    std::fs::write(
        &path,
        format!("{first}{}", pgbouncer_line("unrecognized sentinel")),
    )
    .expect("write log");
    let due = DueSet::for_test(vec![SourceKind::Logs]);
    let mut sources = sources(dir.path(), path.clone());

    let root = DataRoot::open(dir.path()).expect("open data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let body = one_wal_part();
    let completed = sources
        .collect(&due, 0, |rows| {
            assert_eq!(rows.pgbouncer[0].events.len(), 1);
            journal
                .append(SegmentId::new(1).expect("segment id"), &body)
                .expect("append and sync WAL");
            Ok(false)
        })
        .expect("recoverable downstream failure");

    assert!(!completed);
    assert_eq!(journal.parts().len(), 1, "the WAL append is durable");
    assert_eq!(sources.pgbouncer[0].position().offset, 0);
    assert_eq!(
        Offsets::load(dir.path())
            .expect("reload offsets")
            .get(&key(&path))
            .offset,
        0
    );

    let mut replayed = Vec::new();
    assert!(
        sources
            .collect(&due, 0, |rows| {
                replayed.push(rows.pgbouncer[0].events[0].text.clone());
                Ok(true)
            })
            .expect("retry succeeds")
    );
    assert_eq!(replayed, ["kernel file descriptor limit: 1024"]);
    let committed = sources.pgbouncer[0].position();
    assert_eq!(committed.offset, first.len() as u64);
    assert_eq!(
        Offsets::load(dir.path())
            .expect("reload committed offsets")
            .get(&key(&path)),
        committed
    );
}
