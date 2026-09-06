//! What the command line asked for.

use std::path::PathBuf;

use chrono::{DateTime, FixedOffset, Timelike as _};
use kronika_dump::UtcSecond;

/// The usage text, printed when the arguments do not parse.
pub(crate) const USAGE: &str = "\
usage: kronika-dump <data-dir> [--section <type_id>] [--index] [--json]
                    [--limit <rows>] [--from <ts>] [--to <ts>]
       kronika-dump slice --from <RFC3339> --to <RFC3339> --out <FILE>

  no flag        every segment with what each section costs
  --section ID   the rows of one section, dictionary ids resolved
  --index        typed identities and bounded series summaries
  --json         machine-readable instead of a table
  --limit N      stop after N rows per segment (default 20, 0 means all)
  --from TS      inspection: unix microseconds; slice: whole-second RFC3339
  --to TS        inspection: unix microseconds; slice: whole-second RFC3339
  --out FILE     one new standalone .zms file; an existing path is refused
";

/// Top-level command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// Existing inspection behavior.
    Inspect(InspectArgs),
    /// Create one standalone time slice.
    Slice(SliceArgs),
}

/// What to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Want {
    /// Sections and their sizes.
    Sizes,
    /// The rows of one section.
    Section(u32),
    /// Typed identities and bounded series summaries.
    Index,
}

/// Parsed inspection arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InspectArgs {
    /// The data directory to read.
    pub(crate) root: PathBuf,
    /// What to print.
    pub(crate) want: Want,
    /// Print JSON rather than a table.
    pub(crate) json: bool,
    /// Rows per segment, `0` for all of them.
    pub(crate) limit: usize,
    /// Earliest timestamp of interest, unix microseconds.
    pub(crate) from: Option<i64>,
    /// Latest timestamp of interest, unix microseconds.
    pub(crate) to: Option<i64>,
}

/// Parsed slice arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SliceArgs {
    /// Inclusive first requested UTC second.
    pub(crate) from: UtcSecond,
    /// Inclusive last requested UTC second.
    pub(crate) to: UtcSecond,
    /// Exact caller-selected output file.
    pub(crate) out: PathBuf,
}

/// Parse arguments, without the program name.
///
/// # Errors
///
/// Returns what was wrong with them, ready to print.
pub(crate) fn parse<I: IntoIterator<Item = String>>(arguments: I) -> Result<Command, String> {
    let mut rest = arguments.into_iter();
    let Some(first) = rest.next() else {
        return Err("a data directory or the slice command is required".to_owned());
    };
    if first == "slice" {
        parse_slice(rest).map(Command::Slice)
    } else {
        parse_inspect(std::iter::once(first).chain(rest)).map(Command::Inspect)
    }
}

fn parse_inspect<I: IntoIterator<Item = String>>(arguments: I) -> Result<InspectArgs, String> {
    let mut root = None;
    let mut want = Want::Sizes;
    let mut json = false;
    let mut limit = None;
    let mut from = None;
    let mut to = None;
    let mut rest = arguments.into_iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--index" => {
                if matches!(want, Want::Section(_)) {
                    return Err("--index and --section are mutually exclusive".to_owned());
                }
                want = Want::Index;
            }
            "--section" => {
                let value = rest.next().ok_or("--section needs a type_id")?;
                let type_id = value
                    .parse()
                    .map_err(|_bad| format!("{value:?} is not a type_id"))?;
                match want {
                    Want::Index => {
                        return Err("--index and --section are mutually exclusive".to_owned());
                    }
                    Want::Section(_) => {
                        return Err("--section may be specified only once".to_owned());
                    }
                    Want::Sizes => want = Want::Section(type_id),
                }
            }
            "--limit" => {
                let value = rest.next().ok_or("--limit needs a row count")?;
                limit = Some(
                    value
                        .parse()
                        .map_err(|_bad| format!("{value:?} is not a row count"))?,
                );
            }
            "--from" | "--to" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("{argument} needs a timestamp"))?;
                let ts = value
                    .parse()
                    .map_err(|_bad| format!("{value:?} is not a timestamp"))?;
                if argument == "--from" {
                    from = Some(ts);
                } else {
                    to = Some(ts);
                }
            }
            flag if flag.starts_with('-') => return Err(format!("unknown flag {flag}")),
            path if root.is_none() => root = Some(PathBuf::from(path)),
            extra => return Err(format!("unexpected argument {extra:?}")),
        }
    }
    if limit.is_some() && !matches!(want, Want::Section(_)) {
        return Err("--limit requires --section".to_owned());
    }
    Ok(InspectArgs {
        root: root.ok_or("a data directory is required")?,
        want,
        json,
        limit: limit.unwrap_or(20),
        from,
        to,
    })
}

fn parse_slice<I: IntoIterator<Item = String>>(arguments: I) -> Result<SliceArgs, String> {
    let mut from = None;
    let mut to = None;
    let mut out = None;
    let mut rest = arguments.into_iter();
    while let Some(flag) = rest.next() {
        let target = match flag.as_str() {
            "--from" => &mut from,
            "--to" => &mut to,
            "--out" => &mut out,
            known if !known.starts_with("--") => {
                return Err(format!("unexpected argument {known:?}"));
            }
            unknown => return Err(format!("unknown flag {unknown}")),
        };
        let value = rest.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if target.replace(value).is_some() {
            return Err(format!("{flag} may be specified only once"));
        }
    }

    let from = parse_second("--from", from)?;
    let to = parse_second("--to", to)?;
    if from > to {
        return Err("--from must not be later than --to".to_owned());
    }
    let out = PathBuf::from(out.ok_or("--out is required")?);
    if out.as_os_str().is_empty() {
        return Err("--out must name a file".to_owned());
    }
    if out.extension().is_none_or(|extension| extension != "zms") {
        return Err("--out must have the .zms suffix".to_owned());
    }
    Ok(SliceArgs { from, to, out })
}

fn parse_second(flag: &str, value: Option<String>) -> Result<UtcSecond, String> {
    let value = value.ok_or_else(|| format!("{flag} is required"))?;
    let bytes = value.as_bytes();
    let separators = bytes.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && matches!(bytes.get(10), Some(b'T' | b't'))
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && (bytes.len() == 20 && matches!(bytes.get(19), Some(b'Z' | b'z'))
            || bytes.len() == 25
                && matches!(bytes.get(19), Some(b'+' | b'-'))
                && bytes.get(22) == Some(&b':'));
    if !separators || bytes.get(11..19).is_none_or(|time| time.contains(&b'.')) {
        return Err(format!("{value:?} is not a whole-second RFC3339 timestamp"));
    }
    let parsed: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(&value)
        .map_err(|_bad| format!("{value:?} is not a whole-second RFC3339 timestamp"))?;
    if parsed.nanosecond() != 0 {
        return Err(format!("{value:?} is not a whole-second RFC3339 timestamp"));
    }
    UtcSecond::from_unix_seconds(parsed.timestamp())
        .map_err(|problem| format!("{value:?} is outside the supported timestamp range: {problem}"))
}

#[cfg(test)]
mod tests;
