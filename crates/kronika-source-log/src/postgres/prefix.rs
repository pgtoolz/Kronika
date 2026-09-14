//! What `log_line_prefix` puts in front of a `stderr` record.
//!
//! The setting is read from `pg_settings`, so the collector matches whatever
//! the server was configured with instead of guessing a layout.

use crate::text::bounded;
use crate::timestamp;

/// One piece of a `log_line_prefix`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// Text printed as it stands.
    Literal(String),
    /// Event time priority: `%n`, `%m`, `%t`. Zero is session start.
    Time(u8),
    /// `%u`: the user name.
    User,
    /// `%d`: the database name.
    Database,
    /// Any other escape, whose value is read and discarded.
    Skipped,
    /// `%q`: everything after this is absent in non-session processes.
    SessionOnly,
}

/// A compiled `log_line_prefix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePrefix {
    tokens: Vec<Token>,
}

/// What one line's prefix carried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PrefixFields {
    pub(super) ts: Option<i64>,
    pub(super) time_expected: bool,
    pub(super) database: Option<String>,
    pub(super) username: Option<String>,
}

impl LinePrefix {
    /// Compile the `log_line_prefix` setting.
    #[must_use]
    pub fn parse(setting: &str) -> Self {
        let mut tokens = Vec::new();
        let mut literal = String::new();
        let mut chars = setting.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                literal.push(c);
                continue;
            }
            let Some(escape) = chars.next() else {
                literal.push('%');
                break;
            };
            if escape == '%' {
                literal.push('%');
                continue;
            }
            if !literal.is_empty() {
                tokens.push(Token::Literal(std::mem::take(&mut literal)));
            }
            tokens.push(match escape {
                'n' => Token::Time(3),
                'm' => Token::Time(2),
                't' => Token::Time(1),
                's' => Token::Time(0),
                'u' => Token::User,
                'd' => Token::Database,
                'q' => Token::SessionOnly,
                _ => Token::Skipped,
            });
        }
        if !literal.is_empty() {
            tokens.push(Token::Literal(literal));
        }
        Self { tokens }
    }

    fn has_event_time(&self) -> bool {
        self.tokens
            .iter()
            .any(|token| matches!(token, Token::Time(1..=3)))
    }

    /// Read the prefix out of `head`, the text before the severity marker.
    ///
    /// Matching stops at the first literal the text does not carry, and at the
    /// `%q` a background process wrote nothing after, keeping whatever was read
    /// before it.
    pub(super) fn read(&self, head: &str, zone: Option<&timestamp::LogTimezone>) -> PrefixFields {
        let mut fields = PrefixFields {
            time_expected: self.has_event_time(),
            ..PrefixFields::default()
        };
        let mut rest = head;
        let mut priority = 0;
        for (index, token) in self.tokens.iter().enumerate() {
            match token {
                Token::Literal(text) => {
                    let Some(tail) = rest.strip_prefix(text.as_str()) else {
                        break;
                    };
                    rest = tail;
                }
                Token::SessionOnly => {
                    if rest.is_empty() {
                        fields.time_expected = priority != 0;
                        break;
                    }
                }
                Token::Time(rank) => {
                    let parsed = if *rank == 3 {
                        timestamp::epoch(rest).map(|(ts, tail)| (Some(ts), tail))
                    } else if *rank == 0 || *rank < priority {
                        timestamp::calendar(rest).map(|(_, _, _, tail)| (None, tail))
                    } else {
                        match timestamp::parse(rest, zone) {
                            Ok((ts, tail)) => Some((Some(ts), tail)),
                            Err(_) => timestamp::calendar(rest).map(|(_, _, _, tail)| (None, tail)),
                        }
                    };
                    if *rank != 0 && *rank >= priority {
                        fields.ts = parsed.and_then(|(ts, _)| ts);
                        priority = *rank;
                    }
                    let Some((_, tail)) = parsed else { break };
                    rest = tail;
                }
                other => {
                    let (value, tail) = take_value(rest, self.tokens.get(index + 1));
                    match other {
                        Token::User => fields.username = bounded(value),
                        Token::Database => fields.database = bounded(value),
                        _ => {}
                    }
                    rest = tail;
                }
            }
        }
        fields
    }
}

/// Take an escape's value: it runs up to the literal that follows it, or up to
/// the next space when the next token is another escape.
fn take_value<'a>(rest: &'a str, next: Option<&Token>) -> (&'a str, &'a str) {
    let end = match next {
        Some(Token::Literal(text)) => rest.find(text.as_str()),
        _ => rest.find(' '),
    }
    .unwrap_or(rest.len());
    (
        rest.get(..end).unwrap_or_default(),
        rest.get(end..).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests;
