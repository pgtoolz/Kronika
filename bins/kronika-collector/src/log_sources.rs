//! Discover and follow `PostgreSQL` and `PgBouncer` log files.
//!
//! `discovery` refreshes server metadata and expands configured paths/globs.
//! `collection` reads bounded batches and acknowledges them after row admission.
//! Resume positions are stored by path in `<storage>/log.offsets`.

mod buffering;
mod collection;
mod discovery;
mod paths;
mod settings;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kronika_source_log::pgbouncer::PgBouncerLog;
use kronika_source_log::postgres::{Events, LogTimezone, PgLog};
use kronika_source_log::{Offsets, pgbouncer};

use crate::config::Config;
use crate::logging::{LogLevel, field, log_event};
use crate::pg_sources::PgObservation;
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::push_log_sources;

// Time between metadata/path refreshes, including retries of missing sources.
const RESCAN: Duration = Duration::from_mins(5);

/// What one read of one `PostgreSQL` log produced.
#[derive(Debug)]
pub(crate) struct PostgresBatch {
    pub(crate) system_identifier: Option<u64>,
    pub(crate) source_file: String,
    pub(crate) events: Events,
}

/// What one read of one `PgBouncer` log produced.
#[derive(Debug)]
pub(crate) struct PgBouncerBatch {
    pub(crate) source_file: String,
    pub(crate) events: Vec<pgbouncer::Event>,
}

/// Parsed log batches passed to the window writer for admission.
#[derive(Debug, Default)]
pub(crate) struct LogRows {
    pub(crate) postgres: Vec<PostgresBatch>,
    pub(crate) pgbouncer: Vec<PgBouncerBatch>,
}

/// One followed `PostgreSQL` log.
#[derive(Debug)]
struct PostgresSource {
    log: PgLog,
    system_identifier: Option<u64>,
}

/// What a rescan decided one `PostgreSQL` file should be read as.
#[derive(Debug, Clone, Default)]
struct PostgresFacts {
    system_identifier: Option<u64>,
    line_prefix: Option<String>,
    log_timezone: Option<LogTimezone>,
}

/// One configured server and the facts that survive a failed refresh.
#[derive(Debug)]
struct PostgresTarget {
    connection: settings::ConnectionTarget,
    transport: kronika_source_pg::Transport,
    system_identifier: Option<u64>,
    last_log: Option<PathBuf>,
    facts: PostgresFacts,
}

impl PostgresTarget {
    fn new(
        connection: settings::ConnectionTarget,
        transport: kronika_source_pg::Transport,
    ) -> Self {
        Self {
            connection,
            transport,
            system_identifier: None,
            last_log: None,
            facts: PostgresFacts::default(),
        }
    }
}

/// The configured logs and where each of them was left off.
#[derive(Debug)]
pub(crate) struct LogSources {
    offsets: Offsets,
    pg_dsn: Option<PostgresTarget>,
    discover_postgres_paths: bool,
    pg_logs: Vec<String>,
    pg_log_max_lag_secs: u64,
    pgbouncer_dsns: Vec<settings::ConnectionTarget>,
    pgbouncer_logs: Vec<String>,
    postgres: Vec<PostgresSource>,
    pgbouncer: Vec<PgBouncerLog>,
    next_scan: Option<Instant>,
}

impl LogSources {
    /// Take the configuration and resume from `<storage>/log.offsets`.
    ///
    /// # Errors
    ///
    /// Returns an opaque configuration error or the error of reading the
    /// offsets file. Log files and server connections are opened during rescans
    /// and collection.
    pub(crate) fn open(config: &Config) -> anyhow::Result<Self> {
        let pg_dsn = config
            .pg_dsn
            .as_deref()
            .map(|raw| {
                let connection = settings::ConnectionTarget::parse(raw, 0).map_err(|_error| {
                    anyhow::anyhow!("KRONIKA_PG_DSN is not a valid connection string")
                })?;
                let transport =
                    kronika_source_pg::Transport::from_ca_file(config.pg_ssl_root_cert.as_deref())?;
                Ok::<_, anyhow::Error>(PostgresTarget::new(connection, transport))
            })
            .transpose()?;
        let pgbouncer_dsns = parse_connections("KRONIKA_PGBOUNCER_DSNS", &config.pgbouncer_dsns)?;
        let offsets = Offsets::load(&config.storage_dir)?;
        Ok(Self {
            offsets,
            pg_dsn,
            discover_postgres_paths: config.mode.collect_os(),
            pg_logs: config.pg_logs.clone(),
            pg_log_max_lag_secs: config.pg_log_max_lag_secs,
            pgbouncer_dsns,
            pgbouncer_logs: config.pgbouncer_logs.clone(),
            postgres: Vec::new(),
            pgbouncer: Vec::new(),
            next_scan: None,
        })
    }

    /// Ask every configured server what it writes, expand every glob, and
    /// bring the followed set in line with what came back.
    pub(crate) async fn rescan(&mut self, observe: &mut (dyn FnMut(PgObservation) + Send)) {
        let now = Instant::now();
        if self.next_scan.is_some_and(|due| now < due) {
            return;
        }
        self.next_scan = Some(now + RESCAN);
        self.rescan_postgres(observe).await;
        self.rescan_pgbouncer(observe).await;
    }

    /// Read each followed file in bounded batches and offer every nonempty
    /// parsed batch to `admit` before acknowledging its input position.
    ///
    /// Returns `false` when `admit` safely rejected a batch for a later retry.
    ///
    /// # Errors
    ///
    /// Returns a fatal downstream error. File read errors remain best-effort
    /// source failures and do not stop the other configured files.
    pub(crate) fn collect(
        &mut self,
        due: &DueSet,
        mut admit: impl FnMut(&LogRows) -> anyhow::Result<bool>,
    ) -> anyhow::Result<bool> {
        if !due.has(SourceKind::Logs) {
            return Ok(true);
        }
        let mut offsets_changed = false;
        let result = self.collect_files(&mut admit, &mut offsets_changed);
        // Save earlier acknowledgements even when a later batch was rejected
        // or failed. A failed save may replay already recorded rows on restart.
        if offsets_changed && let Err(error) = self.offsets.save() {
            log_event(
                LogLevel::Warn,
                "log_offsets_save_failure",
                &[field("error", format!("{error:#}"))],
            );
        }
        result
    }
}

fn parse_connections(
    variable: &'static str,
    configured: &[String],
) -> anyhow::Result<Vec<settings::ConnectionTarget>> {
    configured
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            settings::ConnectionTarget::parse(raw, index).map_err(|_error| {
                anyhow::anyhow!("{variable}[{index}] is not a valid connection string")
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/log_sources.rs"]
mod tests;
