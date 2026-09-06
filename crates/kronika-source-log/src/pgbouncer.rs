//! Reading a `PgBouncer` log.
//!
//! The line layout is fixed, unlike `PostgreSQL`'s: `lib/usual/logging.c:231`
//! writes `<time> [<pid>] <LEVEL> <message>`, and `src/util.c:40` puts a socket
//! context in front of the message when the line belongs to a connection.

mod events;

pub use events::RECOGNIZED;

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
    /// What happened, with the `closing because:` wrapper and the connection's
    /// age removed so repeated events share one dictionary entry.
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
    /// Returns the operating system's error for reading the file.
    pub fn read_batch(&mut self, max_records: usize) -> io::Result<ReadBatch> {
        let batch = self
            .tail
            .read_batch_without_quote_tracking(continues, max_records)?;
        Ok(ReadBatch {
            events: batch.records.iter().filter_map(parse).collect(),
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
pub fn parse(record: &Record) -> Option<Event> {
    let (ts, rest) = timestamp::parse_local(record.first())?;
    let rest = rest.strip_prefix(" [")?;
    let level_at = rest.find("] ")?;
    let rest = rest.get(level_at + "] ".len()..)?;
    let message_at = rest.find(' ')?;
    let level = Level::parse(rest.get(..message_at)?)?;
    let (context, message) = split_socket_context(rest.get(message_at + 1..)?);

    let text = event_text(message, record.rest())?;
    Some(Event {
        ts,
        level,
        database: context.database,
        username: context.username,
        host: context.host,
        text,
    })
}

/// What the socket context in front of a message carried.
#[derive(Debug, Default)]
struct SocketContext {
    database: Option<String>,
    username: Option<String>,
    host: Option<String>,
}

/// Split `C-0x55f1: db/user@10.0.0.1:41537 ` off the front of a message.
///
/// Lines from `janitor.c`, `main.c` and `pooler.c` carry no socket, so the
/// whole message is returned untouched. The port is dropped: it is the client's
/// ephemeral port, different for every connection, and keeping it would cost
/// one dictionary entry per connection.
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
    (
        SocketContext {
            database,
            username,
            host: bounded(strip_port(address)),
        },
        tail,
    )
}

/// Drop the `:41537` an address ends with, leaving `[::1]` brackets in place.
fn strip_port(address: &str) -> &str {
    address
        .rfind(':')
        .and_then(|at| address.get(..at))
        .unwrap_or(address)
}

/// The event text, or `None` when the line is not one of the recognized events.
fn event_text(message: &str, continuations: &[String]) -> Option<String> {
    // `disconnect_client(notify=true)` logs the reason twice: once as `closing
    // because:` and once as this, through `send_pooler_error`
    // (`src/proto.c:266`). Keeping both would count every such event twice.
    if message.starts_with("pooler error: ") {
        return None;
    }
    let reason = message
        .strip_prefix("closing because: ")
        .map_or(message, strip_age);
    if !RECOGNIZED
        .iter()
        .any(|recognized| reason.starts_with(recognized))
    {
        return None;
    }
    let mut text = truncate(reason.trim(), MAX_TEXT_BYTES).to_owned();
    for line in continuations {
        crate::text::append_str(&mut text, line);
    }
    (!text.is_empty()).then_some(text)
}

/// Drop the ` (age=42s)` a disconnection reason ends with.
fn strip_age(reason: &str) -> &str {
    let Some(at) = reason.rfind(" (age=") else {
        return reason;
    };
    if reason.ends_with("s)") {
        reason.get(..at).unwrap_or(reason)
    } else {
        reason
    }
}
