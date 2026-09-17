//! Primary-connection queries: fast activity snapshots and slower server counters.

use std::sync::Arc;

use kronika_source_pg::Pool;
use kronika_source_pg::query::{self, BatchWrite};
use kronika_source_pg::settings::SettingsRow;
use kronika_source_pg::{
    activity, archiver, bgwriter, checkpointer, database, io, locks, prepared_xacts,
    progress_vacuum, wal, wal_storage,
};

use super::execution::{
    QUERY_TIMEOUT, deliver, finish_batched, finish_failed, fixed_source_can_continue,
    session_for_generation,
};
use super::measurement::measure;
use super::probe::GenerationProbe;
use super::settings::{CachedSettings, read_settings};
use super::{PgBatch, PgObservation, PgSources};
use crate::logging::{LogLevel, field, log_event};

struct InstancePass<'a, A> {
    pool: &'a mut Pool,
    probe: &'a GenerationProbe,
    observe: &'a mut (dyn FnMut(PgObservation) + Send),
    settings: Option<&'a Arc<[SettingsRow]>>,
    admit: &'a mut A,
}

// Snapshot readers return one row or an optional row. They share the query
// timeout and acknowledge successful admission in the query's measurement.
macro_rules! snapshot {
    ($pass:ident, $name:literal =>
     $module:ident::$collect:ident($($argument:expr),*),
     |$row:ident| $batch:expr) => {{
        let connection = $pass.pool.connection_label(0);
        let session = match session_for_generation($pass.pool, $pass.probe.generation, $pass.observe) {
            Ok(session) => session,
            Err(_failure) => return Ok(false),
        };
        let mut measured = measure($pass.observe, $name, &connection, &$pass.probe.database);
        let result = query::timeout(
            session,
            QUERY_TIMEOUT,
            $module::$collect(session, $($argument,)* measured.stats_mut()),
        )
        .await;
        let can_continue = match result {
            Ok(Ok($row)) => {
                if let Some(batch) = $batch {
                    measured = deliver(measured, $pass.admit, batch, $pass.settings.cloned())?;
                }
                measured.success();
                true
            }
            other => fixed_source_can_continue(finish_failed(measured, other)),
        };
        if !can_continue {
            return Ok(false);
        }
    }};
}

// Streaming readers account for each admitted batch inside the source query.
// finish_batched preserves partial accounting and closes failed connections.
macro_rules! batches {
    ($pass:ident, $name:literal =>
     $module:ident::$collect:ident($($argument:expr),*),
     |$batch:ident| $rows:expr) => {{
        let connection = $pass.pool.connection_label(0);
        let session = match session_for_generation($pass.pool, $pass.probe.generation, $pass.observe) {
            Ok(session) => session,
            Err(_failure) => return Ok(false),
        };
        let mut measured = measure($pass.observe, $name, &connection, &$pass.probe.database);
        let result = $module::$collect(session, $($argument,)* measured.stats_mut(), |$batch| {
            ($pass.admit)($rows, $pass.settings.cloned())
        })
        .await;
        if !finish_batched($pass.pool, measured, result)? {
            return Ok(false);
        }
    }};
}

impl PgSources {
    /// Stop the pass on `false`; the caller discards the connection generation.
    pub(super) async fn collect_instance<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<bool, E> {
        let current = self.settings.as_ref().map(|cached| cached.rows.as_ref());
        if let Some(server) = self.server.as_mut()
            && let Some(rows) = read_settings(server, probe, current, observe, admit).await?
        {
            self.settings = Some(CachedSettings {
                generation: probe.generation,
                rows,
            });
        }
        let cached_settings = self.last_settings();
        let Some(pool) = self.server.as_mut() else {
            return Ok(false);
        };
        let major = probe.major;
        let pass = InstancePass {
            pool,
            probe,
            observe,
            settings: cached_settings.as_ref(),
            admit,
        };

        snapshot!(pass, "pg_stat_archiver" => archiver::collect_archiver(),
            |row| Some(PgBatch::Archiver(row)));
        snapshot!(pass, "pg_stat_bgwriter" => bgwriter::collect_bgwriter(major),
            |row| Some(PgBatch::Bgwriter(row)));
        if checkpointer::checkpointer_version(major).is_some() {
            snapshot!(pass, "pg_stat_checkpointer" => checkpointer::collect_checkpointer(major),
                |row| row.map(PgBatch::Checkpointer));
        }
        if wal_storage::wal_storage_query(major).is_some() {
            snapshot!(pass, "pg_wal_storage" => wal_storage::collect(major),
                |row| row.map(PgBatch::WalStorage));
        }
        if wal::wal_version(major).is_some() {
            snapshot!(pass, "pg_stat_wal" => wal::collect_wal(major),
                |row| row.map(PgBatch::Wal));
        }
        batches!(pass, "pg_prepared_xacts" => prepared_xacts::collect_prepared_xacts(),
            |batch| PgBatch::PreparedXacts(batch.rows));
        let database_version = database::database_version(major);
        batches!(pass, "pg_stat_database" => database::collect_database(major),
            |batch| PgBatch::Database(database_version, batch.rows));
        if let Some(version) = io::io_version(major) {
            batches!(pass, "pg_stat_io" => io::collect_io(major),
                |batch| PgBatch::Io(version, batch.rows));
        }
        Ok(true)
    }

    /// Read activity, blocking chains and vacuum progress on the fast schedule.
    pub(super) async fn collect_activity<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<bool, E> {
        let Some(pool) = self.server.as_mut() else {
            return Ok(false);
        };
        let major = probe.major;
        let pass = InstancePass {
            pool,
            probe,
            observe,
            settings: cached_settings,
            admit,
        };
        if probe.full_visibility {
            let activity_version = activity::activity_version(major);
            batches!(pass, "pg_stat_activity" => activity::collect_activity(major),
                |batch| PgBatch::Activity(activity_version, batch.rows));
            let locks_version = locks::locks_version(major);
            batches!(pass, "pg_locks" => locks::collect_locks(major),
                |batch| PgBatch::Locks(locks_version, batch.rows));
            batches!(pass, "pg_stat_progress_vacuum" => progress_vacuum::collect_progress_vacuum(major),
                |batch| PgBatch::ProgressVacuum(batch.rows));
        } else {
            log_event(
                LogLevel::Warn,
                "pg_stats_visibility_required",
                &[
                    field("database", &probe.database),
                    field("reason", "pg_read_all_stats_required"),
                ],
            );
        }
        Ok(true)
    }
}
