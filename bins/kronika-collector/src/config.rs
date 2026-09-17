//! Startup settings read from the environment before collection begins.
//!
//! `KRONIKA_STORAGE_DIR` is required. In `postgresql` mode a `PostgreSQL` DSN is
//! required too. `Config::from_env` reads settings in validation order; retention
//! parsing and its startup log live in `retention`.
//!
//! Variable names, defaults and accepted values: `bins/kronika-collector/README.md`.

use anyhow::{Context, Result};
use kronika_format::{JOURNAL_HEADER_LEN, MAX_JOURNAL_LEN};
use std::path::PathBuf;

use crate::logging::{LogLevel, field, log_event, log_level_from_env};
use crate::scheduler::{Intervals, MIN_PG_STATEMENTS_INTERVAL_SECS};

mod retention;

pub(crate) use retention::RetentionConfig;

// Base timer wake period; individual sources also have their own intervals.
const DEFAULT_TICK_SECS: u64 = 5;
// Close a segment after 64 MiB of raw journal data, before compression.
const DEFAULT_SEGMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;
// Period for phased segment closing; the first segment may close sooner.
const DEFAULT_SEGMENT_MAX_AGE_SECS: u64 = 900;
// Ignore PostgreSQL log entries older than 15 minutes at read time.
const DEFAULT_PG_LOG_MAX_LAG_SECS: u64 = 900;

/// Collector settings loaded and validated before startup.
pub(crate) struct Config {
    /// Which recorded source families this collector reads.
    pub(crate) mode: CollectorMode,
    /// Data root: the journal, the finished segments, and the writer lock.
    pub(crate) storage_dir: PathBuf,
    /// Base tick of the internal timer, seconds; `0` disables the timer and
    /// leaves collection to signals only.
    pub(crate) tick_secs: u64,
    /// Per-source read intervals.
    pub(crate) intervals: Intervals,

    /// Write the segment when the journal holds at least this many raw bytes.
    pub(crate) segment_max_bytes: u64,
    /// Period of phased age eligibility; the first segment may close sooner.
    pub(crate) segment_max_age_secs: u64,
    /// Hard cap of the on-disk journal file; reaching it writes the open
    /// segment early instead of failing the append.
    pub(crate) journal_max_bytes: u64,
    /// Storage-rotation target for the whole storage tree.
    pub(crate) retention: Option<RetentionConfig>,

    /// The one `PostgreSQL` server used for metrics and log discovery.
    pub(crate) pg_dsn: Option<String>,
    /// Explicit CPU capacity of the monitored `PostgreSQL` server.
    pub(crate) postgres_effective_cpus: Option<u32>,
    /// `PostgreSQL` logs named outright, as paths or globs.
    pub(crate) pg_logs: Vec<String>,
    /// Maximum `PostgreSQL` log age at read time, seconds.
    pub(crate) pg_log_max_lag_secs: u64,

    /// Where to ask `PgBouncer` which log it writes and who it is.
    pub(crate) pgbouncer_dsns: Vec<String>,
    /// `PgBouncer` logs named outright, as paths or globs.
    pub(crate) pgbouncer_logs: Vec<String>,
}

impl Config {
    /// Read and validate the collector settings from the environment.
    ///
    /// # Errors
    ///
    /// Returns an error when `KRONIKA_STORAGE_DIR` is unset, a variable does not
    /// parse, or a bound fails validation.
    pub(crate) fn from_env() -> Result<Self> {
        let storage_dir: PathBuf = std::env::var("KRONIKA_STORAGE_DIR")
            .context("KRONIKA_STORAGE_DIR is not set")?
            .into();
        validate_log_level()?;
        let mode = CollectorMode::parse(
            &std::env::var("KRONIKA_COLLECTOR_MODE").unwrap_or_else(|_| "local".to_owned()),
        )?;

        let tick_secs = env_u64("KRONIKA_INTERVAL_S", DEFAULT_TICK_SECS)?;

        let segment_max_bytes = env_u64("KRONIKA_SEGMENT_MAX_BYTES", DEFAULT_SEGMENT_MAX_BYTES)?;
        validate_segment_max_bytes(segment_max_bytes)?;
        let segment_max_age_secs =
            env_u64("KRONIKA_SEGMENT_MAX_AGE_S", DEFAULT_SEGMENT_MAX_AGE_SECS)?;
        let journal_max_bytes = env_u64("KRONIKA_JOURNAL_MAX_BYTES", MAX_JOURNAL_LEN as u64)?;
        validate_journal_max_bytes(journal_max_bytes)?;
        if segment_max_bytes > journal_max_bytes {
            log_event(
                LogLevel::Warn,
                "config_degraded",
                &[
                    field("reason", "segment_cap_exceeds_journal_cap"),
                    field("segment_max_bytes", segment_max_bytes),
                    field("journal_max_bytes", journal_max_bytes),
                ],
            );
        }
        let retention = RetentionConfig::from_env(segment_max_bytes)?;

        let pg_dsn = parse_pg_dsn(
            std::env::var_os("KRONIKA_PG_DSN").as_deref(),
            std::env::var_os("KRONIKA_PG_DSNS").as_deref(),
        )?;
        let postgres_effective_cpus = optional_positive_u32(
            "KRONIKA_POSTGRES_EFFECTIVE_CPUS",
            std::env::var("KRONIKA_POSTGRES_EFFECTIVE_CPUS")
                .ok()
                .as_deref(),
        )?;
        anyhow::ensure!(
            pg_dsn.is_some() || postgres_effective_cpus.is_none(),
            "KRONIKA_POSTGRES_EFFECTIVE_CPUS requires KRONIKA_PG_DSN"
        );
        anyhow::ensure!(
            mode.collect_os() || pg_dsn.is_some(),
            "KRONIKA_COLLECTOR_MODE=postgresql requires KRONIKA_PG_DSN"
        );

        let pg_log_max_lag_secs = env_u64("KRONIKA_PG_LOG_MAX_LAG_S", DEFAULT_PG_LOG_MAX_LAG_SECS)?;
        anyhow::ensure!(
            pg_log_max_lag_secs > 0,
            "KRONIKA_PG_LOG_MAX_LAG_S must be greater than zero"
        );

        let pgbouncer_dsns = env_list("KRONIKA_PGBOUNCER_DSNS")?;
        let pgbouncer_logs = env_list("KRONIKA_PGBOUNCER_LOGS")?;
        anyhow::ensure!(
            mode.collect_os() || (pgbouncer_dsns.is_empty() && pgbouncer_logs.is_empty()),
            "KRONIKA_COLLECTOR_MODE=postgresql does not collect PgBouncer; remove KRONIKA_PGBOUNCER_DSNS and KRONIKA_PGBOUNCER_LOGS"
        );

        Ok(Self {
            mode,
            storage_dir,
            tick_secs,
            intervals: intervals_from_env()?,
            segment_max_bytes,
            segment_max_age_secs,
            journal_max_bytes,
            retention: Some(retention),
            pg_dsn,
            postgres_effective_cpus,
            pg_logs: env_list("KRONIKA_PG_LOGS")?,
            pg_log_max_lag_secs,
            pgbouncer_dsns,
            pgbouncer_logs,
        })
    }
}

/// Collection placement explicitly selected by the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectorMode {
    /// Linux resources and `PostgreSQL` on the same machine.
    Local,
    /// `PostgreSQL` metrics and explicitly named `PostgreSQL` log files only.
    Postgresql,
}

impl CollectorMode {
    pub(crate) const fn collect_os(self) -> bool {
        matches!(self, Self::Local)
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            "local" => Ok(Self::Local),
            "postgresql" => Ok(Self::Postgresql),
            _ => anyhow::bail!("KRONIKA_COLLECTOR_MODE must be local or postgresql"),
        }
    }
}

/// The canonical variable is one DSN; the legacy list contributes only its first
/// entry. Ignore the legacy tail before decoding Unicode or validating syntax.
fn parse_pg_dsn(
    canonical: Option<&std::ffi::OsStr>,
    legacy: Option<&std::ffi::OsStr>,
) -> Result<Option<String>> {
    let (key, selected) = match (canonical, legacy) {
        (Some(_), Some(_)) => {
            anyhow::bail!("KRONIKA_PG_DSN and KRONIKA_PG_DSNS must not both be set")
        }
        (Some(raw), None) => ("KRONIKA_PG_DSN", raw.as_encoded_bytes()),
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

/// Read the per-source intervals, falling back to the built-in defaults.
fn intervals_from_env() -> Result<Intervals> {
    let defaults = Intervals::default();
    let intervals = Intervals {
        os_core: env_u64("KRONIKA_OS_CORE_INTERVAL_S", defaults.os_core)?,
        os_mount_topo: env_u64("KRONIKA_OS_MOUNTTOPO_INTERVAL_S", defaults.os_mount_topo)?,
        os_processes: env_u64("KRONIKA_OS_PROCESS_INTERVAL_S", defaults.os_processes)?,
        os_process_status: env_u64(
            "KRONIKA_OS_PROCESS_STATUS_INTERVAL_S",
            defaults.os_process_status,
        )?,
        os_cgroup: env_u64("KRONIKA_OS_CGROUP_INTERVAL_S", defaults.os_cgroup)?,
        os_cgroup_mapping: env_u64(
            "KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S",
            defaults.os_cgroup_mapping,
        )?,
        logs: env_u64("KRONIKA_LOG_INTERVAL_S", defaults.logs)?,
        pg_instance: env_u64("KRONIKA_PG_INTERVAL_S", defaults.pg_instance)?,
        pg_tables_and_indexes: env_u64(
            "KRONIKA_PG_RELATIONS_INTERVAL_S",
            defaults.pg_tables_and_indexes,
        )?,
        pg_activity: env_u64("KRONIKA_PG_ACTIVITY_INTERVAL_S", defaults.pg_activity)?,
        pg_activity_blocked: env_u64(
            "KRONIKA_PG_ACTIVITY_BLOCKED_INTERVAL_S",
            defaults.pg_activity_blocked,
        )?,
        pg_statements_and_plans: env_u64(
            "KRONIKA_PG_STATEMENTS_INTERVAL_S",
            defaults.pg_statements_and_plans,
        )?,
    };
    anyhow::ensure!(
        intervals.pg_statements_and_plans >= MIN_PG_STATEMENTS_INTERVAL_SECS,
        "KRONIKA_PG_STATEMENTS_INTERVAL_S must be at least {MIN_PG_STATEMENTS_INTERVAL_SECS} seconds"
    );
    Ok(intervals)
}

/// Read a `;`-separated list. Missing or non-Unicode values produce an empty list.
fn env_list(key: &str) -> Result<Vec<String>> {
    match std::env::var(key) {
        Ok(raw) => parse_env_list(key, &raw),
        Err(_unset) => Ok(Vec::new()),
    }
}

/// A blank value is an empty list; a blank element inside a list is an error.
fn parse_env_list(key: &str, raw: &str) -> Result<Vec<String>> {
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

/// Read an unsigned integer. Missing or non-Unicode values use `default`.
fn env_u64(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(raw) => parse_env_number(key, &raw),
        Err(_unset) => Ok(default),
    }
}

/// Parse a number after trimming, naming the variable and raw value on failure.
fn parse_env_number<T: std::str::FromStr>(key: &str, raw: &str) -> Result<T> {
    raw.trim()
        .parse()
        .map_err(|_parse| anyhow::anyhow!("{key}={raw:?} is not a whole number"))
}

fn optional_positive_u32(key: &str, raw: Option<&str>) -> Result<Option<u32>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = parse_env_number::<u32>(key, raw)?;
    anyhow::ensure!(value > 0, "{key} must be greater than zero");
    Ok(Some(value))
}

/// Reject an unrecognized `KRONIKA_LOG_LEVEL` before collection starts.
fn validate_log_level() -> Result<()> {
    anyhow::ensure!(
        log_level_from_env().is_some(),
        "KRONIKA_LOG_LEVEL={:?} is not one of error, warn, info, debug, trace",
        std::env::var("KRONIKA_LOG_LEVEL").unwrap_or_default()
    );
    Ok(())
}

fn validate_journal_max_bytes(value: u64) -> Result<()> {
    anyhow::ensure!(
        (JOURNAL_HEADER_LEN as u64..=MAX_JOURNAL_LEN as u64).contains(&value),
        "KRONIKA_JOURNAL_MAX_BYTES must be in {JOURNAL_HEADER_LEN}..={MAX_JOURNAL_LEN}, got {value}"
    );
    Ok(())
}

fn validate_segment_max_bytes(value: u64) -> Result<()> {
    anyhow::ensure!(
        value > 0,
        "KRONIKA_SEGMENT_MAX_BYTES must be greater than zero"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/config/mode.rs"]
mod mode_tests;
