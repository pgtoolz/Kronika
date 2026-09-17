//! Interning the text of a log event and buffering its row.

use anyhow::{Context as _, Result};
use kronika_registry::pg_log::{
    PgLogAutovacuum, PgLogCheckpoints, PgLogErrors, PgLogLifecycle, PgLogLockWaits,
    PgLogSlowQueries, PgLogTempFiles,
};
use kronika_registry::pgbouncer_events::PgBouncerEvents;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, SectionBuffers};

use super::LogRows;
use crate::buffering::buffer_row;

/// Move one read's events into the window.
///
/// # Errors
///
/// Returns an error when a string cannot be interned or a section buffer is
/// full.
pub(crate) fn push_log_sources(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &LogRows,
) -> Result<()> {
    push_errors(buffers, interner, rows)?;
    push_checkpoints(buffers, interner, rows)?;
    push_autovacuum(buffers, interner, rows)?;
    push_slow_queries(buffers, interner, rows)?;
    push_lock_waits(buffers, interner, rows)?;
    push_lifecycle(buffers, interner, rows)?;
    push_temp_files(buffers, interner, rows)?;
    push_pgbouncer(buffers, interner, rows)
}

// Keep each category's traversal separate, including source-file interning for
// batches whose category is empty, so string IDs and first-error behavior stay stable.
macro_rules! pg_log_buffer {
    ($name:ident, $category:ident, $section:ident, $interner:ident, $event:ident, {
        $($fields:tt)*
    }) => {
        fn $name(
            buffers: &mut SectionBuffers,
            $interner: &mut Interner,
            rows: &LogRows,
        ) -> Result<()> {
            for batch in &rows.postgres {
                let source_file = intern($interner, &batch.source_file)?;
                for $event in &batch.events.$category {
                    let row = $section {
                        ts: Ts($event.ts),
                        system_identifier: batch.system_identifier,
                        source_file,
                        $($fields)*
                    };
                    buffer_row(buffers, row)?;
                }
            }
            Ok(())
        }
    };
}

pg_log_buffer!(push_errors, errors, PgLogErrors, interner, group, {
    severity: group.severity.code(),
    category: group.category.code(),
    sqlstate: option(interner, group.sqlstate.as_deref())?,
    pattern: intern(interner, &group.pattern)?,
    count: group.count,
    sample: intern(interner, &group.sample)?,
    detail: option(interner, group.detail.as_deref())?,
    hint: option(interner, group.hint.as_deref())?,
    context: option(interner, group.context.as_deref())?,
    statement: option(interner, group.statement.as_deref())?,
    database: option(interner, group.database.as_deref())?,
    username: option(interner, group.username.as_deref())?,
});

pg_log_buffer!(push_checkpoints, checkpoints, PgLogCheckpoints, interner, event, {
    phase: event.phase.code(),
    reason: option(interner, event.reason.as_deref())?,
    seconds_apart: event.seconds_apart,
    buffers_written: event.buffers_written,
    write_ms: event.write_ms,
    sync_ms: event.sync_ms,
    total_ms: event.total_ms,
    distance_kb: event.distance_kb,
    estimate_kb: event.estimate_kb,
    wal_added: event.wal_added,
    wal_removed: event.wal_removed,
    wal_recycled: event.wal_recycled,
    sync_files: event.sync_files,
    longest_sync_ms: event.longest_sync_ms,
    average_sync_ms: event.average_sync_ms,
});

pg_log_buffer!(push_autovacuum, autovacuum, PgLogAutovacuum, interner, event, {
    kind: event.kind.code(),
    relation: option(interner, event.relation.as_deref())?,
    index_scans: event.index_scans,
    pages_removed: event.pages_removed,
    pages_remaining: event.pages_remaining,
    tuples_removed: event.tuples_removed,
    tuples_remaining: event.tuples_remaining,
    tuples_dead_not_removable: event.tuples_dead_not_removable,
    elapsed_ms: event.elapsed_ms,
    buffer_hits: event.buffer_hits,
    buffer_misses: event.buffer_misses,
    buffer_dirtied: event.buffer_dirtied,
    avg_read_rate_mbs: event.avg_read_rate_mbs,
    avg_write_rate_mbs: event.avg_write_rate_mbs,
    cpu_user_ms: event.cpu_user_ms,
    cpu_system_ms: event.cpu_system_ms,
    wal_records: event.wal_records,
    wal_fpi: event.wal_fpi,
    wal_bytes: event.wal_bytes,
});

pg_log_buffer!(push_slow_queries, slow_queries, PgLogSlowQueries, interner, query, {
    pattern: intern(interner, &query.pattern)?,
    sample: intern(interner, &query.sample)?,
    count: query.count,
    max_duration_ms: query.max_duration_ms,
    total_duration_ms: query.total_duration_ms,
});

pg_log_buffer!(push_lock_waits, lock_waits, PgLogLockWaits, interner, wait, {
    kind: wait.kind.code(),
    pid: wait.pid,
    lock_mode: option(interner, wait.lock_mode.as_deref())?,
    lock_target: option(interner, wait.lock_target.as_deref())?,
    duration_ms: wait.duration_ms,
    holding_pids: option(interner, wait.holding_pids())?,
    wait_queue: option(interner, wait.wait_queue())?,
    detail: option(interner, wait.detail.as_deref())?,
    context: option(interner, wait.context.as_deref())?,
    statement: option(interner, wait.statement.as_deref())?,
});

pg_log_buffer!(push_lifecycle, lifecycle, PgLogLifecycle, interner, event, {
    kind: event.kind.code(),
    pid: event.pid,
    signal: event.signal,
    shutdown_mode: option(interner, event.shutdown_mode.as_deref())?,
    message: intern(interner, &event.message)?,
    query_detail: option(interner, event.query_detail.as_deref())?,
});

pg_log_buffer!(push_temp_files, temp_files, PgLogTempFiles, interner, file, {
    path: option(interner, file.path.as_deref())?,
    size_bytes: file.size_bytes,
    statement: option(interner, file.statement.as_deref())?,
});

fn push_pgbouncer(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &LogRows,
) -> Result<()> {
    for batch in &rows.pgbouncer {
        let source_file = intern(interner, &batch.source_file)?;
        for event in &batch.events {
            let row = PgBouncerEvents {
                ts: Ts(event.ts),
                source_file,
                level: event.level.code(),
                database: option(interner, event.database.as_deref())?,
                username: option(interner, event.username.as_deref())?,
                host: option(interner, event.host.as_deref())?,
                text: intern(interner, &event.text)?,
            };
            buffer_row(buffers, row)?;
        }
    }
    Ok(())
}

fn intern(interner: &mut Interner, value: &str) -> Result<StrId> {
    interner
        .intern(value.as_bytes())
        .map(|id| StrId(id.get()))
        .with_context(|| format!("intern a log event string of {} bytes", value.len()))
}

fn option(interner: &mut Interner, value: Option<&str>) -> Result<Option<StrId>> {
    value.map(|value| intern(interner, value)).transpose()
}
