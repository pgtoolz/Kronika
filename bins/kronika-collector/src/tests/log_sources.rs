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
use kronika_source_log::postgres::LinePrefix;
use kronika_source_log::{Offsets, Position};
use kronika_writer::{Journal, JournalConfig, SectionBuffers};

use crate::scheduler::{DueSet, SourceKind};

use super::{LogSources, PostgresTarget, parse_connections};

#[derive(Clone)]
struct LogFacts {
    path: String,
    prefix: &'static str,
    timezone: &'static str,
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
                            query.contains("log_line_prefix"),
                            "unexpected simple query: {query}"
                        );
                        match facts.pop_front().expect("log facts reply") {
                            Reply::Value(facts) => write_row(
                                &mut stream,
                                &[
                                    ("user_name", Some("monitor".to_owned())),
                                    ("database_name", Some("postgres".to_owned())),
                                    ("line_prefix", Some(facts.prefix.to_owned())),
                                    ("log_timezone", Some(facts.timezone.to_owned())),
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
        discover_postgres_paths: true,
        offsets: Offsets::load(root).expect("load offsets"),
        pg_dsn: Some(PostgresTarget::new(connection).expect("load PostgreSQL transport")),
        pg_logs: Vec::new(),
        pg_log_max_lag_secs: 900,
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
        timezone: "GMT",
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
        discover_postgres_paths: true,
        offsets: Offsets::load(root).expect("load offsets"),
        pg_dsn: None,
        pg_logs: Vec::new(),
        pg_log_max_lag_secs: 900,
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

    let parsed = parse_connections("KRONIKA_PGBOUNCER_DSNS", &configured)
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

    let error = parse_connections("KRONIKA_PGBOUNCER_DSNS", &configured)
        .expect_err("the second connection is invalid");
    let message = format!("{error:#}");

    assert_eq!(
        message,
        "KRONIKA_PGBOUNCER_DSNS[1] is not a valid connection string"
    );
    for secret in [raw, "RAW_SECRET", "PRIVATE_DATABASE"] {
        assert!(!message.contains(secret));
    }
}

#[tokio::test]
async fn configured_postgres_log_discovery_never_attempts_an_ignored_legacy_target() {
    const CHILD: &str = "KRONIKA_TEST_SINGLE_DSN_LOG_CHILD";
    if let Ok(expected) = std::env::var(CHILD) {
        let config = crate::config::Config::from_env().expect("normalized collector config");
        let _metrics =
            crate::pg_sources::PgSources::open(&config).expect("same selected metrics DSN");
        let mut sources = LogSources::open(&config).expect("open normalized log target");
        sources.rescan(&mut |_| {}).await;
        if expected == "success" {
            assert_eq!(sources.postgres.len(), 1);
            assert_eq!(sources.postgres[0].system_identifier, Some(777));
        } else {
            assert!(sources.postgres.is_empty());
        }
        return;
    }

    for (variable, succeeds) in [
        ("KRONIKA_PG_DSN", true),
        ("KRONIKA_PG_DSNS", true),
        ("KRONIKA_PG_DSNS", false),
    ] {
        let directory = tempfile::tempdir().expect("routing fixture");
        let path = directory.path().join("selected.log");
        std::fs::write(&path, "").expect("selected log file");
        let server = if succeeds {
            FakePostgres::start(vec![facts(&path, "%m ")], vec![Reply::Value(777)])
        } else {
            FakePostgres::start(vec![Reply::Error], vec![])
        };
        let ignored = TcpListener::bind(("127.0.0.1", 0)).expect("ignored legacy target");
        ignored
            .set_nonblocking(true)
            .expect("check without waiting");
        let configured = if variable == "KRONIKA_PG_DSN" {
            format!("{} password='one;complete;value'", server.dsn)
        } else {
            format!(
                "{};host=127.0.0.1 port={} user=monitor sslmode=disable;;host='unterminated",
                server.dsn,
                ignored.local_addr().expect("ignored address").port()
            )
        };
        let mut command = std::process::Command::new(std::env::current_exe().expect("test binary"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KRONIKA_") {
                command.env_remove(name);
            }
        }
        let output = command
            .args([
                "--exact",
                "log_sources::tests::configured_postgres_log_discovery_never_attempts_an_ignored_legacy_target",
                "--nocapture",
            ])
            .env(CHILD, if succeeds { "success" } else { "failure" })
            .env("KRONIKA_STORAGE_DIR", directory.path())
            .env(variable, configured)
            .output()
            .expect("isolated production routing check");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(query_counts(&server.finish()), (1, usize::from(succeeds)));
        assert!(
            matches!(ignored.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "ignored target receives no connection, including when selected discovery fails"
        );
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

    assert_eq!(
        sources
            .pg_dsn
            .as_ref()
            .expect("selected target")
            .system_identifier,
        Some(42)
    );
    assert_eq!(
        sources
            .pg_dsn
            .as_ref()
            .expect("selected target")
            .facts
            .line_prefix
            .as_deref(),
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
    assert_eq!(
        sources
            .pg_dsn
            .as_ref()
            .expect("selected target")
            .system_identifier,
        None
    );
    assert_eq!(sources.postgres[0].system_identifier, None);

    sources.rescan_postgres(&mut observe).await;
    assert_eq!(
        sources
            .pg_dsn
            .as_ref()
            .expect("selected target")
            .system_identifier,
        Some(43)
    );
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

    assert_eq!(
        sources
            .pg_dsn
            .as_ref()
            .expect("selected target")
            .system_identifier,
        Some(44)
    );
    assert_eq!(sources.postgres.len(), 1);
    assert_eq!(sources.postgres[0].log.path(), path);
    assert_eq!(sources.postgres[0].system_identifier, Some(44));
    assert_eq!(query_counts(&server.finish()), (2, 1));
}

#[test]
fn earlier_offsets_are_saved_when_a_later_file_is_rejected_or_fails() {
    for fatal in [false, true] {
        let dir = tempfile::tempdir().expect("log fixture");
        let paths = ["a.log", "b.log", "c.log"].map(|name| dir.path().join(name));
        let first = pgbouncer_line("kernel file descriptor limit: 1024");
        for path in &paths {
            std::fs::write(
                path,
                format!("{first}{}", pgbouncer_line("unrecognized sentinel")),
            )
            .expect("write log");
        }
        let mut logs = sources(dir.path(), paths[0].clone());
        logs.pgbouncer.extend(
            paths[1..]
                .iter()
                .map(|path| PgBouncerLog::new(path.clone(), Position::default())),
        );
        let mut admission_paths = Vec::new();

        let result = logs.collect(&DueSet::logs(), |rows| {
            assert!(rows.postgres.is_empty());
            assert_eq!(rows.pgbouncer.len(), 1);
            let batch = &rows.pgbouncer[0];
            assert_eq!(batch.events.len(), 1);
            admission_paths.push(batch.source_file.clone());
            if batch.source_file == paths[1].display().to_string() {
                if fatal {
                    anyhow::bail!("downstream append failed");
                }
                return Ok(false);
            }
            Ok(true)
        });

        if fatal {
            assert_eq!(
                result.expect_err("fatal admission failure").to_string(),
                "downstream append failed"
            );
        } else {
            assert!(!result.expect("recoverable admission rejection"));
        }
        assert_eq!(
            admission_paths,
            paths[..2]
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>(),
            "collection stops before the third file"
        );
        let committed = logs.pgbouncer[0].position();
        assert_eq!(committed.offset, first.len() as u64);
        let saved = Offsets::load(dir.path()).expect("offsets saved before returning");
        assert_eq!(saved.get(&paths[0].display().to_string()), committed);
        for log in &logs.pgbouncer[1..] {
            assert_eq!(log.position().offset, 0, "unacknowledged file");
            assert_eq!(saved.get(&log.path().display().to_string()).offset, 0);
        }
        assert_eq!(
            logs.pgbouncer[2].position(),
            Position::default(),
            "the third file was never read"
        );
    }
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
        .collect(&due, |rows| {
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
            .get(&path.display().to_string())
            .offset,
        0
    );

    let mut replayed = Vec::new();
    assert!(
        sources
            .collect(&due, |rows| {
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
            .get(&path.display().to_string()),
        committed
    );
}

#[tokio::test]
async fn postgresql_mode_follows_only_explicit_paths_when_settings_are_unavailable() {
    let dir = tempfile::tempdir().expect("log fixture");
    let explicit = dir.path().join("explicit.log");
    let unrelated = dir.path().join("remote-same-name.log");
    std::fs::write(&explicit, b"").expect("explicit file");
    std::fs::write(&unrelated, b"").expect("unrelated local file");
    let mut sources = postgres_sources(
        dir.path(),
        "host=127.0.0.1 port=1 user=monitor dbname=postgres sslmode=disable",
    );
    sources.discover_postgres_paths = false;
    sources
        .pg_logs
        .push(explicit.to_string_lossy().into_owned());
    sources.pg_dsn.as_mut().expect("selected target").last_log = Some(unrelated);
    let mut observations = Vec::new();
    sources
        .rescan_postgres(&mut |observation| observations.push(observation))
        .await;
    assert!(
        !observations.is_empty(),
        "settings were requested for the explicit log"
    );
    assert_eq!(sources.postgres.len(), 1);
    assert_eq!(sources.postgres[0].log.path(), explicit);
    assert!(sources.postgres[0].system_identifier.is_none());
}

#[tokio::test]
async fn explicit_postgres_logs_receive_and_refresh_the_servers_timezone() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("explicit.log");
    std::fs::write(
        &path,
        "2026-09-14 10:13:00 GMT ERROR:  first error\nignored\n",
    )
    .expect("log");
    let server = FakePostgres::start(
        vec![
            Reply::Value(LogFacts {
                path: "/inaccessible/server.log".to_owned(),
                prefix: "%t ",
                timezone: "GMT",
            }),
            Reply::Value(LogFacts {
                path: "/inaccessible/server.log".to_owned(),
                prefix: "%m ",
                timezone: "America/New_York",
            }),
            Reply::Error,
        ],
        vec![Reply::Value(42)],
    );
    let mut sources = postgres_sources(dir.path(), &server.dsn);
    sources.discover_postgres_paths = false;
    sources.pg_logs.push(path.display().to_string());
    sources.rescan_postgres(&mut |_| {}).await;
    assert_eq!(sources.postgres.len(), 1);
    let log = &mut sources.postgres[0].log;
    let batch = log.read_batch(|| Ok(0), 100, 900).expect("GMT record");
    assert_eq!(batch.events.errors[0].ts, 1_789_380_780_000_000);
    log.acknowledge().expect("commit GMT record");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append log");
    writeln!(
        file,
        "2026-09-14 06:13:01.789 EDT ERROR:  second error\nignored"
    )
    .expect("new record");
    sources.rescan_postgres(&mut |_| {}).await;
    sources.rescan_postgres(&mut |_| {}).await;
    assert_eq!(sources.postgres.len(), 1);
    let batch = sources.postgres[0]
        .log
        .read_batch(|| Ok(0), 100, 900)
        .expect("cached timezone after failed refresh");
    assert_eq!(batch.events.errors[0].ts, 1_789_380_781_789_000);
    let queries = server.finish();
    assert_eq!(query_counts(&queries), (3, 1));
    assert!(
        queries
            .iter()
            .all(|query| !query.contains("pg_current_logfile")
                && !query.contains("current_setting('data_directory')"))
    );
}

#[test]
fn old_pg_records_advance_offsets_without_bypassing_mixed_batch_admission() {
    for mixed in [false, true] {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("postgresql.log");
        let now = crate::clock::unix_now_us().expect("system clock") / 1_000_000;
        let old = format!("{}.000 ERROR:  old error\n", now - 3_600);
        let current = format!("{now}.000 ERROR:  current error\n");
        let records = if mixed {
            format!("{old}{current}")
        } else {
            old
        };
        std::fs::write(&path, format!("{records}{now}.000 INFO:  sentinel\n")).expect("write log");
        let mut logs = sources(dir.path(), path.clone());
        logs.pgbouncer.clear();
        logs.postgres.push(super::PostgresSource {
            log: super::PgLog::new(
                path.clone(),
                Position::default(),
                Some(LinePrefix::parse("%n ")),
            ),
            system_identifier: None,
        });
        let due = DueSet::logs();
        let completed = logs
            .collect(&due, |rows| {
                assert!(mixed, "all-old batch never needs row admission");
                assert_eq!(rows.postgres[0].events.errors.len(), 1);
                assert_eq!(rows.postgres[0].events.errors[0].sample, "current error");
                Ok(false)
            })
            .expect("read log");
        assert_eq!(completed, !mixed);
        if mixed {
            assert_eq!(logs.postgres[0].log.position().offset, 0);
            assert!(logs.collect(&due, |_| Ok(true)).expect("admit retry"));
        }
        let committed = logs.postgres[0].log.position();
        assert_eq!(committed.offset, records.len() as u64);
        assert_eq!(
            Offsets::load(dir.path())
                .expect("saved offsets")
                .get(&path.display().to_string()),
            committed
        );
    }
}
