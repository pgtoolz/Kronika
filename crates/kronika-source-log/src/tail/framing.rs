//! Retain bounded UTF-8 prefixes while tracking framing across discarded bytes.

use super::MAX_LINE_BYTES;
use memchr::memchr_iter;

#[derive(Debug)]
pub(super) struct PartialLine {
    bytes: Vec<u8>,
    truncated: bool,
    quotes_odd: bool,
}

impl PartialLine {
    pub(super) fn push(&mut self, chunk: &[u8], track_quotes: bool) {
        if track_quotes {
            self.quotes_odd ^= quote_parity(chunk);
        }
        if self.truncated {
            return;
        }
        let room = MAX_LINE_BYTES.saturating_sub(self.bytes.len());
        let kept = chunk.get(..room.min(chunk.len())).unwrap_or_default();
        self.bytes.extend_from_slice(kept);
        if kept.len() < chunk.len() {
            self.truncated = true;
        }
    }

    pub(super) fn finish(&mut self, end: u64) -> PhysicalLine {
        let partial = std::mem::replace(self, Self::new());
        let Self {
            bytes: mut raw,
            mut truncated,
            quotes_odd,
        } = partial;
        if !truncated && raw.last() == Some(&b'\r') {
            raw.pop();
        }
        let text = match String::from_utf8(raw) {
            Ok(text) => Some(text),
            Err(error) => {
                let valid = error.utf8_error().valid_up_to();
                let mut bytes = error.into_bytes();
                bytes.truncate(valid);
                truncated = true;
                String::from_utf8(bytes).ok()
            }
        };
        PhysicalLine {
            end,
            text,
            truncated,
            quotes_odd,
        }
    }

    pub(super) const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
            quotes_odd: false,
        }
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.bytes.is_empty() && !self.truncated
    }
}

#[derive(Debug)]
pub(super) struct OpenRecord {
    pub(super) lines: Vec<String>,
    bytes: usize,
    pub(super) end: u64,
    pub(super) truncated: bool,
    pub(super) quotes_odd: bool,
}

impl OpenRecord {
    pub(super) fn push(&mut self, line: PhysicalLine) {
        self.end = line.end;
        self.quotes_odd ^= line.quotes_odd;
        self.truncated |= line.truncated;

        let Some(mut text) = line.text else {
            return;
        };
        let separator = usize::from(!self.lines.is_empty());
        let Some(room) = MAX_LINE_BYTES
            .checked_sub(self.bytes)
            .and_then(|room| room.checked_sub(separator))
        else {
            self.truncated = true;
            return;
        };
        let retained = crate::text::truncate(&text, room);
        if retained.len() < text.len() {
            self.truncated = true;
        }
        text.truncate(retained.len());
        if separator != 0 {
            self.bytes += 1;
        }
        self.bytes += text.len();
        self.lines.push(text);
    }

    pub(super) const fn new() -> Self {
        Self {
            lines: Vec::new(),
            bytes: 0,
            end: 0,
            truncated: false,
            quotes_odd: false,
        }
    }
}

#[derive(Debug)]
pub(super) struct PhysicalLine {
    end: u64,
    pub(super) text: Option<String>,
    truncated: bool,
    quotes_odd: bool,
}

fn quote_parity(bytes: &[u8]) -> bool {
    memchr_iter(b'"', bytes).count() % 2 == 1
}
