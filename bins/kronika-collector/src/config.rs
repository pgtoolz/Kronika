//! CLI arguments and environment fallbacks, validated once before startup.
//!
//! The process owns one immutable configuration. Parsing is independent of that
//! global instance, so tests can check different settings without changing it.

use anyhow::Result;
use clap::parser::ValueSource;
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum};
use kronika_format::{JOURNAL_HEADER_LEN, MAX_JOURNAL_LEN};
use kronika_source_os::{ProcFs, SysFs};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::logging::{LogLevel, field, log_event};
use crate::scheduler::{Intervals, MIN_PG_STATEMENTS_INTERVAL_SECS};

mod retention;
pub(crate) mod values;
pub(crate) use retention::RetentionConfig;
use values::{TrimmedEnum, byte_size, number, parse_env_list, parse_pg_dsn};

// Initialized after validation, before any runtime thread or collection starts.
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Record Linux metrics, `PostgreSQL` metrics, and local logs.
#[derive(Parser)]
#[command(name = "kronika-collector", version, after_long_help = crate::help::EXAMPLES)]
pub(crate) struct Config {
    /// Collection mode; postgresql requires a DSN and does not read Linux metrics.
    #[arg(long, env = "KRONIKA_COLLECTOR_MODE", default_value = "local", value_parser = TrimmedEnum::<CollectorMode>::new(), hide_env_values = true)]
    pub(crate) mode: CollectorMode,
    /// Recording directory containing active.wal and finished segments. Required.
    #[arg(
        long,
        env = "KRONIKA_STORAGE_DIR",
        value_name = "DIR",
        hide_env_values = true
    )]
    pub(crate) storage_dir: PathBuf,
    /// Maximum timer sleep in seconds; 0 disables timed collection (SIGUSR2 only).
    #[arg(long = "interval-s", env = "KRONIKA_INTERVAL_S", default_value_t = 5, value_parser = number::<u64>, hide_env_values = true)]
    pub(crate) tick_secs: u64,
    #[command(flatten)]
    pub(crate) intervals: Intervals,

    /// Close a segment at this raw journal size, before compression (e.g. 64MiB).
    #[arg(long, env = "KRONIKA_SEGMENT_MAX_BYTES", default_value = "64MiB", value_name = "SIZE", value_parser = byte_size, help_heading = "Storage", hide_env_values = true)]
    pub(crate) segment_max_bytes: u64,
    /// Segment closing period in seconds; the first segment may close sooner.
    #[arg(long = "segment-max-age-s", env = "KRONIKA_SEGMENT_MAX_AGE_S", default_value_t = 900, value_parser = number::<u64>, help_heading = "Storage", hide_env_values = true)]
    pub(crate) segment_max_age_secs: u64,
    /// Hard active.wal limit (36 bytes..1GiB); closes the segment early.
    #[arg(long, env = "KRONIKA_JOURNAL_MAX_BYTES", default_value = "1GiB", value_name = "SIZE", value_parser = byte_size, help_heading = "Storage", hide_env_values = true)]
    pub(crate) journal_max_bytes: u64,
    /// Storage target: size (e.g. 10GiB, 10G, 10GB), auto (= auto:80), or auto:1..99.
    #[arg(
        long,
        env = "KRONIKA_RETENTION",
        default_value = "2GiB",
        value_name = "SIZE|auto[:P]",
        help_heading = "Storage",
        hide_env_values = true
    )]
    pub(crate) retention: Option<RetentionConfig>,

    /// One `PostgreSQL` DSN; local mode also discovers its log file automatically.
    #[arg(
        long,
        env = "KRONIKA_PG_DSN",
        value_name = "DSN",
        help_heading = "PostgreSQL and logs",
        hide_env_values = true
    )]
    pub(crate) pg_dsn: Option<String>,
    /// Target `PostgreSQL` CPU capacity (> 0); requires --pg-dsn.
    #[arg(long, env = "KRONIKA_POSTGRES_EFFECTIVE_CPUS", value_parser = number::<u32>, help_heading = "PostgreSQL and logs", hide_env_values = true)]
    pub(crate) postgres_effective_cpus: Option<u32>,
    /// PEM CA bundle replacing the built-in public roots; hostname validation stays enabled.
    #[arg(
        long,
        env = "KRONIKA_PG_SSL_ROOT_CERT",
        value_name = "FILE",
        help_heading = "PostgreSQL and logs",
        hide_env_values = true
    )]
    pub(crate) pg_ssl_root_cert: Option<PathBuf>,
    /// Extra local log path/glob; local mode already discovers the current log via --pg-dsn.
    /// In postgresql mode, only these explicit paths are read. Repeat for multiple paths.
    #[arg(
        long = "pg-log",
        env = "KRONIKA_PG_LOGS",
        value_name = "PATH",
        help_heading = "PostgreSQL and logs",
        hide_env_values = true
    )]
    pub(crate) pg_logs: Vec<String>,
    /// Skip `PostgreSQL` log entries older than this many seconds (> 0).
    #[arg(long = "pg-log-max-lag-s", env = "KRONIKA_PG_LOG_MAX_LAG_S", default_value_t = 900, value_parser = number::<u64>, help_heading = "PostgreSQL and logs", hide_env_values = true)]
    pub(crate) pg_log_max_lag_secs: u64,
    /// `PgBouncer` admin-console DSN for log discovery. Repeat for multiple consoles.
    #[arg(
        long = "pgbouncer-dsn",
        env = "KRONIKA_PGBOUNCER_DSNS",
        value_name = "DSN",
        help_heading = "PostgreSQL and logs",
        hide_env_values = true
    )]
    pub(crate) pgbouncer_dsns: Vec<String>,
    /// Local `PgBouncer` log path or final-component glob. Repeat for multiple paths.
    #[arg(
        long = "pgbouncer-log",
        env = "KRONIKA_PGBOUNCER_LOGS",
        value_name = "PATH",
        help_heading = "PostgreSQL and logs",
        hide_env_values = true
    )]
    pub(crate) pgbouncer_logs: Vec<String>,

    /// `host:port` for the Prometheus `/metrics` endpoint; unset disables it.
    #[arg(
        long,
        env = "KRONIKA_PROMETHEUS_LISTEN",
        value_name = "ADDR",
        help_heading = "Prometheus",
        hide_env_values = true
    )]
    pub(crate) prometheus_listen: Option<std::net::SocketAddr>,
    /// Overlay with metrics/presets in the metrics.yaml format; file or directory. Repeatable.
    #[arg(
        long,
        env = "KRONIKA_PROMETHEUS_METRICS",
        value_name = "PATH",
        help_heading = "Prometheus",
        hide_env_values = true
    )]
    pub(crate) prometheus_metrics: Vec<PathBuf>,
    /// Preset name from the embedded catalog or the overlays.
    #[arg(
        long,
        env = "KRONIKA_PROMETHEUS_PRESET",
        default_value = kronika_prometheus::catalog::DEFAULT_PRESET,
        value_name = "NAME",
        help_heading = "Prometheus",
        hide_env_values = true
    )]
    pub(crate) prometheus_preset: String,

    /// Structured stderr logging level (case-insensitive; warning aliases warn).
    #[arg(long, env = "KRONIKA_LOG_LEVEL", default_value = "info", value_parser = TrimmedEnum::<LogLevel>::new(), ignore_case = true, help_heading = "Logging and Linux paths", hide_env_values = true)]
    pub(crate) log_level: LogLevel,
    /// Procfs root (default /proc). An explicit root disables container packaging hints.
    #[arg(
        long,
        env = "KRONIKA_PROC_ROOT",
        value_name = "DIR",
        help_heading = "Logging and Linux paths",
        hide_env_values = true
    )]
    pub(crate) proc_root: Option<PathBuf>,
    /// Sysfs root, used only in local mode.
    #[arg(
        long,
        env = "KRONIKA_SYS_ROOT",
        default_value = "/sys",
        value_name = "DIR",
        help_heading = "Logging and Linux paths",
        hide_env_values = true
    )]
    pub(crate) sys_root: PathBuf,
}

/// Linux/`PostgreSQL` colocated on this machine, or `PostgreSQL` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CollectorMode {
    Local,
    Postgresql,
}

impl CollectorMode {
    pub(crate) const fn collect_os(self) -> bool {
        matches!(self, Self::Local)
    }

    #[cfg(test)]
    fn parse(raw: &str) -> Result<Self, String> {
        Self::from_str(raw.trim(), false)
    }
}

/// Parse without installing the global configuration or touching storage/network.
pub(crate) fn parse_from(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
) -> Result<Config, clap::Error> {
    let mut command = Config::command();
    let matches = command.try_get_matches_from_mut(args)?;
    let mut config = Config::from_arg_matches(&matches)?;
    config
        .normalize(&matches)
        .and_then(|()| config.validate())
        .map_err(|error| {
            command.error(clap::error::ErrorKind::ValueValidation, error.to_string())
        })?;
    Ok(config)
}

pub(crate) fn install(config: Config) -> Result<()> {
    CONFIG
        .set(config)
        .map_err(|_error| anyhow::anyhow!("collector configuration already initialized"))
}

pub(crate) fn get() -> &'static Config {
    CONFIG
        .get()
        .expect("configuration is installed before collector startup")
}

impl Config {
    // CLI lists are individual arguments. Only old env lists use semicolons.
    fn normalize(&mut self, matches: &clap::ArgMatches) -> Result<()> {
        let legacy = (matches.value_source("pg_dsn") != Some(ValueSource::CommandLine))
            .then(|| std::env::var_os("KRONIKA_PG_DSNS"))
            .flatten();
        self.pg_dsn = parse_pg_dsn(
            self.pg_dsn.as_deref().map(std::ffi::OsStr::new),
            legacy.as_deref(),
        )?;
        for (id, key, rows) in [
            ("pg_logs", "KRONIKA_PG_LOGS", &mut self.pg_logs),
            (
                "pgbouncer_dsns",
                "KRONIKA_PGBOUNCER_DSNS",
                &mut self.pgbouncer_dsns,
            ),
            (
                "pgbouncer_logs",
                "KRONIKA_PGBOUNCER_LOGS",
                &mut self.pgbouncer_logs,
            ),
        ] {
            if matches.value_source(id) == Some(ValueSource::EnvVariable) {
                *rows = parse_env_list(key, rows.first().map_or("", String::as_str))?;
            } else {
                anyhow::ensure!(
                    rows.iter().all(|row| !row.trim().is_empty()),
                    "{key} has an empty element"
                );
            }
        }
        if matches.value_source("prometheus_metrics") == Some(ValueSource::EnvVariable) {
            self.prometheus_metrics = parse_env_list(
                "KRONIKA_PROMETHEUS_METRICS",
                self.prometheus_metrics
                    .first()
                    .map_or("", |p| p.to_str().unwrap_or("")),
            )?
            .into_iter()
            .map(PathBuf::from)
            .collect();
        }
        for dsn in &self.pgbouncer_dsns {
            dsn.parse::<tokio_postgres::Config>().map_err(|_error| {
                anyhow::anyhow!(
                    "--pgbouncer-dsn / KRONIKA_PGBOUNCER_DSNS is not a valid connection string"
                )
            })?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        validate_segment_max_bytes(self.segment_max_bytes)?;
        validate_journal_max_bytes(self.journal_max_bytes)?;
        if let Some(retention) = self.retention {
            retention.validate(self.segment_max_bytes)?;
        }
        anyhow::ensure!(
            self.postgres_effective_cpus != Some(0),
            "--postgres-effective-cpus / KRONIKA_POSTGRES_EFFECTIVE_CPUS must be greater than zero"
        );
        anyhow::ensure!(
            self.pg_dsn.is_some() || self.postgres_effective_cpus.is_none(),
            "--postgres-effective-cpus / KRONIKA_POSTGRES_EFFECTIVE_CPUS requires --pg-dsn / KRONIKA_PG_DSN"
        );
        anyhow::ensure!(
            self.mode.collect_os() || self.pg_dsn.is_some(),
            "--mode postgresql requires --pg-dsn / KRONIKA_PG_DSN"
        );
        anyhow::ensure!(
            self.mode.collect_os()
                || (self.pgbouncer_dsns.is_empty() && self.pgbouncer_logs.is_empty()),
            "--mode postgresql does not collect PgBouncer; remove PgBouncer DSN and log settings"
        );
        anyhow::ensure!(
            self.pg_log_max_lag_secs > 0,
            "--pg-log-max-lag-s / KRONIKA_PG_LOG_MAX_LAG_S must be greater than zero"
        );
        anyhow::ensure!(
            self.intervals.pg_statements_and_plans >= MIN_PG_STATEMENTS_INTERVAL_SECS,
            "--pg-statements-interval-s / KRONIKA_PG_STATEMENTS_INTERVAL_S must be at least {MIN_PG_STATEMENTS_INTERVAL_SECS} seconds"
        );
        // CFG-1: settings without the endpoint are meaningless.
        anyhow::ensure!(
            self.prometheus_listen.is_some()
                || (self.prometheus_metrics.is_empty() && self.prometheus_preset_is_default()),
            "--prometheus-metrics / --prometheus-preset require --prometheus-listen / KRONIKA_PROMETHEUS_LISTEN"
        );
        anyhow::ensure!(
            self.prometheus_listen.is_none() || self.pg_dsn.is_some(),
            "--prometheus-listen / KRONIKA_PROMETHEUS_LISTEN requires --pg-dsn / KRONIKA_PG_DSN"
        );
        Ok(())
    }

    fn prometheus_preset_is_default(&self) -> bool {
        self.prometheus_preset == kronika_prometheus::catalog::DEFAULT_PRESET
    }

    pub(crate) fn proc_fs(&self) -> ProcFs {
        ProcFs::new(
            self.proc_root
                .clone()
                .unwrap_or_else(|| PathBuf::from("/proc")),
        )
    }

    pub(crate) fn sys_fs(&self) -> SysFs {
        SysFs::new(self.sys_root.clone())
    }

    /// Log validated storage settings after the configured logger is ready.
    pub(crate) fn log_storage_settings(&self) {
        if self.segment_max_bytes > self.journal_max_bytes {
            log_event(
                LogLevel::Warn,
                "config_degraded",
                &[
                    field("reason", "segment_cap_exceeds_journal_cap"),
                    field("segment_max_bytes", self.segment_max_bytes),
                    field("journal_max_bytes", self.journal_max_bytes),
                ],
            );
        }
        if let Some(retention) = self.retention {
            retention.log();
        }
    }

    #[cfg(test)]
    pub(crate) fn from_env() -> Result<Self> {
        Ok(parse_from(["kronika-collector"])?)
    }
}

fn validate_journal_max_bytes(value: u64) -> Result<()> {
    anyhow::ensure!(
        (JOURNAL_HEADER_LEN as u64..=MAX_JOURNAL_LEN as u64).contains(&value),
        "--journal-max-bytes / KRONIKA_JOURNAL_MAX_BYTES must be in {JOURNAL_HEADER_LEN}..={MAX_JOURNAL_LEN}, got {value}"
    );
    Ok(())
}

fn validate_segment_max_bytes(value: u64) -> Result<()> {
    anyhow::ensure!(
        value > 0,
        "--segment-max-bytes / KRONIKA_SEGMENT_MAX_BYTES must be greater than zero"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/config/mode.rs"]
mod mode_tests;
