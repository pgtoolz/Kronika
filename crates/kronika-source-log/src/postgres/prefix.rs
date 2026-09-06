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
    /// `%m`, `%t` or `%s`: a timestamp.
    Time,
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
                'm' | 't' | 's' | 'n' => Token::Time,
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

    /// Read the prefix out of `head`, the text before the severity marker.
    ///
    /// Matching stops at the first literal the text does not carry, and at the
    /// `%q` a background process wrote nothing after, keeping whatever was read
    /// before it.
    pub(super) fn read(&self, head: &str) -> PrefixFields {
        let mut fields = PrefixFields::default();
        let mut rest = head;
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
                        break;
                    }
                }
                Token::Time => {
                    let Some((ts, tail)) = timestamp::parse_local(rest) else {
                        break;
                    };
                    fields.ts = Some(ts);
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
