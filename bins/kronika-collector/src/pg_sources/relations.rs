//! Read table and index statistics sequentially in every discovered database.

use super::execution::{
    QueryCompletion, QueryFailure, finish_batched_kind, open_session, session_for_generation,
};
use super::measurement::measure;
use super::probe::GenerationProbe;
use super::{PgBatch, PgObservation, PgSources};
use kronika_source_pg::{
    Pool, databases, query::BatchWrite, settings::SettingsRow, user_indexes, user_tables,
};
use std::sync::Arc;

impl PgSources {
    pub(super) async fn collect_relations<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        let found = self.discovered.clone();
        for database in &found {
            let is_current = self.server_database.as_deref() == Some(database.name.as_str());
            let result = {
                let Some(pool) = self.database_pool_mut(&database.name) else {
                    continue;
                };
                collect_relation_database(
                    pool,
                    database,
                    probe.major,
                    is_current.then_some(probe.generation),
                    observe,
                    cached_settings,
                    admit,
                )
                .await
            };
            match result {
                Err(error) => {
                    return Err(error);
                }
                Ok(RelationResult::Complete | RelationResult::SourceFailed) => {}
                Ok(RelationResult::ConnectionFailed | RelationResult::TimedOut) if is_current => {
                    self.clear_primary_connection();
                    return Ok(());
                }
                Ok(RelationResult::ConnectionFailed | RelationResult::TimedOut) => {
                    if let Some(pool) = self.database_pool_mut(&database.name) {
                        pool.close();
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelationResult {
    Complete,
    SourceFailed,
    ConnectionFailed,
    TimedOut,
}

async fn collect_relation_database<E>(
    pool: &mut Pool,
    database: &databases::Database,
    major: u32,
    expected_generation: Option<u64>,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<RelationResult, E> {
    let connection = pool.connection_label(0);
    let session = match match expected_generation {
        Some(generation) => session_for_generation(pool, generation, observe),
        None => open_session(pool, observe).await,
    } {
        Ok(session) => session,
        Err(QueryFailure::Timeout) => return Ok(RelationResult::TimedOut),
        Err(QueryFailure::Source | QueryFailure::ServerTimeout) => {
            return Ok(RelationResult::SourceFailed);
        }
        Err(QueryFailure::Connection) => return Ok(RelationResult::ConnectionFailed),
    };
    let generation = session.generation();
    let mut source_failed = false;
    let table_version = user_tables::user_tables_version(major);
    let mut measured = measure(observe, "pg_stat_user_tables", &connection, &database.name);
    let result =
        user_tables::collect_user_tables(session, database, major, measured.stats_mut(), |batch| {
            admit(
                PgBatch::UserTables(table_version, batch.rows),
                settings.cloned(),
            )
        })
        .await;
    match finish_batched_kind(pool, measured, result)? {
        QueryCompletion::Complete => {}
        QueryCompletion::SourceFailed
        | QueryCompletion::CapabilityChanged
        | QueryCompletion::ServerTimedOut => source_failed = true,
        QueryCompletion::ConnectionFailed => return Ok(RelationResult::ConnectionFailed),
        QueryCompletion::TimedOut => return Ok(RelationResult::TimedOut),
    }

    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(QueryFailure::Timeout) => return Ok(RelationResult::TimedOut),
        Err(QueryFailure::Source | QueryFailure::ServerTimeout) => {
            return Ok(RelationResult::SourceFailed);
        }
        Err(QueryFailure::Connection) => return Ok(RelationResult::ConnectionFailed),
    };
    let index_version = user_indexes::user_indexes_version(major);
    let mut measured = measure(observe, "pg_stat_user_indexes", &connection, &database.name);
    let result = user_indexes::collect_user_indexes(
        session,
        database,
        major,
        measured.stats_mut(),
        |batch| {
            admit(
                PgBatch::UserIndexes(index_version, batch.rows),
                settings.cloned(),
            )
        },
    )
    .await;
    Ok(match finish_batched_kind(pool, measured, result)? {
        QueryCompletion::Complete if source_failed => RelationResult::SourceFailed,
        QueryCompletion::Complete => RelationResult::Complete,
        QueryCompletion::SourceFailed
        | QueryCompletion::CapabilityChanged
        | QueryCompletion::ServerTimedOut => RelationResult::SourceFailed,
        QueryCompletion::ConnectionFailed => RelationResult::ConnectionFailed,
        QueryCompletion::TimedOut => RelationResult::TimedOut,
    })
}
