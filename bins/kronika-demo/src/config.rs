//! Parse CLI and environment once, before starting threads or touching storage.

use anyhow::{Context, Result};
use clap::{Arg, ArgMatches, Command};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use crate::system_activity::SystemActivityConfig;
use crate::workload::WorkloadConfig;

pub(crate) struct Config {
    pub(crate) root: PathBuf,
    pub(crate) storage_dir: PathBuf,
    pub(crate) duration_s: u64,
    pub(crate) collector_bin: PathBuf,
    pub(crate) collector_log: CollectorLog,
    pub(crate) system_activity: Option<SystemActivityConfig>,
    pub(crate) workload: Option<WorkloadConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectorLog {
    File,
    Stderr,
}

pub(crate) fn command() -> Command {
    Command::new("kronika-demo")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Run a bounded collector demonstration and report segment size, RSS, and CPU")
        .after_long_help(concat!(
            "CLI options override their environment variables. Other collector KRONIKA_* variables are inherited.\n",
            "Duration options accept integers in the named unit or durations such as 1m or 4000ms.\n",
            "Size/rate options accept integers in the named unit or byte sizes such as 32MiB or 32KiB.\n\n",
            "Examples:\n",
            "  kronika-demo --duration-s 1m --dir demo-data\n",
            "  kronika-demo --duration-s 10 --system-workload-enabled false\n",
            "  kronika-demo --duration-s 0 --collector-log stderr",
        ))
        .args([
            option(
                "dir",
                "KRONIKA_DEMO_DIR",
                Some("demo-data"),
                "Directory for collector.log and report.json",
            )
            .value_name("DIR")
            .value_parser(clap::value_parser!(PathBuf)),
            option(
                "storage-dir",
                "KRONIKA_STORAGE_DIR",
                None,
                "Collector storage directory [default: <dir>/segments]",
            )
            .value_name("DIR")
            .value_parser(clap::value_parser!(PathBuf)),
            option(
                "duration-s",
                "KRONIKA_DEMO_DURATION_S",
                Some("60"),
                "Run duration in seconds; 0 waits for SIGTERM or SIGINT",
            )
            .value_name("SECONDS"),
            option(
                "collector-bin",
                "KRONIKA_COLLECTOR_BIN",
                None,
                "Collector executable [default: kronika-collector beside this binary]",
            )
            .value_name("FILE")
            .value_parser(clap::value_parser!(PathBuf)),
            option(
                "collector-log",
                "KRONIKA_DEMO_COLLECTOR_LOG",
                Some("file"),
                "Write collector.log or inherit process output",
            )
            .value_parser(["file", "stderr"]),
        ])
        .args(crate::system_activity::config::args())
        .args(crate::workload::config::args())
}

pub(crate) fn parse_from(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
) -> Result<Config, clap::Error> {
    let mut command = command();
    let matches = command.try_get_matches_from_mut(args)?;
    Config::from_matches(&matches).map_err(|error| {
        command.error(
            clap::error::ErrorKind::ValueValidation,
            format!("{error:#}"),
        )
    })
}

impl Config {
    fn from_matches(matches: &ArgMatches) -> Result<Self> {
        let root = matches
            .get_one::<PathBuf>("KRONIKA_DEMO_DIR")
            .expect("default demo directory")
            .clone();
        let storage_dir = matches
            .get_one::<PathBuf>("KRONIKA_STORAGE_DIR")
            .cloned()
            .unwrap_or_else(|| root.join("segments"));
        let collector_bin = match matches.get_one::<PathBuf>("KRONIKA_COLLECTOR_BIN") {
            Some(path) => path.clone(),
            None => std::env::current_exe()
                .context("locate the demo binary")?
                .with_file_name("kronika-collector"),
        };
        Ok(Self {
            duration_s: seconds(matches, "KRONIKA_DEMO_DURATION_S")?,
            collector_bin,
            collector_log: match matches
                .get_one::<String>("KRONIKA_DEMO_COLLECTOR_LOG")
                .map(String::as_str)
            {
                Some("stderr") => CollectorLog::Stderr,
                _ => CollectorLog::File,
            },
            system_activity: SystemActivityConfig::from_matches(matches, &root, &storage_dir)?,
            workload: WorkloadConfig::from_matches(matches)?,
            root,
            storage_dir,
        })
    }
}

/// One metadata entry owns the flag, environment fallback, default, and help.
/// Optional workload values remain text until their group is enabled, matching
/// the existing behavior of ignoring unused environment controls.
pub(crate) fn option(
    long: &'static str,
    env: &'static str,
    default: Option<&'static str>,
    help: &'static str,
) -> Arg {
    let arg = Arg::new(env)
        .long(long)
        .env(env)
        .hide_env_values(true)
        .value_name("VALUE")
        .value_parser(clap::value_parser!(OsString))
        .help(help);
    if let Some(default) = default {
        arg.default_value(default)
    } else {
        arg
    }
}

pub(crate) fn text<'a>(matches: &'a ArgMatches, key: &str) -> Result<&'a str> {
    matches
        .get_one::<OsString>(key)
        .expect("option is present or has a default")
        .to_str()
        .with_context(|| format!("{key} must be valid UTF-8"))
}

pub(crate) fn integer<T: std::str::FromStr>(matches: &ArgMatches, key: &str) -> Result<T> {
    text(matches, key)?
        .trim()
        .parse()
        .map_err(|_error| anyhow::anyhow!("{key} must be a nonnegative whole number"))
}

pub(crate) fn seconds(matches: &ArgMatches, key: &str) -> Result<u64> {
    duration(text(matches, key)?, Duration::from_secs(1))
        .with_context(|| format!("{key} must be whole seconds"))
}

pub(crate) fn milliseconds(matches: &ArgMatches, key: &str) -> Result<u64> {
    duration(text(matches, key)?, Duration::from_millis(1))
        .with_context(|| format!("{key} must be whole milliseconds"))
}

fn duration(raw: &str, unit: Duration) -> Result<u64> {
    let raw = raw.trim();
    if let Ok(value) = raw.parse::<u64>() {
        return Ok(value);
    }
    let duration =
        humantime::parse_duration(raw).context("expected an integer or a duration such as 1m")?;
    anyhow::ensure!(
        duration.as_nanos() % unit.as_nanos() == 0,
        "fractional units are not supported"
    );
    u64::try_from(duration.as_nanos() / unit.as_nanos()).context("duration is too large")
}

pub(crate) fn size_units(matches: &ArgMatches, key: &str, unit: u64) -> Result<u64> {
    let raw = text(matches, key)?.trim();
    if let Ok(value) = raw.parse::<u64>() {
        return Ok(value);
    }
    let bytes = raw
        .parse::<bsize::BSize64>()
        .with_context(|| format!("{key} must be an integer or a byte size"))?
        .bytes();
    anyhow::ensure!(
        bytes % unit == 0,
        "{key} must be a multiple of {unit} bytes"
    );
    Ok(bytes / unit)
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
