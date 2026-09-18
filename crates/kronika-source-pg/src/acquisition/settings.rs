//! Cache settings within one connection generation and emit only changed values.

use super::execution::{
    QUERY_TIMEOUT, QueryCompletion, deliver, finish_failed, session_for_generation,
};
use super::measurement::measure;
use super::probe::GenerationProbe;
use super::{PgBatch, PgObservation};
use crate::{
    Pool,
    query::{self, BatchWrite},
    settings::{self, SettingsRow},
};
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct CachedSettings {
    pub(super) generation: u64,
    pub(super) rows: Arc<[SettingsRow]>,
}

pub(super) fn settings_equal_ignoring_ts(left: &[SettingsRow], right: &[SettingsRow]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.datid == right.datid
                && left.datname == right.datname
                && left.usesysid == right.usesysid
                && left.usename == right.usename
                && left.name == right.name
                && left.setting == right.setting
                && left.unit == right.unit
                && left.source == right.source
                && left.sourcefile == right.sourcefile
                && left.sourceline == right.sourceline
                && left.pending_restart == right.pending_restart
                && left.context == right.context
                && left.vartype == right.vartype
                && left.boot_val == right.boot_val
                && left.reset_val == right.reset_val
        })
}

pub(super) fn cached_settings_for_generation(
    settings: Option<&CachedSettings>,
    generation: u64,
) -> Option<Arc<[SettingsRow]>> {
    settings
        .filter(|cached| cached.generation == generation)
        .map(|cached| Arc::clone(&cached.rows))
}

pub(super) async fn read_settings<E>(
    server: &mut Pool,
    probe: &GenerationProbe,
    cached: Option<&[SettingsRow]>,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<Option<Arc<[SettingsRow]>>, E> {
    let database = server.database_label().to_owned();
    let connection = server.connection_label(0);
    let session = match session_for_generation(server, probe.generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(None),
    };
    let mut measured = measure(observe, "pg_settings", &connection, &database);
    let result = query::timeout(
        session,
        QUERY_TIMEOUT,
        settings::collect(
            session,
            measured.stats_mut(),
            probe.datid,
            &probe.database,
            probe.usesysid,
            &probe.user,
        ),
    )
    .await;
    match result {
        Ok(Ok(rows)) => {
            let rows: Arc<[SettingsRow]> = Arc::from(rows);
            if cached.is_none_or(|cached| !settings_equal_ignoring_ts(cached, &rows)) {
                measured = deliver(measured, admit, PgBatch::Settings(Arc::clone(&rows)), None)?;
            }
            measured.success();
            Ok(Some(rows))
        }
        other => {
            if matches!(
                finish_failed(measured, other),
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut
            ) {
                server.close();
            }
            Ok(None)
        }
    }
}
