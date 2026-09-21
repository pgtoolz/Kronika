//! Reading a `PgBouncer` log.
//!
//! The line layout is fixed, unlike `PostgreSQL`'s: `lib/usual/logging.c:231`
//! writes `<time> [<pid>] <LEVEL> <message>`, and `src/util.c:40` puts a socket
//! context in front of the message when the line belongs to a connection.

use std::io;
use std::path::{Path, PathBuf};

use crate::tail::{Position, Record, Tail};
use crate::text::{MAX_TEXT_BYTES, bounded, truncate};
use crate::timestamp;

/// The severity `PgBouncer` gave a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// `FATAL`.
    Fatal,
    /// `ERROR`.
    Error,
    /// `WARNING`.
    Warning,
    /// `LOG`, which both `LG_STATS` and `LG_INFO` print.
    Log,
    /// `DEBUG`, printed only at `verbose >= 1`.
    Debug,
    /// `NOISE`, printed only at `verbose >= 2`.
    Noise,
}

impl Level {
    /// The code stored in `pgbouncer_events.level`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Fatal => 0,
            Self::Error => 1,
            Self::Warning => 2,
            Self::Log => 3,
            Self::Debug => 4,
            Self::Noise => 5,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "FATAL" => Some(Self::Fatal),
            "ERROR" => Some(Self::Error),
            "WARNING" => Some(Self::Warning),
            "LOG" => Some(Self::Log),
            "DEBUG" => Some(Self::Debug),
            "NOISE" => Some(Self::Noise),
            _ => None,
        }
    }
}

/// One recognized `PgBouncer` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Line time, unix microseconds.
    pub ts: i64,
    /// Severity.
    pub level: Level,
    /// The `pgbouncer.ini` section the connection used, not the `dbname` it
    /// resolves to (`src/util.c:52`).
    pub database: Option<String>,
    /// The login user.
    pub username: Option<String>,
    /// The client or server address, without the port.
    pub host: Option<String>,
    /// Pooler process ID, when present.
    pub pid: Option<i32>,
    /// `C` for a client connection, `S` for a server connection.
    pub side: Option<String>,
    /// Connection port, including zero for a Unix socket.
    pub port: Option<u16>,
    /// Connection age in whole seconds, when printed.
    pub age_s: Option<u64>,
    /// Complete bounded message, including closing reason and age.
    pub text: String,
}

/// A followed `PgBouncer` log file.
#[derive(Debug)]
pub struct PgBouncerLog {
    tail: Tail,
}

/// One bounded read from a followed `PgBouncer` log.
#[derive(Debug)]
pub struct ReadBatch {
    /// Recognized events from complete records.
    pub events: Vec<Event>,
    /// Raw file bytes read while producing this batch.
    pub raw_bytes: usize,
    /// Whether the volatile scan cursor reached the observed end of file.
    pub at_eof: bool,
    /// Whether input completed by this batch awaits acknowledgement.
    pub needs_ack: bool,
}

impl PgBouncerLog {
    /// Follow `path`, resuming from `position`.
    #[must_use]
    pub const fn new(path: PathBuf, position: Position) -> Self {
        Self {
            tail: Tail::new(path, position),
        }
    }

    /// The path being followed.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.tail.path()
    }

    /// The offset to resume from after a restart.
    #[must_use]
    pub const fn position(&self) -> Position {
        self.tail.position()
    }

    /// Read and recognize one bounded batch written since the last call.
    ///
    /// # Errors
    ///
    /// Returns a file or clock error.
    pub fn read_batch(
        &mut self,
        now: impl FnOnce() -> io::Result<i64>,
        max_records: usize,
    ) -> io::Result<ReadBatch> {
        let batch = self
            .tail
            .read_batch_without_quote_tracking(continues, max_records)?;
        let now = now().inspect_err(|_error| self.tail.retry())?;
        Ok(ReadBatch {
            events: batch
                .records
                .iter()
                .filter_map(|record| parse(record, now))
                .collect(),
            raw_bytes: batch.raw_bytes,
            at_eof: batch.at_eof,
            needs_ack: batch.needs_ack,
        })
    }

    /// Commit the candidate position from the last completed batch.
    ///
    /// The collector calls this only after the batch's rows have reached the
    /// active journal, or immediately when the batch contains no durable rows.
    pub fn acknowledge(&mut self) -> Option<Position> {
        self.tail.acknowledge()
    }

    /// Discard unacknowledged volatile progress so the same input is read again.
    pub fn retry(&mut self) {
        self.tail.retry();
    }
}

/// A message with a newline in it is written as `\n\t`
/// (`lib/usual/logging.c:177`), so a line starting with a tab continues the
/// line before it.
fn continues(_open: &[String], line: &str, _raw_quotes_odd: bool) -> bool {
    line.starts_with('\t')
}

/// Read one line, or `None` when it is not an event this collector records.
#[must_use]
pub fn parse(record: &Record, now: i64) -> Option<Event> {
    let first = record.first();
    let timestamp = timestamp::parse_local(first);
    let rest = timestamp.map_or(first, |(_, rest)| rest).trim_start();
    let (pid, rest) = if rest
        .split_once(' ')
        .is_some_and(|(level, _)| Level::parse(level).is_some())
    {
        (None, rest)
    } else if let Some((prefix, rest)) = rest.split_once("] ") {
        let (_, pid) = prefix.rsplit_once('[')?;
        (pid.parse().ok(), rest)
    } else {
        (None, rest)
    };
    let (level, message) = rest.split_once(' ')?;
    let level = Level::parse(level)?;
    if matches!(level, Level::Debug | Level::Noise) {
        return None;
    }
    let (context, message) = split_socket_context(message);
    if level == Level::Log && routine(message) {
        return None;
    }
    let mut text = truncate(message.trim(), MAX_TEXT_BYTES).to_owned();
    for line in record.rest() {
        crate::text::append_str(&mut text, line);
    }
    if text.is_empty() {
        return None;
    }
    Some(Event {
        ts: timestamp.map_or(now, |(ts, _)| ts),
        level,
        database: context.database,
        username: context.username,
        host: context.host,
        pid,
        side: context.side,
        port: context.port,
        age_s: closing(message).and_then(|(_, age)| age),
        text,
    })
}

/// What the socket context in front of a message carried.
#[derive(Debug, Default)]
struct SocketContext {
    database: Option<String>,
    username: Option<String>,
    host: Option<String>,
    side: Option<String>,
    port: Option<u16>,
}

/// Split `C-0x55f1: db/user@10.0.0.1:41537 ` off the front of a message.
///
/// Lines from `janitor.c`, `main.c` and `pooler.c` carry no socket, so the
/// whole message is returned untouched.
fn split_socket_context(message: &str) -> (SocketContext, &str) {
    let none = (SocketContext::default(), message);
    let Some(rest) = message
        .strip_prefix("C-")
        .or_else(|| message.strip_prefix("S-"))
    else {
        return none;
    };
    let Some(at) = rest.find(": ") else {
        return none;
    };
    let rest = rest.get(at + ": ".len()..).unwrap_or_default();
    let Some(end) = rest.find(' ') else {
        return none;
    };
    let (peer, tail) = (
        rest.get(..end).unwrap_or_default(),
        rest.get(end + 1..).unwrap_or_default(),
    );
    let Some(at) = peer.rfind('@') else {
        return none;
    };
    let (who, address) = (
        peer.get(..at).unwrap_or_default(),
        peer.get(at + 1..).unwrap_or_default(),
    );
    // `(nodb)`, `(nouser)` and `peer-7` are the literals PgBouncer substitutes;
    // they are kept as they stand rather than turned into a missing value.
    let (database, username) = match who.split_once('/') {
        Some((database, username)) => (bounded(database), bounded(username)),
        None => (None, bounded(who)),
    };
    let (host, port) = address
        .rsplit_once(':')
        .filter(|(host, _)| !host.contains(':') || (host.starts_with('[') && host.ends_with(']')))
        .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, Some(port))))
        .unwrap_or((address, None));
    (
        SocketContext {
            database,
            username,
            host: bounded(host),
            side: message.get(..1).map(str::to_owned),
            port,
        },
        tail,
    )
}

fn closing(message: &str) -> Option<(&str, Option<u64>)> {
    let reason = message.strip_prefix("closing because: ")?;
    let Some((reason, age)) = reason.rsplit_once(" (age=") else {
        return Some((reason, None));
    };
    let age = age.strip_suffix("s)")?;
    if !age.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let age = age.parse().ok()?;
    Some((reason, Some(age)))
}

fn routine(message: &str) -> bool {
    if closing(message).is_some_and(|(reason, _)| {
        matches!(
            reason,
            "client close request" | "server idle timeout" | "server lifetime over"
        )
    }) {
        return true;
    }
    if message == "new connection to server"
        || message
            .strip_prefix("new connection to server (from ")
            .and_then(|address| address.strip_suffix(')'))
            .is_some_and(|address| !address.is_empty() && !address.contains(['(', ')']))
    {
        return true;
    }
    if let Some(login) = message.strip_prefix("login attempt: db=") {
        return login.split_once(" user=").is_some_and(|(db, rest)| {
            !db.is_empty()
                && rest
                    .split_once(" tls=")
                    .is_some_and(|(user, tls)| !user.is_empty() && !tls.is_empty())
        });
    }
    let Some(stats) = message.strip_prefix("stats: ") else {
        return false;
    };
    if !stats.split_once(" xacts/s, ").is_some_and(|(count, rest)| {
        count.parse::<u64>().is_ok()
            && rest
                .split_once(" queries/s,")
                .is_some_and(|(count, _)| count.parse::<u64>().is_ok())
    }) {
        return false;
    }
    stats.split(", ").all(|field| {
        let Some((value, unit)) = field.split_once(' ') else {
            return false;
        };
        if matches!(value, "in" | "out" | "xact" | "query" | "wait") {
            let suffix = if matches!(value, "in" | "out") {
                " B/s"
            } else {
                " us"
            };
            return unit
                .strip_suffix(suffix)
                .is_some_and(|count| count.parse::<u64>().is_ok());
        }
        value.parse::<u64>().is_ok()
            && matches!(
                unit,
                "xacts/s"
                    | "queries/s"
                    | "client parses/s"
                    | "server parses/s"
                    | "binds/s"
                    | "client logins/s"
                    | "in B/s"
                    | "out B/s"
                    | "xact us"
                    | "query us"
                    | "wait us"
            )
    })
}
