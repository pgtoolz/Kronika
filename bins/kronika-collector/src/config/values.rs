//! Input normalization shared by CLI arguments and their environment fallbacks.

use anyhow::{Context, Result};
use clap::builder::{EnumValueParser, PossibleValue, TypedValueParser};
use clap::{Arg, Command, ValueEnum};
use std::ffi::OsStr;

/// Decimal suffixes (G/GB) use 1000; IEC suffixes (GiB) use 1024.
pub(super) fn byte_size(raw: &str) -> Result<u64, bsize::ParseError> {
    raw.trim()
        .parse::<bsize::BSize64>()
        .map(bsize::BSize64::bytes)
}

/// Preserve whitespace accepted by the old environment configuration.
pub(crate) fn number<T: std::str::FromStr>(raw: &str) -> Result<T, String> {
    raw.trim()
        .parse()
        .map_err(|_error| "must be a nonnegative whole number".to_owned())
}

#[derive(Clone)]
pub(super) struct TrimmedEnum<T: ValueEnum + Clone + Send + Sync + 'static>(EnumValueParser<T>);

impl<T: ValueEnum + Clone + Send + Sync + 'static> TrimmedEnum<T> {
    pub(super) fn new() -> Self {
        Self(EnumValueParser::new())
    }
}

impl<T: ValueEnum + Clone + Send + Sync + 'static> TypedValueParser for TrimmedEnum<T> {
    type Value = T;
    fn parse_ref(&self, cmd: &Command, arg: Option<&Arg>, value: &OsStr) -> Result<T, clap::Error> {
        let value = value
            .to_str()
            .map_or(value, |value| OsStr::new(value.trim()));
        self.0.parse_ref(cmd, arg, value)
    }
    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        self.0.possible_values()
    }
}

/// The canonical variable is one DSN; the legacy list contributes only its first
/// entry. Ignore the legacy tail before decoding Unicode or validating syntax.
pub(super) fn parse_pg_dsn(
    canonical: Option<&OsStr>,
    legacy: Option<&OsStr>,
) -> Result<Option<String>> {
    let (key, selected) = match (canonical, legacy) {
        (Some(_), Some(_)) => {
            anyhow::bail!("KRONIKA_PG_DSN and KRONIKA_PG_DSNS must not both be set")
        }
        (Some(raw), None) => ("KRONIKA_PG_DSN (--pg-dsn)", raw.as_encoded_bytes()),
        (None, Some(raw)) => {
            if raw.to_str().is_some_and(|value| value.trim().is_empty()) {
                return Ok(None);
            }
            let first = raw
                .as_encoded_bytes()
                .split(|byte| *byte == b';')
                .next()
                .unwrap_or_default();
            ("KRONIKA_PG_DSNS", first)
        }
        (None, None) => return Ok(None),
    };
    let selected = std::str::from_utf8(selected)
        .with_context(|| format!("{key} must be valid Unicode"))?
        .trim();
    anyhow::ensure!(!selected.is_empty(), "{key} has an empty connection string");
    // Parser errors can contain the DSN, including credentials. Replace the
    // error entirely instead of attaching it as a source to the public message.
    selected
        .parse::<tokio_postgres::Config>()
        .map_err(|_error| anyhow::anyhow!("{key} is not a valid connection string"))?;
    Ok(Some(selected.to_owned()))
}

/// A blank value is an empty list; a blank element inside a list is an error.
pub(super) fn parse_env_list(key: &str, raw: &str) -> Result<Vec<String>> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    raw.split(';')
        .map(|element| {
            let value = element.trim();
            anyhow::ensure!(!value.is_empty(), "{key} has an empty element");
            Ok(value.to_owned())
        })
        .collect()
}
