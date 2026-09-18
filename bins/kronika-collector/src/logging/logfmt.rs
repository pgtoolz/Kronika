//! Field values and quoting for the collector's single-line logfmt output.

use std::fmt::{Display, Write as _};

use super::LogLevel;

pub(crate) struct LogField<'a> {
    key: &'static str,
    value: LogValue<'a>,
}

// Keep scalar values and borrowed Display objects unformatted until an event
// passes its level filter. Owned text and path displays already hold a string.
pub(crate) enum LogValue<'a> {
    Str(&'a str),
    Display(&'a dyn Display),
    Owned(String),
    Bool(bool),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    U128(u128),
    Usize(usize),
}

pub(crate) fn field<'a>(key: &'static str, value: impl Into<LogValue<'a>>) -> LogField<'a> {
    LogField {
        key,
        value: value.into(),
    }
}

impl<'a, T: Display> From<&'a T> for LogValue<'a> {
    fn from(value: &'a T) -> Self {
        Self::Display(value)
    }
}

impl<'a> From<&'a str> for LogValue<'a> {
    fn from(value: &'a str) -> Self {
        Self::Str(value)
    }
}

impl<'a> From<&'a dyn Display> for LogValue<'a> {
    fn from(value: &'a dyn Display) -> Self {
        Self::Display(value)
    }
}

impl From<String> for LogValue<'_> {
    fn from(value: String) -> Self {
        Self::Owned(value)
    }
}

impl From<std::path::Display<'_>> for LogValue<'_> {
    fn from(value: std::path::Display<'_>) -> Self {
        Self::Owned(value.to_string())
    }
}

macro_rules! scalar_values {
    ($($type:ty => $variant:ident),+ $(,)?) => {
        $(impl From<$type> for LogValue<'_> {
            fn from(value: $type) -> Self {
                Self::$variant(value)
            }
        })+
    };
}

scalar_values!(
    bool => Bool,
    i32 => I32,
    i64 => I64,
    u32 => U32,
    u64 => U64,
    u128 => U128,
    usize => Usize,
);

impl From<Option<u64>> for LogValue<'_> {
    fn from(value: Option<u64>) -> Self {
        value.map_or(Self::Str("unavailable"), Self::U64)
    }
}

pub(crate) fn render_log_line(
    level: LogLevel,
    action: &'static str,
    fields: &[LogField<'_>],
) -> String {
    let mut line = String::from("kronika-collector");
    push_log_field(&mut line, "level", level.as_str());
    push_log_field(&mut line, "action", action);
    for field in fields {
        push_log_field_value(&mut line, field.key, &field.value);
    }
    line
}

fn push_log_field(line: &mut String, key: &str, value: &str) {
    line.push(' ');
    line.push_str(key);
    line.push('=');
    push_log_value(line, value);
}

fn push_log_field_value(line: &mut String, key: &str, value: &LogValue<'_>) {
    let display: &dyn Display = match value {
        LogValue::Str(value) => {
            push_log_field(line, key, value);
            return;
        }
        LogValue::Owned(value) => {
            push_log_field(line, key, value);
            return;
        }
        LogValue::Display(value) => *value,
        LogValue::Bool(value) => value,
        LogValue::I32(value) => value,
        LogValue::I64(value) => value,
        LogValue::U32(value) => value,
        LogValue::U64(value) => value,
        LogValue::U128(value) => value,
        LogValue::Usize(value) => value,
    };
    let mut rendered = String::new();
    let _ = write!(&mut rendered, "{display}");
    push_log_field(line, key, &rendered);
}

fn push_log_value(line: &mut String, value: &str) {
    let plain = !value.is_empty()
        && value.chars().all(|ch| {
            !ch.is_whitespace() && !ch.is_control() && ch != '=' && ch != '"' && ch != '\\'
        });
    if plain {
        line.push_str(value);
        return;
    }
    line.push('"');
    for ch in value.chars() {
        match ch {
            '"' => line.push_str("\\\""),
            '\\' => line.push_str("\\\\"),
            '\n' => line.push_str("\\n"),
            '\r' => line.push_str("\\r"),
            '\t' => line.push_str("\\t"),
            _ if ch.is_control() => {
                line.push_str("\\u{");
                let _ = write!(line, "{:x}", ch as u32);
                line.push('}');
            }
            _ => line.push(ch),
        }
    }
    line.push('"');
}
