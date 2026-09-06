//! Grouping errors by what they say rather than by the values in them.

use super::Severity;
use crate::text::{MAX_PATTERN_BYTES, truncate};

/// What an error was about.
///
/// The taxonomy is ordered: the first family whose phrases match wins, so a
/// deadlock reported alongside a permission problem counts as a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorCategory {
    /// Lock contention.
    Lock,
    /// Constraint violations.
    Constraint,
    /// Serialization failures.
    Serialization,
    /// Timeout or cancellation.
    Timeout,
    /// Resource exhaustion.
    Resource,
    /// Data corruption or `PANIC`.
    DataCorruption,
    /// System-level failures and uncategorized `FATAL`.
    System,
    /// Connection failures.
    Connection,
    /// Authentication or authorization failures.
    Auth,
    /// SQL syntax or semantic errors.
    Syntax,
    /// Everything else.
    Other,
}

impl ErrorCategory {
    /// The code stored in `pg_log_errors.category`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Lock => 0,
            Self::Constraint => 1,
            Self::Serialization => 2,
            Self::Timeout => 3,
            Self::Resource => 4,
            Self::DataCorruption => 5,
            Self::System => 6,
            Self::Connection => 7,
            Self::Auth => 8,
            Self::Syntax => 9,
            Self::Other => 10,
        }
    }
}

/// Replace the values in a message so two occurrences of the same error group
/// together.
#[must_use]
pub fn normalize_error(message: &str) -> String {
    let mut value = strip_at_character(message).to_owned();
    value = replace_quoted(value, '"', "\"...\"");
    value = replace_quoted(value, '\'', "'...'");
    value = replace_delimited(value, '(', ')', "(...)");
    value = replace_delimited(value, '[', ']', "[...]");
    replace_word_patterns(&mut value);
    truncate_owned(value, MAX_PATTERN_BYTES)
}

/// Normalize a statement for grouping: the error normalization plus the bare
/// numeric literals SQL carries.
#[must_use]
pub fn normalize_sql(sql: &str) -> String {
    let error = normalize_error(sql);
    let normalized = replace_numeric_literals(&error);
    truncate_owned(normalized, MAX_PATTERN_BYTES)
}

/// Decide which family a normalized message belongs to.
#[must_use]
pub fn classify_error(pattern: &str, severity: Severity) -> ErrorCategory {
    if severity == Severity::Panic {
        return ErrorCategory::DataCorruption;
    }
    let lower = pattern.to_ascii_lowercase();
    let category = first_match(&lower).unwrap_or(ErrorCategory::Other);
    if category == ErrorCategory::Other && severity == Severity::Fatal {
        ErrorCategory::System
    } else {
        category
    }
}

/// The taxonomy, in precedence order.
const FAMILIES: &[(ErrorCategory, &[&str])] = &[
    (
        ErrorCategory::Lock,
        &[
            "deadlock detected",
            "could not obtain lock",
            "lock timeout",
            "still waiting for",
        ],
    ),
    (
        ErrorCategory::Constraint,
        &[
            "duplicate key",
            "violates foreign key",
            "violates not-null",
            "violates check",
            "violates exclusion",
            "null value in column",
        ],
    ),
    (
        ErrorCategory::Serialization,
        &["could not serialize access"],
    ),
    (
        ErrorCategory::Timeout,
        &[
            "statement timeout",
            "idle-in-transaction session timeout",
            "transaction timeout",
            "idle session timeout",
            "canceling statement due to user request",
        ],
    ),
    (
        ErrorCategory::Resource,
        &[
            "out of memory",
            "out of shared memory",
            "too many connections",
            "disk full",
            "could not extend",
            "remaining connection slots",
        ],
    ),
    (
        ErrorCategory::DataCorruption,
        &[
            "invalid page",
            "could not read block",
            "data corrupted",
            "index corrupted",
            "unexpected zero page",
            "invalid checkpoint record",
            "invalid memory alloc",
        ],
    ),
    (
        ErrorCategory::System,
        &[
            "could not open file",
            "i/o error",
            "crash shutdown",
            "server process",
            "shutting down",
        ],
    ),
    (
        ErrorCategory::Connection,
        &[
            "connection reset by peer",
            "unexpected eof",
            "broken pipe",
            "could not receive data",
            "could not send data",
            "terminating connection",
        ],
    ),
    (
        ErrorCategory::Auth,
        &[
            "password authentication failed",
            "no pg_hba.conf entry",
            "role ",
            "permission denied",
            "ssl connection is required",
        ],
    ),
    (
        ErrorCategory::Syntax,
        &[
            "syntax error",
            "does not exist",
            "invalid input syntax",
            "division by zero",
            "value too long",
        ],
    ),
];

fn first_match(lower: &str) -> Option<ErrorCategory> {
    // An OOM kill reads as a resource problem, not as the system family its
    // "server process" wording would otherwise put it in.
    if is_oom_kill(lower) {
        return Some(ErrorCategory::Resource);
    }
    FAMILIES
        .iter()
        .find(|(_category, phrases)| phrases.iter().any(|phrase| lower.contains(phrase)))
        .map(|(category, _phrases)| *category)
}

fn is_oom_kill(lower: &str) -> bool {
    lower.contains("terminated by signal") && lower.contains(": killed")
}

fn strip_at_character(value: &str) -> &str {
    value
        .find(" at character ")
        .and_then(|pos| value.get(..pos))
        .unwrap_or(value)
}

fn replace_quoted(value: String, quote: char, replacement: &str) -> String {
    if !value.contains(quote) {
        return value;
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.char_indices();
    while let Some((start, c)) = chars.next() {
        if c != quote {
            out.push(c);
            continue;
        }
        let mut closed = None;
        for (idx, next) in chars.by_ref() {
            if next == quote {
                closed = Some(idx);
                break;
            }
        }
        match closed {
            Some(end) if end > start + quote.len_utf8() => out.push_str(replacement),
            Some(_empty) => {
                out.push(quote);
                out.push(quote);
            }
            None => out.push(quote),
        }
    }
    out
}

fn replace_delimited(value: String, open: char, close: char, replacement: &str) -> String {
    if !value.contains(open) {
        return value;
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != open {
            out.push(c);
            continue;
        }
        if chars.by_ref().any(|next| next == close) {
            out.push_str(replacement);
        } else {
            out.push(open);
        }
    }
    out
}

fn replace_word_patterns(out: &mut String) {
    for prefix in [
        "transaction ",
        "relation ",
        "process ",
        "database ",
        "PID ",
        "signal ",
        "on page ",
    ] {
        replace_after(out, prefix, |b| b.is_ascii_digit());
    }
    replace_after(out, "after ", |b| b.is_ascii_digit() || b == b'.');
    replace_wal_address(out);
    if let Some(pos) = out.find("invalid input syntax for ")
        && let Some(colon) = out.get(pos..).and_then(|tail| tail.find(": "))
    {
        out.truncate(pos + colon + 2);
        out.push_str("...");
    }
    let lower = out.to_ascii_lowercase();
    for object in [
        "permission denied for table ",
        "permission denied for schema ",
        "permission denied for sequence ",
        "permission denied for function ",
        "permission denied for database ",
    ] {
        if let Some(pos) = lower.find(object) {
            out.truncate(pos + object.len());
            out.push_str("...");
            break;
        }
    }
    if let Some(pos) = out.find("byte sequence for encoding") {
        out.truncate(pos + "byte sequence for encoding".len());
        out.push_str(" ...");
    }
}

fn replace_after(value: &mut String, prefix: &str, keep: impl Fn(u8) -> bool) {
    let Some(pos) = value.find(prefix) else {
        return;
    };
    let after = pos + prefix.len();
    let rest = value.as_bytes().get(after..).unwrap_or_default();
    if !rest.first().is_some_and(|b| keep(*b)) {
        return;
    }
    let end = rest.iter().position(|b| !keep(*b)).unwrap_or(rest.len());
    value.replace_range(after..after + end, "...");
}

/// Collapse a `0/16B3D40` WAL address, which is unique to one moment in the
/// write-ahead log and would give every occurrence its own group.
fn replace_wal_address(value: &mut String) {
    if !value.as_bytes().contains(&b'/') {
        return;
    }
    *value = replace_wal_address_copy(value);
}

fn replace_wal_address_copy(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if !bytes.get(idx).is_some_and(u8::is_ascii_hexdigit) {
            let Some(ch) = value.get(idx..).and_then(|tail| tail.chars().next()) else {
                break;
            };
            out.push(ch);
            idx += ch.len_utf8();
            continue;
        }
        let start = idx;
        while bytes.get(idx).is_some_and(u8::is_ascii_hexdigit) {
            idx += 1;
        }
        let first_len = idx - start;
        if bytes.get(idx) != Some(&b'/') || !(1..=8).contains(&first_len) {
            push_range(&mut out, value, start, idx);
            continue;
        }
        idx += 1;
        let second_start = idx;
        while bytes.get(idx).is_some_and(u8::is_ascii_hexdigit) {
            idx += 1;
        }
        let second_len = idx - second_start;
        let before_ok = start == 0 || !bytes.get(start - 1).is_some_and(u8::is_ascii_alphanumeric);
        let after_ok = !bytes.get(idx).is_some_and(u8::is_ascii_alphanumeric);
        if (1..=8).contains(&second_len) && before_ok && after_ok {
            out.push_str("x/x");
        } else {
            push_range(&mut out, value, start, idx);
        }
    }
    out
}

fn replace_numeric_literals(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if !bytes.get(idx).is_some_and(u8::is_ascii_digit) {
            let Some(ch) = value.get(idx..).and_then(|tail| tail.chars().next()) else {
                break;
            };
            out.push(ch);
            idx += ch.len_utf8();
            continue;
        }
        let start = idx;
        while bytes
            .get(idx)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
        {
            idx += 1;
        }
        let before_ok = start == 0 || !bytes.get(start - 1).is_some_and(|b| is_ident_byte(*b));
        let after_ok = !bytes.get(idx).is_some_and(|b| is_ident_byte(*b));
        if before_ok && after_ok {
            out.push_str("...");
        } else {
            push_range(&mut out, value, start, idx);
        }
    }
    out
}

const fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn push_range(out: &mut String, value: &str, start: usize, end: usize) {
    if let Some(slice) = value.get(start..end) {
        out.push_str(slice);
    }
}

fn truncate_owned(mut value: String, max_bytes: usize) -> String {
    let retained = truncate(&value, max_bytes).len();
    value.truncate(retained);
    value
}

#[cfg(test)]
mod tests;
