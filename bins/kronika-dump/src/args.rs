//! CLI arguments and environment fallbacks, validated before storage access.

use std::ffi::OsString;
use std::path::PathBuf;

use chrono::{DateTime, FixedOffset, Timelike as _};
use clap::{CommandFactory, FromArgMatches, Parser};
use kronika_slice::UtcSecond;

/// The validated command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Inspect(InspectArgs),
    Slice(SliceArgs),
}

/// The selected inspection output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Sizes,
    Section(u32),
    Index,
}

/// Inspect recorded metrics or extract a standalone ZMS file.
#[derive(Parser)]
#[command(name = "kronika-dump", version, after_long_help = crate::help::INSPECT)]
struct InspectCli {
    /// Real collector recording directory containing dated segments and active.wal.
    #[arg(value_name = "DIR")]
    root: PathBuf,
    /// Decode one numeric section ID, resolving dictionary references.
    #[arg(long, value_name = "ID", conflicts_with = "index")]
    section: Option<u32>,
    /// Print calculated series/index summaries without creating an index file.
    #[arg(long, overrides_with = "index")]
    index: bool,
    /// Print one JSON object per line (NDJSON), including scan warnings.
    #[arg(long, overrides_with = "json")]
    json: bool,
    /// Rows per segment for --section; default 20, 0 prints every row.
    #[arg(
        long,
        value_name = "N",
        requires = "section",
        conflicts_with = "index",
        overrides_with = "limit"
    )]
    limit: Option<usize>,
    /// Inclusive earliest Unix microsecond; selects segments, not individual rows.
    #[arg(
        long,
        value_name = "MICROSECONDS",
        allow_negative_numbers = true,
        overrides_with = "from"
    )]
    from: Option<i64>,
    /// Inclusive latest Unix microsecond; selects segments, not individual rows.
    #[arg(
        long,
        value_name = "MICROSECONDS",
        allow_negative_numbers = true,
        overrides_with = "to"
    )]
    to: Option<i64>,
}

/// Validated inspection arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InspectArgs {
    pub(crate) root: PathBuf,
    pub(crate) view: View,
    pub(crate) json: bool,
    pub(crate) limit: usize,
    pub(crate) from: Option<i64>,
    pub(crate) to: Option<i64>,
}

/// Extract an interval into one new standalone ZMS file.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "slice", after_long_help = crate::help::SLICE)]
pub(crate) struct SliceArgs {
    /// Existing real collector storage directory; requires read access.
    #[arg(
        long,
        env = "KRONIKA_STORAGE_DIR",
        value_name = "DIR",
        hide_env_values = true
    )]
    pub(crate) storage_dir: PathBuf,
    /// Inclusive first whole second with Z or an offset; fractions are rejected.
    #[arg(long, value_name = "RFC3339", value_parser = parse_second)]
    pub(crate) from: UtcSecond,
    /// Inclusive last whole second, at or after --from; equal bounds select one second.
    #[arg(long, value_name = "RFC3339", value_parser = parse_second)]
    pub(crate) to: UtcSecond,
    /// New .zms output path; parent must exist and existing files are refused.
    #[arg(long, value_name = "FILE.zms")]
    pub(crate) out: PathBuf,
}

/// Parse arguments without the program name or any filesystem effects.
pub(crate) fn parse(
    arguments: impl IntoIterator<Item = impl Into<OsString>>,
) -> Result<Command, clap::Error> {
    let arguments = arguments.into_iter().map(Into::into).collect::<Vec<_>>();
    let mut command = InspectCli::command();
    // `slice` is a command only in the first position. For example,
    // `--json slice` still inspects the relative recording directory `slice`.
    // Include the subcommand for help so clap documents both entry points.
    if arguments
        .first()
        .is_some_and(|argument| argument == "slice")
        || arguments
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
    {
        command = command
            .subcommand(SliceArgs::command())
            .subcommand_negates_reqs(true)
            .args_conflicts_with_subcommands(true)
            .disable_help_subcommand(true);
    }
    let matches = command.try_get_matches_from_mut(
        std::iter::once(OsString::from("kronika-dump")).chain(arguments),
    )?;
    if let Some(("slice", matches)) = matches.subcommand() {
        let args = SliceArgs::from_arg_matches(matches)?;
        if args.from > args.to {
            return Err(command.error(
                clap::error::ErrorKind::ValueValidation,
                "--from must not be later than --to",
            ));
        }
        if args
            .out
            .extension()
            .is_none_or(|extension| extension != "zms")
        {
            return Err(command.error(
                clap::error::ErrorKind::ValueValidation,
                "--out must have the .zms suffix",
            ));
        }
        return Ok(Command::Slice(args));
    }
    let args = InspectCli::from_arg_matches(&matches)?;
    Ok(Command::Inspect(InspectArgs {
        root: args.root,
        view: args.section.map_or_else(
            || if args.index { View::Index } else { View::Sizes },
            View::Section,
        ),
        json: args.json,
        limit: args.limit.unwrap_or(20),
        from: args.from,
        to: args.to,
    }))
}

fn parse_second(value: &str) -> Result<UtcSecond, String> {
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
    if !separators {
        return Err(format!("{value:?} is not a whole-second RFC3339 timestamp"));
    }
    let parsed: DateTime<FixedOffset> = DateTime::parse_from_rfc3339(value)
        .map_err(|_bad| format!("{value:?} is not a whole-second RFC3339 timestamp"))?;
    if parsed.nanosecond() != 0 {
        return Err(format!("{value:?} is not a whole-second RFC3339 timestamp"));
    }
    UtcSecond::from_unix_seconds(parsed.timestamp())
        .map_err(|problem| format!("{value:?} is outside the supported timestamp range: {problem}"))
}

#[cfg(test)]
#[path = "tests/args.rs"]
mod tests;
