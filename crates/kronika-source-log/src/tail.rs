//! Following a growing file through a fixed buffer.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

mod framing;
use framing::{OpenRecord, PartialLine, PhysicalLine};
use memchr::memchr;

/// Bytes handed to the kernel per `read` call. The file itself can be any
/// size; this is all of it that is ever in memory at once.
const READ_BUF_BYTES: usize = 65_536;

/// Longest prefix retained from a physical line and a logical record. What
/// follows the cut is scanned but not kept.
pub const MAX_LINE_BYTES: usize = 65_536;

/// Most raw bytes one [`Tail::read_batch`] reads from the file. Callers may
/// issue more batches while applying their catch-up limit.
pub const MAX_READ_BYTES: usize = 4 * 1_048_576;

/// Where a source left off in a file.
///
/// `dev` and `inode` identify the file the offset belongs to, so a rotated or
/// truncated file is read from its start instead of from a stale offset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    /// Device id of the file the offset was taken in.
    pub dev: u64,
    /// Inode of the file the offset was taken in.
    pub inode: u64,
    /// Byte offset of the first unacknowledged logical record.
    pub offset: u64,
}

/// One logical record: the line that opens it and the lines that continue it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The opening line and its continuations, in file order.
    pub lines: Vec<String>,
    truncated: bool,
}

impl Record {
    /// The line that opens the record.
    #[must_use]
    pub fn first(&self) -> &str {
        self.lines.first().map_or("", String::as_str)
    }

    /// The continuation lines, in file order.
    #[must_use]
    pub fn rest(&self) -> &[String] {
        self.lines.get(1..).unwrap_or_default()
    }

    /// The whole retained record with its original newlines.
    #[must_use]
    pub fn joined(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether bytes from this record were not retained.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}

/// Decides whether `line` continues the record read so far.
///
/// `raw_quotes_odd` is quote parity over every byte in `open`, including bytes
/// beyond the retained prefix. `csvlog` uses it; the text formats ignore it.
pub type Continues = fn(open: &[String], line: &str, raw_quotes_odd: bool) -> bool;

/// One bounded physical read and the complete logical records it produced.
#[derive(Debug)]
pub struct TailBatch {
    /// Complete logical records, in file order.
    pub records: Vec<Record>,
    /// Raw bytes read from the file for this batch.
    pub raw_bytes: usize,
    /// Whether the volatile scan cursor reached the observed end of file.
    pub at_eof: bool,
    /// Whether this batch completed input that awaits [`Tail::acknowledge`].
    pub needs_ack: bool,
}

/// Follows one file, keeping durable progress separate from volatile scanning.
///
/// A partial physical line and an open logical record survive calls in memory.
/// [`position`](Tail::position) changes only when the caller acknowledges a
/// completed batch after its rows are durable.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    position: Position,
    scan_offset: u64,
    partial: PartialLine,
    open: Option<OpenRecord>,
    staged: Option<PhysicalLine>,
    pending_end: Option<Position>,
}

impl Tail {
    /// Follow `path`, resuming from the acknowledged `position`.
    #[must_use]
    pub const fn new(path: PathBuf, position: Position) -> Self {
        Self {
            path,
            position,
            scan_offset: position.offset,
            partial: PartialLine::new(),
            open: None,
            staged: None,
            pending_end: None,
        }
    }

    /// The path being followed.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The acknowledged offset to resume from after a restart.
    #[must_use]
    pub const fn position(&self) -> Position {
        self.position
    }

    /// Commit the candidate position from the last completed batch.
    ///
    /// Returns the newly committed position, or `None` when the last read did
    /// not complete any logical input.
    pub fn acknowledge(&mut self) -> Option<Position> {
        let position = self.pending_end.take()?;
        self.position = position;
        Some(position)
    }

    /// Drop unacknowledged volatile progress and scan it again from the last
    /// committed position.
    ///
    /// Callers use this after a batch could not be admitted or made durable.
    pub fn retry(&mut self) {
        self.reset_volatile();
    }

    /// Read a bounded batch of complete records.
    ///
    /// At most `max_records` records are returned. The caller must acknowledge
    /// a batch with `needs_ack` before reading again.
    ///
    /// # Errors
    ///
    /// Returns the operating system's error for opening, stating, seeking or
    /// reading the file. It also rejects zero as a record bound and a second
    /// read while a candidate position is awaiting acknowledgement.
    pub fn read_batch(
        &mut self,
        continues: Continues,
        max_records: usize,
    ) -> io::Result<TailBatch> {
        self.read_batch_configured(continues, max_records, MAX_READ_BYTES, true)
    }

    pub(crate) fn read_batch_without_quote_tracking(
        &mut self,
        continues: Continues,
        max_records: usize,
    ) -> io::Result<TailBatch> {
        self.read_batch_configured(continues, max_records, MAX_READ_BYTES, false)
    }

    fn read_batch_configured(
        &mut self,
        continues: Continues,
        max_records: usize,
        raw_limit: usize,
        track_quotes: bool,
    ) -> io::Result<TailBatch> {
        if max_records == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tail batch record bound must be positive",
            ));
        }
        if raw_limit == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tail batch raw byte bound must be positive",
            ));
        }
        if self.pending_end.is_some() {
            return Err(io::Error::other(
                "tail batch must be acknowledged before another read",
            ));
        }

        let mut file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        let (dev, inode) = identity(&metadata);
        let size = metadata.len();

        // A different file under the same name, or a file cut behind the
        // volatile cursor, invalidates both durable and in-memory state.
        if dev != self.position.dev || inode != self.position.inode || size < self.scan_offset {
            self.reset(dev, inode);
        }

        file.seek(SeekFrom::Start(self.scan_offset))?;
        match self.scan(
            &mut file,
            size,
            continues,
            max_records,
            raw_limit,
            track_quotes,
        ) {
            Ok(batch) => Ok(batch),
            Err(error) => {
                // No candidate was handed to the caller. Re-scan from the last
                // acknowledged boundary rather than lose completed local rows.
                self.reset_volatile();
                Err(error)
            }
        }
    }

    fn scan(
        &mut self,
        file: &mut File,
        size: u64,
        continues: Continues,
        max_records: usize,
        raw_limit: usize,
        track_quotes: bool,
    ) -> io::Result<TailBatch> {
        let mut records = Vec::new();
        let mut candidate_end = None;
        let mut raw_bytes = 0_usize;

        if let Some(line) = self.staged.take() {
            self.accept_line(
                line,
                continues,
                max_records,
                &mut records,
                &mut candidate_end,
            );
        }

        let mut stop = !records.is_empty() && records.len() >= max_records;
        let mut buf = Vec::new();
        if !stop && raw_bytes < raw_limit && self.scan_offset < size {
            buf.resize(READ_BUF_BYTES, 0);
        }
        while !stop && raw_bytes < raw_limit && self.scan_offset < size {
            let remaining_file = usize::try_from(size - self.scan_offset).unwrap_or(usize::MAX);
            let want = READ_BUF_BYTES
                .min(raw_limit - raw_bytes)
                .min(remaining_file);
            let read = file.read(&mut buf[..want])?;
            if read == 0 {
                break;
            }
            raw_bytes += read;
            let mut at = 0_usize;
            while at < read {
                let chunk = buf.get(at..read).unwrap_or_default();
                let Some(end) = memchr(b'\n', chunk) else {
                    self.partial.push(chunk, track_quotes);
                    self.scan_offset = self.scan_offset.saturating_add(as_u64(chunk.len()));
                    break;
                };

                let before_newline = chunk.get(..end).unwrap_or_default();
                self.partial.push(before_newline, track_quotes);
                self.scan_offset = self
                    .scan_offset
                    .saturating_add(as_u64(before_newline.len() + 1));
                at += end + 1;
                let line = self.partial.finish(self.scan_offset);
                stop = self.accept_line(
                    line,
                    continues,
                    max_records,
                    &mut records,
                    &mut candidate_end,
                );
                if stop {
                    break;
                }
            }
        }

        if self.scan_offset >= size
            && records.len() < max_records
            && self.staged.is_none()
            && self.partial.is_empty()
            && self
                .open
                .as_ref()
                .is_some_and(|open| !continues(&open.lines, "", open.quotes_odd))
        {
            self.flush_open(&mut records, &mut candidate_end);
        }

        if let Some(offset) = candidate_end {
            self.pending_end = Some(Position {
                dev: self.position.dev,
                inode: self.position.inode,
                offset,
            });
        }
        // A staged line is logically unread even when its bytes reached the
        // physical end of the observed file.
        let at_eof = self.scan_offset >= size && self.staged.is_none();
        Ok(TailBatch {
            records,
            raw_bytes,
            at_eof,
            needs_ack: self.pending_end.is_some(),
        })
    }

    /// Accept one complete physical line. Returns whether the record cap was
    /// reached and scanning must stop.
    fn accept_line(
        &mut self,
        line: PhysicalLine,
        continues: Continues,
        max_records: usize,
        records: &mut Vec<Record>,
        candidate_end: &mut Option<u64>,
    ) -> bool {
        let line_text = line.text.as_deref().unwrap_or("");
        let starts_new = self
            .open
            .as_ref()
            .is_some_and(|open| !continues(&open.lines, line_text, open.quotes_odd));
        if starts_new {
            self.flush_open(records, candidate_end);
            if records.len() >= max_records {
                self.staged = Some(line);
                return true;
            }
        }
        self.open.get_or_insert_with(OpenRecord::new).push(line);
        false
    }

    fn flush_open(&mut self, records: &mut Vec<Record>, candidate_end: &mut Option<u64>) {
        let Some(open) = self.open.take() else {
            return;
        };
        if open.lines.iter().any(|line| !line.is_empty()) {
            records.push(Record {
                lines: open.lines,
                truncated: open.truncated,
            });
        }
        *candidate_end = Some(open.end);
    }

    fn reset(&mut self, dev: u64, inode: u64) {
        self.position = Position {
            dev,
            inode,
            offset: 0,
        };
        self.reset_volatile();
    }

    fn reset_volatile(&mut self) {
        self.scan_offset = self.position.offset;
        self.partial = PartialLine::new();
        self.open = None;
        self.staged = None;
        self.pending_end = None;
    }
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
const fn identity(_metadata: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

#[cfg(test)]
#[path = "tests/tail.rs"]
mod tests;
