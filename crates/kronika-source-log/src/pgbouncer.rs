//! Reading a `PgBouncer` log.
//!
//! The line layout is fixed, unlike `PostgreSQL`'s: `lib/usual/logging.c:231`
//! writes `<time> [<pid>] <LEVEL> <message>`, and `src/util.c:40` puts a socket
//! context in front of the message when the line belongs to a connection.
//!
//! A pooler running under systemd writes that line to stderr, and the file an
//! operator keeps with `journalctl -u pgbouncer -o short-full >> pgbouncer.log`
//! carries a `journalctl` prefix ending in `<identifier>[<pid>]: ` in front of
//! it. Such a prefix is skipped; the pooler's own time and level are used.
//!
//! With `log_disconnections = 0` the pooler writes no `closing because:` line
//! and a rejected client leaves only the `pooler error:` warning behind, so
//! that warning is an event unless the matching `closing because:` line came
//! right before it on the same client socket.

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
    /// The last `closing because:` event, so the `pooler error:` line the
    /// pooler writes right after it for the same client is not counted twice.
    last_closing: Option<Closing>,
}

/// A `closing because:` event waiting for its `pooler error:` twin.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Closing {
    socket: String,
    text: String,
}

/// Which line of a disconnection an event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `closing because: <reason>`, written when `log_disconnections` is on.
    Closing,
    /// `pooler error: <reason>`, written when `log_pooler_errors` is on.
    PoolerError,
    /// Any other recognized message.
    Other,
}

/// One recognized line with what the deduplication needs to know about it.
#[derive(Debug)]
struct Parsed {
    event: Event,
    socket: Option<String>,
    kind: Kind,
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
            last_closing: None,
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
        let mut events = Vec::new();
        for parsed in batch.records.iter().filter_map(classify) {
            if self.admit(&parsed) {
                events.push(parsed.event);
            }
        }
        Ok(ReadBatch {
            events,
            raw_bytes: batch.raw_bytes,
            at_eof: batch.at_eof,
            needs_ack: batch.needs_ack,
        })
    }

    /// Whether `parsed` is a new event rather than the `pooler error:` twin
    /// of the `closing because:` event just before it.
    ///
    /// `disconnect_client(notify=true)` logs the reason as `closing because:`
    /// (`src/objects.c:900`) and then sends it to the client, which logs it
    /// again as `pooler error:` (`src/proto.c:129`). The twin follows on the
    /// same client socket with the same text; it is dropped even across a
    /// batch boundary. A `pooler error:` with no such predecessor is the only
    /// trace of the rejection when `log_disconnections` is off, and stays.
    fn admit(&mut self, parsed: &Parsed) -> bool {
        match parsed.kind {
            Kind::Closing => {
                self.last_closing = parsed.socket.as_ref().map(|socket| Closing {
                    socket: socket.clone(),
                    text: parsed.event.text.clone(),
                });
                true
            }
            Kind::PoolerError => {
                let twin = self.last_closing.take().is_some_and(|closing| {
                    parsed.socket.as_deref() == Some(closing.socket.as_str())
                        && parsed.event.text == closing.text
                });
                !twin
            }
            Kind::Other => true,
        }
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

/// The pooler's own line behind a `journalctl` or syslog prefix.
///
/// Every `journalctl` output style (`short`, `short-full`, `short-iso`, with
/// or without the host name) and rsyslog end their prefix with
/// `<identifier>[<pid>]: `; the pooler's `<time> [<pid>] <LEVEL> <message>`
/// follows it unchanged. The first such marker closes the prefix: the
/// pooler's own `[<pid>]` is followed by a space, not a colon. Returns `None`
/// for a line without the marker.
fn journal_payload(line: &str) -> Option<&str> {
    let at = line.find("]: ")?;
    line.get(at + "]: ".len()..)
}

/// Read one line, or `None` when it is not an event this collector records.
///
/// A lone record is read without the `pooler error:` deduplication a
/// [`PgBouncerLog`] applies across the lines it follows.
#[must_use]
pub fn parse(record: &Record) -> Option<Event> {
    classify(record).map(|parsed| parsed.event)
}

fn classify(record: &Record) -> Option<Parsed> {
    let first = record.first();
    let (ts, rest) = timestamp::parse_local(first)
        .or_else(|| timestamp::parse_local(journal_payload(first)?))?;
    let rest = rest.strip_prefix(" [")?;
    let level_at = rest.find("] ")?;
    let rest = rest.get(level_at + "] ".len()..)?;
    let message_at = rest.find(' ')?;
    let level = Level::parse(rest.get(..message_at)?)?;
    let (context, message) = split_socket_context(rest.get(message_at + 1..)?);

    let (kind, text) = event_text(message, record.rest())?;
    Some(Parsed {
        event: Event {
            ts,
            level,
            database: context.database,
            username: context.username,
            host: context.host,
            text,
        },
        socket: context.socket,
        kind,
    })
}

/// What the socket context in front of a message carried.
#[derive(Debug, Default)]
struct SocketContext {
    /// The pointer `PgBouncer` prints for the socket, `C-0x55f1a2b3c4d5`,
    /// which ties the lines of one disconnection together.
    socket: Option<String>,
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
    if !message.starts_with("C-") && !message.starts_with("S-") {
        return none;
    }
    let Some(at) = message.find(": ") else {
        return none;
    };
    let socket = message.get(..at).unwrap_or_default();
    let rest = message.get(at + ": ".len()..).unwrap_or_default();
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
            socket: bounded(socket),
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

/// The event text and where it came from, or `None` when the line is not one
/// of the recognized events.
///
/// A `closing because:` reason and the `pooler error:` sent for it carry the
/// same text; [`PgBouncerLog::admit`] keeps only one of the pair.
fn event_text(message: &str, continuations: &[String]) -> Option<(Kind, String)> {
    let (kind, reason) = match (
        message.strip_prefix("pooler error: "),
        message.strip_prefix("closing because: "),
    ) {
        (Some(reason), _) => (Kind::PoolerError, reason),
        (None, Some(reason)) => (Kind::Closing, strip_age(reason)),
        (None, None) => (Kind::Other, message),
    };
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
    (!text.is_empty()).then_some((kind, text))
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
