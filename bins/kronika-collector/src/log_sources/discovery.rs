//! Refresh server log metadata and reconcile the files being followed.

use anyhow::Context as _;
use std::collections::BTreeMap;
use std::path::PathBuf;

use kronika_source_log::pgbouncer::PgBouncerLog;
use kronika_source_log::postgres::{LinePrefix, LogTimezone, PgLog};

use super::{LogSources, PostgresFacts, PostgresSource, paths, settings};
use crate::logging::{LogLevel, field, log_event};
use crate::pg_sources::PgObservation;

impl LogSources {
    pub(super) async fn rescan_postgres(
        &mut self,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) {
        let mut wanted: BTreeMap<PathBuf, PostgresFacts> = BTreeMap::new();
        for target in self
            .pg_dsn
            .iter_mut()
            .filter(|_| self.discover_postgres_paths || !self.pg_logs.is_empty())
        {
            match settings::postgres(
                &target.connection,
                &target.transport,
                target.system_identifier,
                self.discover_postgres_paths,
                |name| LogTimezone::parse(name).context("resolve PostgreSQL log_timezone"),
                observe,
            )
            .await
            {
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
                    target.facts = PostgresFacts {
                        system_identifier: target.system_identifier,
                        line_prefix: Some(server.line_prefix),
                        log_timezone: Some(server.log_timezone),
                    };
                    if !self.discover_postgres_paths {
                        continue;
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
                    target.last_log = Some(path.clone());
                    wanted.insert(path, target.facts.clone());
                }
                Err(_error) => {
                    log_source_unreachable(
                        "postgresql",
                        target.connection.label(),
                        target.connection.source_index(),
                    );
                    if let Some(path) = &target.last_log
                        && self.discover_postgres_paths
                    {
                        wanted.insert(path.clone(), target.facts.clone());
                    }
                }
            }
        }
        for entry in &self.pg_logs {
            for path in paths::expand(entry) {
                wanted.entry(path).or_insert_with(|| {
                    self.pg_dsn
                        .as_ref()
                        .map(|target| target.facts.clone())
                        .unwrap_or_default()
                });
            }
        }
        self.follow_postgres(wanted);
    }

    fn follow_postgres(&mut self, wanted: BTreeMap<PathBuf, PostgresFacts>) {
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
                if let Some(timezone) = facts.log_timezone {
                    existing.log.set_timezone(timezone);
                }
                continue;
            }
            let position = self.offsets.get(&path.display().to_string());
            let mut log = PgLog::new(path, position, prefix);
            if let Some(timezone) = facts.log_timezone {
                log.set_timezone(timezone);
            }
            log_source_opened("postgresql", log.path(), log.format().as_str());
            self.postgres.push(PostgresSource {
                log,
                system_identifier: facts.system_identifier,
            });
        }
    }

    pub(super) async fn rescan_pgbouncer(
        &mut self,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) {
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
            let position = self.offsets.get(&path.display().to_string());
            let log = PgBouncerLog::new(path, position);
            log_source_opened("pgbouncer", log.path(), "pgbouncer");
            self.pgbouncer.push(log);
        }
    }
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
                "mount the directory here and name the file with --pg-log or KRONIKA_PG_LOGS",
            ),
        ],
    );
}
