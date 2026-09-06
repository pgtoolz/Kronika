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
    /// Data root: the journal, the finished segments, and the writer lock.
    pub(crate) storage_dir: PathBuf,
    /// Base tick of the internal timer, seconds; `0` disables the timer and
    /// leaves collection to signals only.
    pub(crate) tick_secs: u64,
    /// Per-source read intervals.
    pub(crate) intervals: Intervals,
    /// Write the segment when the journal holds at least this many raw bytes.
    pub(crate) segment_max_bytes: u64,
    /// Write an open segment at this age even if the byte cap was not reached.
    pub(crate) segment_max_age_secs: u64,
    /// Hard cap of the on-disk journal file; reaching it writes the open
    /// segment early instead of failing the append.
    pub(crate) journal_max_bytes: u64,
    /// Storage-rotation target for the whole storage tree.
    pub(crate) retention: Option<RetentionConfig>,
    /// Where to ask `PostgreSQL` which log it writes and who it is.
    pub(crate) pg_dsns: Vec<String>,
    /// Explicit CPU capacity of the monitored `PostgreSQL` server.
    pub(crate) postgres_effective_cpus: Option<u32>,
    /// `PostgreSQL` logs named outright, as paths or globs.
    pub(crate) pg_logs: Vec<String>,
    /// Where to ask `PgBouncer` which log it writes and who it is.
    pub(crate) pgbouncer_dsns: Vec<String>,
    /// `PgBouncer` logs named outright, as paths or globs.
    pub(crate) pgbouncer_logs: Vec<String>,
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
/// `auto:<P>` with `P` in `1..=99`. The name omits a `_BYTES` suffix because a
/// future time criterion will arrive under its own env as an OR condition.
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
        let pg_dsns = env_list("KRONIKA_PG_DSNS")?;
        let postgres_effective_cpus = optional_positive_u32(
            "KRONIKA_POSTGRES_EFFECTIVE_CPUS",
            std::env::var("KRONIKA_POSTGRES_EFFECTIVE_CPUS")
                .ok()
                .as_deref(),
        )?;
        anyhow::ensure!(
            !pg_dsns.is_empty() || postgres_effective_cpus.is_none(),
            "KRONIKA_POSTGRES_EFFECTIVE_CPUS requires KRONIKA_PG_DSNS"
        );
        Ok(Self {
            storage_dir,
            tick_secs,
            intervals: intervals_from_env()?,
            segment_max_bytes,
            segment_max_age_secs,
            journal_max_bytes,
            retention: Some(retention),
            pg_dsns,
            postgres_effective_cpus,
            pg_logs: env_list("KRONIKA_PG_LOGS")?,
            pgbouncer_dsns: env_list("KRONIKA_PGBOUNCER_DSNS")?,
            pgbouncer_logs: env_list("KRONIKA_PGBOUNCER_LOGS")?,
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
