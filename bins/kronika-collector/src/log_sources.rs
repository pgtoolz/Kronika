//! Reading the configured log files into sections.
//!
//! A source is named one of two ways: a DSN, and then the server itself says
//! which file it writes and who it is, or a path or glob, and then the file is
//! read for what it holds and nothing is known about the writer. Both may be
//! given; a file reached both ways is followed once.
//!
//! Each file is followed from where the previous process stopped; the offsets
//! are keyed by path in `<storage>/log.offsets`, so a restart resumes instead of
//! re-reading or skipping. A file that cannot be read is one warning line
//! every rescan and no rows.

mod buffering;
mod paths;
mod settings;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use kronika_registry::SECTION_WRITE_BATCH_ROWS;
use kronika_source_log::pgbouncer::PgBouncerLog;
use kronika_source_log::postgres::{Events, LinePrefix, PgLog};
use kronika_source_log::{MAX_READ_BYTES, Offsets, pgbouncer};

use crate::config::Config;
use crate::logging::{
    LogLevel, field, log_collection_failure, log_collection_finish, log_collection_start, log_event,
};
use crate::pg_sources::PgObservation;
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::push_log_sources;

/// How often a server is asked again and the globs are expanded again. One
/// interval covers every way a source can be missing: the server is down, the
/// file is not there yet, a new instance appeared.
const RESCAN: Duration = Duration::from_mins(5);

/// The type id each source reports its collection under. A `PostgreSQL` read
/// produces seven sections at once, and the errors are the one an operator
/// looks for first.
const PG_LOG_TYPE_ID: u32 = 2_001_001;
const PGBOUNCER_TYPE_ID: u32 = 2_100_001;

/// Most raw bytes read from one file in one scheduled log cycle.
const MAX_SOURCE_READ_BYTES: usize = 256 * 1_048_576;

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

/// What one read of every configured log produced.
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
#[derive(Debug, Default)]
struct PostgresFacts {
    system_identifier: Option<u64>,
    line_prefix: Option<String>,
}

/// One configured server and the facts that survive a failed refresh.
#[derive(Debug)]
struct PostgresTarget {
    connection: settings::ConnectionTarget,
    system_identifier: Option<u64>,
    last_log: Option<(PathBuf, String)>,
}

impl PostgresTarget {
    const fn new(connection: settings::ConnectionTarget) -> Self {
        Self {
            connection,
            system_identifier: None,
            last_log: None,
        }
    }
}

/// The configured logs and where each of them was left off.
#[derive(Debug)]
pub(crate) struct LogSources {
    offsets: Offsets,
    pg_dsns: Vec<PostgresTarget>,
    pg_logs: Vec<String>,
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
    /// offsets file. Nothing is opened here: the first rescan finds what exists.
    pub(crate) fn open(config: &Config) -> anyhow::Result<Self> {
        let pg_dsns = parse_connections("KRONIKA_PG_DSNS", &config.pg_dsns)?
            .into_iter()
            .map(PostgresTarget::new)
            .collect();
        let pgbouncer_dsns = parse_connections("KRONIKA_PGBOUNCER_DSNS", &config.pgbouncer_dsns)?;
        let offsets = Offsets::load(&config.storage_dir)?;
        Ok(Self {
            offsets,
            pg_dsns,
            pg_logs: config.pg_logs.clone(),
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

    async fn rescan_postgres(&mut self, observe: &mut (dyn FnMut(PgObservation) + Send)) {
        let mut wanted: BTreeMap<PathBuf, PostgresFacts> = BTreeMap::new();
        for target in &mut self.pg_dsns {
            match settings::postgres(&target.connection, target.system_identifier, observe).await {
                Ok(server) => {
                    if let Some(identifier) = server.system_identifier {
                        target.system_identifier = Some(identifier);
                    }
                    if server.system_identifier.is_none() {
                        log_source_identity_unavailable(
                            target.connection.label(),
                            target.connection.source_index(),
                        );
                    }
                    let Some(path) = server.log_path else {
                        target.last_log = None;
                        log_source_absent(
                            target.connection.label(),
                            target.connection.source_index(),
                            "logging_collector is off, so there is no log file",
                        );
                        continue;
                    };
                    let path = PathBuf::from(path);
                    if !path.is_file() {
                        target.last_log = None;
                        log_source_unreadable(
                            &path,
                            target.connection.label(),
                            target.connection.source_index(),
                        );
                        continue;
                    }
                    target.last_log = Some((path.clone(), server.line_prefix.clone()));
                    wanted.insert(
                        path,
                        PostgresFacts {
                            system_identifier: target.system_identifier,
                            line_prefix: Some(server.line_prefix),
                        },
                    );
                }
                Err(_error) => {
                    log_source_unreachable(
                        "postgresql",
                        target.connection.label(),
                        target.connection.source_index(),
                    );
                    if let Some((path, line_prefix)) = &target.last_log {
                        wanted.insert(
                            path.clone(),
                            PostgresFacts {
                                system_identifier: target.system_identifier,
                                line_prefix: Some(line_prefix.clone()),
                            },
                        );
                    }
                }
            }
        }
        for entry in &self.pg_logs {
            for path in paths::expand(entry) {
                wanted.entry(path).or_default();
            }
        }
        self.postgres
            .retain(|source| wanted.contains_key(source.log.path()));
        for (path, facts) in wanted {
            let prefix = facts.line_prefix.as_deref().map(LinePrefix::parse);
            if let Some(existing) = self
                .postgres
                .iter_mut()
                .find(|source| source.log.path() == path)
            {
                existing.system_identifier = facts.system_identifier;
                if let Some(prefix) = prefix {
                    existing.log.set_prefix(prefix);
                }
                continue;
            }
            let position = self.offsets.get(&key(&path));
            let log = PgLog::new(path, position, prefix);
            log_source_opened("postgresql", log.path(), log.format().as_str());
            self.postgres.push(PostgresSource {
                log,
                system_identifier: facts.system_identifier,
            });
        }
    }

    async fn rescan_pgbouncer(&mut self, observe: &mut (dyn FnMut(PgObservation) + Send)) {
        let mut wanted: Vec<PathBuf> = Vec::new();
        for target in &self.pgbouncer_dsns {
            match settings::pgbouncer(target, observe).await {
                Ok(server) => {
                    let Some(path) = server.log_path else {
                        log_source_absent(
                            target.label(),
                            target.source_index(),
                            "logfile is unset, so the pooler writes to stderr",
                        );
                        continue;
                    };
                    let path = PathBuf::from(path);
                    if path.is_file() {
                        wanted.push(path);
                    } else {
                        log_source_unreadable(&path, target.label(), target.source_index());
                    }
                }
                Err(_error) => {
                    log_source_unreachable("pgbouncer", target.label(), target.source_index());
                }
            }
        }
        for entry in &self.pgbouncer_logs {
            wanted.extend(paths::expand(entry));
        }
        wanted.sort();
        wanted.dedup();
        self.pgbouncer
            .retain(|log| wanted.contains(&log.path().to_path_buf()));
        for path in wanted {
            if self.pgbouncer.iter().any(|log| log.path() == path) {
                continue;
            }
            let position = self.offsets.get(&key(&path));
            let log = PgBouncerLog::new(path, position);
            log_source_opened("pgbouncer", log.path(), "pgbouncer");
            self.pgbouncer.push(log);
        }
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
        now: i64,
        mut admit: impl FnMut(&LogRows) -> anyhow::Result<bool>,
    ) -> anyhow::Result<bool> {
        if !due.has(SourceKind::Logs) {
            return Ok(true);
        }
        let mut offsets_changed = false;
        let result = match self.collect_postgres(now, &mut admit, &mut offsets_changed) {
            Ok(true) => self.collect_pgbouncer(&mut admit, &mut offsets_changed),
            other => other,
        };
        if offsets_changed {
            save_offsets(&self.offsets);
        }
        result
    }

    fn collect_postgres(
        &mut self,
        now: i64,
        admit: &mut impl FnMut(&LogRows) -> anyhow::Result<bool>,
        offsets_changed: &mut bool,
    ) -> anyhow::Result<bool> {
        for source in &mut self.postgres {
            let started = Instant::now();
            let format = source.log.format().as_str();
            log_collection_start(PG_LOG_TYPE_ID, format);
            let mut raw_bytes = 0_usize;
            let mut event_rows = 0_usize;
            let mut read_failed = false;
            while next_batch_bytes(raw_bytes) != 0 {
                let batch = match source.log.read_batch(now, SECTION_WRITE_BATCH_ROWS) {
                    Ok(batch) => batch,
                    Err(error) => {
                        log_collection_failure(PG_LOG_TYPE_ID, format, &error, started.elapsed());
                        read_failed = true;
                        break;
                    }
                };
                raw_bytes = raw_bytes.saturating_add(batch.raw_bytes);
                event_rows = event_rows.saturating_add(batch.events.rows());
                let at_eof = batch.at_eof;
                let made_progress = batch.raw_bytes != 0 || batch.needs_ack;
                if batch.needs_ack {
                    let accepted = if batch.events.is_empty() {
                        true
                    } else {
                        let rows = LogRows {
                            postgres: vec![PostgresBatch {
                                system_identifier: source.system_identifier,
                                source_file: source.log.path().display().to_string(),
                                events: batch.events,
                            }],
                            pgbouncer: Vec::new(),
                        };
                        admit(&rows)?
                    };
                    if !accepted {
                        source.log.retry();
                        log_collection_finish(
                            PG_LOG_TYPE_ID,
                            format,
                            event_rows,
                            started.elapsed(),
                        );
                        return Ok(false);
                    }
                    let position = source
                        .log
                        .acknowledge()
                        .context("acknowledge the admitted PostgreSQL log batch")?;
                    self.offsets.set(&key(source.log.path()), position);
                    *offsets_changed = true;
                }
                if at_eof || !made_progress {
                    break;
                }
            }
            if !read_failed {
                log_collection_finish(PG_LOG_TYPE_ID, format, event_rows, started.elapsed());
            }
        }
        Ok(true)
    }

    fn collect_pgbouncer(
        &mut self,
        admit: &mut impl FnMut(&LogRows) -> anyhow::Result<bool>,
        offsets_changed: &mut bool,
    ) -> anyhow::Result<bool> {
        for log in &mut self.pgbouncer {
            let started = Instant::now();
            log_collection_start(PGBOUNCER_TYPE_ID, "pgbouncer");
            let mut raw_bytes = 0_usize;
            let mut event_rows = 0_usize;
            let mut read_failed = false;
            while next_batch_bytes(raw_bytes) != 0 {
                let batch = match log.read_batch(SECTION_WRITE_BATCH_ROWS) {
                    Ok(batch) => batch,
                    Err(error) => {
                        log_collection_failure(
                            PGBOUNCER_TYPE_ID,
                            "pgbouncer",
                            &error,
                            started.elapsed(),
                        );
                        read_failed = true;
                        break;
                    }
                };
                raw_bytes = raw_bytes.saturating_add(batch.raw_bytes);
                event_rows = event_rows.saturating_add(batch.events.len());
                let at_eof = batch.at_eof;
                let made_progress = batch.raw_bytes != 0 || batch.needs_ack;
                if batch.needs_ack {
                    let accepted = if batch.events.is_empty() {
                        true
                    } else {
                        let rows = LogRows {
                            postgres: Vec::new(),
                            pgbouncer: vec![PgBouncerBatch {
                                source_file: log.path().display().to_string(),
                                events: batch.events,
                            }],
                        };
                        admit(&rows)?
                    };
                    if !accepted {
                        log.retry();
                        log_collection_finish(
                            PGBOUNCER_TYPE_ID,
                            "pgbouncer",
                            event_rows,
                            started.elapsed(),
                        );
                        return Ok(false);
                    }
                    let position = log
                        .acknowledge()
                        .context("acknowledge the admitted PgBouncer log batch")?;
                    self.offsets.set(&key(log.path()), position);
                    *offsets_changed = true;
                }
                if at_eof || !made_progress {
                    break;
                }
            }
            if !read_failed {
                log_collection_finish(
                    PGBOUNCER_TYPE_ID,
                    "pgbouncer",
                    event_rows,
                    started.elapsed(),
                );
            }
        }
        Ok(true)
    }
}

const fn next_batch_bytes(read: usize) -> usize {
    if MAX_SOURCE_READ_BYTES.saturating_sub(read) >= MAX_READ_BYTES {
        MAX_READ_BYTES
    } else {
        0
    }
}

/// A restart re-reads from the last saved offset, so a failed save can only
/// duplicate rows already present in the WAL.
fn save_offsets(offsets: &Offsets) {
    if let Err(error) = offsets.save() {
        log_event(
            LogLevel::Warn,
            "log_offsets_save_failure",
            &[field("error", format!("{error:#}"))],
        );
    }
}

/// Offsets are keyed by the path, so a file keeps its place across restarts
/// and a file that stops existing stops being written out.
fn key(path: &std::path::Path) -> String {
    path.display().to_string()
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

fn log_source_opened(kind: &str, path: &std::path::Path, format: &str) {
    log_event(
        LogLevel::Info,
        "log_source_opened",
        &[
            field("kind", kind),
            field("path", path.display()),
            field("format", format),
        ],
    );
}

fn log_source_unreachable(kind: &str, connection: &str, source_index: usize) {
    log_event(
        LogLevel::Warn,
        "log_source_unreachable",
        &[
            field("kind", kind),
            field("connection", connection),
            field("source_index", source_index),
            field("reason", "connection_or_discovery_failed"),
        ],
    );
}

fn log_source_identity_unavailable(connection: &str, source_index: usize) {
    log_event(
        LogLevel::Warn,
        "log_source_identity_unavailable",
        &[
            field("kind", "postgresql"),
            field("connection", connection),
            field("source_index", source_index),
            field("reason", "pg_control_system_query_failed"),
        ],
    );
}

fn log_source_absent(connection: &str, source_index: usize, reason: &str) {
    log_event(
        LogLevel::Warn,
        "log_source_absent",
        &[
            field("connection", connection),
            field("source_index", source_index),
            field("reason", reason),
        ],
    );
}

fn log_source_unreadable(path: &std::path::Path, connection: &str, source_index: usize) {
    log_event(
        LogLevel::Warn,
        "log_source_unreadable",
        &[
            field("path", path.display()),
            field("connection", connection),
            field("source_index", source_index),
            field(
                "hint",
                "mount the directory here and name the file in KRONIKA_PG_LOGS",
            ),
        ],
    );
}

#[cfg(test)]
mod tests;
