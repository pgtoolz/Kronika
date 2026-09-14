//! Environment-only configuration for the collector daemon.
//!
//! `KRONIKA_STORAGE_DIR` is the one required variable. Everything else has a
//! default that is safe on a host that also runs a database. Every variable
//! is read and parsed here, before any collection starts; a value that does
//! not parse stops the daemon instead of falling back to the default.
//!
//! The full list with defaults is in `bins/kronika-collector/README.md`.

use anyhow::{Context, Result};
use kronika_format::{JOURNAL_HEADER_LEN, MAX_JOURNAL_LEN};
use std::path::PathBuf;

use crate::logging::{LogLevel, field, log_event, log_level_from_env};
use crate::scheduler::Intervals;

/// The validated daemon contract.
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
    /// Maximum PostgreSQL log age at read time, seconds.
    pub(crate) pg_log_max_lag_secs: u64,
    /// Where to ask `PgBouncer` which log it writes and who it is.
    pub(crate) pgbouncer_dsns: Vec<String>,
    /// `PgBouncer` logs named outright, as paths or globs.
    pub(crate) pgbouncer_logs: Vec<String>,
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
}

fn parse_mode(raw: &str) -> Result<CollectorMode> {
    match raw.trim() {
        "local" => Ok(CollectorMode::Local),
        "postgresql" => Ok(CollectorMode::Postgresql),
        _ => anyhow::bail!("KRONIKA_COLLECTOR_MODE must be local or postgresql"),
    }
}

/// Select one connection before metrics and log discovery are initialized.
fn parse_pg_dsn(
    canonical: Option<&std::ffi::OsStr>,
    legacy: Option<&std::ffi::OsStr>,
) -> Result<Option<String>> {
    anyhow::ensure!(
        canonical.is_none() || legacy.is_none(),
        "KRONIKA_PG_DSN and KRONIKA_PG_DSNS must not both be set"
    );
    let (key, raw) = match (canonical, legacy) {
        (Some(raw), _) => ("KRONIKA_PG_DSN", raw),
        (_, Some(raw)) => ("KRONIKA_PG_DSNS", raw),
        _ => return Ok(None),
    };
    let selected = if canonical.is_some() {
        raw.as_encoded_bytes()
    } else {
        if raw.to_str().is_some_and(|value| value.trim().is_empty()) {
            return Ok(None);
        }
        raw.as_encoded_bytes()
            .split(|byte| *byte == b';')
            .next()
            .unwrap_or_default()
    };
    let selected = std::str::from_utf8(selected)
        .with_context(|| format!("{key} must be valid Unicode"))?
        .trim();
    anyhow::ensure!(!selected.is_empty(), "{key} has an empty connection string");
    selected
        .parse::<tokio_postgres::Config>()
        .map_err(|_error| anyhow::anyhow!("{key} is not a valid connection string"))?;
    Ok(Some(selected.to_owned()))
}

/// Read a `;`-separated list, or an empty one when the variable is unset.
fn env_list(key: &str) -> Result<Vec<String>> {
    match std::env::var(key) {
        Ok(raw) => parse_env_list(key, &raw),
        Err(_unset) => Ok(Vec::new()),
    }
}

/// Parse one list variable's value. An element that is blank after trimming is
/// a typo, not an empty list, so it stops the daemon.
///
/// # Errors
///
/// Returns an error naming the variable when one of its elements is blank.
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

/// Read a numeric variable, or refuse to start naming what was given.
fn env_u64(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(raw) => parse_env_number(key, &raw),
        Err(_unset) => Ok(default),
    }
}

fn optional_positive_u32(key: &str, raw: Option<&str>) -> Result<Option<u32>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = parse_env_number::<u32>(key, raw)?;
    anyhow::ensure!(value > 0, "{key} must be greater than zero");
    Ok(Some(value))
}

/// Parse one numeric variable's value.
///
/// # Errors
///
/// Returns an error naming the variable and the value it was given.
fn parse_env_number<T: std::str::FromStr>(key: &str, raw: &str) -> Result<T> {
    raw.trim()
        .parse()
        .map_err(|_parse| anyhow::anyhow!("{key}={raw:?} is not a whole number"))
}

/// Used-fraction target of the `auto` mode when no percentage is given.
const DEFAULT_AUTO_PERCENT: u8 = 80;
/// Fixed rotation target when `KRONIKA_RETENTION` is unset.
const DEFAULT_RETENTION_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Rotation target for the whole `KRONIKA_STORAGE_DIR` tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    variant_size_differences,
    reason = "a 16-byte Copy enum; the byte-budget and percentage arms cannot share a width"
)]
pub(crate) enum RetentionConfig {
    /// Keep the tree at or below this many bytes.
    Fixed(u64),
    /// Keep the backing partition's used fraction at or below this percentage.
    Auto(u8),
}

/// Parses `KRONIKA_RETENTION` into a rotation target.
///
/// Accepts a raw byte budget (`<u64>`), `auto` (equivalent to `auto:80`), or
/// `auto:<P>` with `P` in `1..=99`.
///
/// # Errors
///
/// Returns an error for an empty value, a non-numeric budget, an out-of-range
/// percentage, or an unrecognized `auto` suffix.
pub(crate) fn parse_retention(raw: &str) -> Result<RetentionConfig> {
    let value = raw.trim();
    anyhow::ensure!(!value.is_empty(), "KRONIKA_RETENTION must not be empty");
    if let Some(suffix) = value.strip_prefix("auto") {
        let percent = if suffix.is_empty() {
            DEFAULT_AUTO_PERCENT
        } else {
            let digits = suffix.strip_prefix(':').with_context(|| {
                format!("KRONIKA_RETENTION must be 'auto' or 'auto:P', got {value:?}")
            })?;
            digits
                .parse::<u8>()
                .with_context(|| format!("KRONIKA_RETENTION percentage is not a u8: {digits:?}"))?
        };
        anyhow::ensure!(
            (1..=99).contains(&percent),
            "KRONIKA_RETENTION auto percentage must be in 1..=99, got {percent}"
        );
        return Ok(RetentionConfig::Auto(percent));
    }
    let budget = value.parse::<u64>().with_context(|| {
        format!("KRONIKA_RETENTION must be a byte budget or 'auto[:P]', got {value:?}")
    })?;
    Ok(RetentionConfig::Fixed(budget))
}

/// Rejects a fixed budget that cannot hold the non-deletable minimum plus room
/// to rotate.
///
/// The floor is `2 × KRONIKA_SEGMENT_MAX_BYTES`: the active journal and the
/// newest finished segment are never deleted, so a smaller budget could never
/// converge. `auto` targets a live partition fraction and has no such bound.
///
/// # Errors
///
/// Returns an error naming the budget and the required floor.
pub(crate) fn validate_retention(retention: RetentionConfig, segment_max_bytes: u64) -> Result<()> {
    if let RetentionConfig::Fixed(budget) = retention {
        let floor = segment_max_bytes.saturating_mul(2);
        anyhow::ensure!(
            budget >= floor,
            "KRONIKA_RETENTION fixed budget {budget} is below 2 × KRONIKA_SEGMENT_MAX_BYTES \
             ({floor}); a budget that cannot hold two segments cannot converge"
        );
    }
    Ok(())
}
impl Config {
    /// Read and validate the daemon contract from the environment.
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
        let mode = parse_mode(
            &std::env::var("KRONIKA_COLLECTOR_MODE").unwrap_or_else(|_| "local".to_owned()),
        )?;
        let tick_secs = env_u64("KRONIKA_INTERVAL_S", 5)?;
        let segment_max_bytes = env_u64("KRONIKA_SEGMENT_MAX_BYTES", 64 * 1024 * 1024)?;
        validate_segment_max_bytes(segment_max_bytes)?;
        let segment_max_age_secs = env_u64("KRONIKA_SEGMENT_MAX_AGE_S", 900)?;
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
        let retention = std::env::var("KRONIKA_RETENTION")
            .ok()
            .map(|raw| parse_retention(&raw))
            .transpose()?
            .unwrap_or(RetentionConfig::Fixed(DEFAULT_RETENTION_BYTES));
        validate_retention(retention, segment_max_bytes)?;
        log_retention_config(retention);
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
        let pg_log_max_lag_secs = env_u64("KRONIKA_PG_LOG_MAX_LAG_S", 900)?;
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

fn log_retention_config(retention: RetentionConfig) {
    match retention {
        RetentionConfig::Fixed(budget) => log_event(
            LogLevel::Info,
            "config_retention",
            &[field("mode", "fixed"), field("budget_bytes", budget)],
        ),
        RetentionConfig::Auto(percent) => log_event(
            LogLevel::Info,
            "config_retention",
            &[
                field("mode", "auto"),
                field("used_percent", u64::from(percent)),
            ],
        ),
    }
}
/// Refuse to start on a log level nothing can print.
fn validate_log_level() -> Result<()> {
    anyhow::ensure!(
        log_level_from_env().is_some(),
        "KRONIKA_LOG_LEVEL={:?} is not one of error, warn, info, debug, trace",
        std::env::var("KRONIKA_LOG_LEVEL").unwrap_or_default()
    );
    Ok(())
}

pub(crate) fn validate_journal_max_bytes(value: u64) -> Result<()> {
    anyhow::ensure!(
        (JOURNAL_HEADER_LEN as u64..=MAX_JOURNAL_LEN as u64).contains(&value),
        "KRONIKA_JOURNAL_MAX_BYTES must be in {JOURNAL_HEADER_LEN}..={MAX_JOURNAL_LEN}, got {value}"
    );
    Ok(())
}

pub(crate) fn validate_segment_max_bytes(value: u64) -> Result<()> {
    anyhow::ensure!(
        value > 0,
        "KRONIKA_SEGMENT_MAX_BYTES must be greater than zero"
    );
    Ok(())
}

/// Read the per-source intervals, falling back to the built-in defaults.
fn intervals_from_env() -> Result<Intervals> {
    let defaults = Intervals::default();
    Ok(Intervals {
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
        pg: env_u64("KRONIKA_PG_INTERVAL_S", defaults.pg)?,
        pg_relations: env_u64("KRONIKA_PG_RELATIONS_INTERVAL_S", defaults.pg_relations)?,
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mode_tests {
    use super::{CollectorMode, parse_mode};

    #[test]
    fn only_explicit_postgresql_mode_disables_linux() {
        assert!(parse_mode("local").expect("local mode").collect_os());
        assert_eq!(
            parse_mode("postgresql").expect("PostgreSQL mode"),
            CollectorMode::Postgresql
        );
        assert!(!CollectorMode::Postgresql.collect_os());
        for invalid in ["", "remote", "2", "auto"] {
            assert!(
                parse_mode(invalid).is_err(),
                "reject unsupported {invalid:?}"
            );
        }
    }
}
