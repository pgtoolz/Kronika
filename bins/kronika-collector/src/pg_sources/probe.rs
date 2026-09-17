//! Resolve the connected database, login role, server major version, and stats visibility.
//!
//! This is a connection probe, not vendor detection. Core queries use the
//! `PostgreSQL` major version advertised by `server_version_num`. A fork must
//! provide the corresponding `PostgreSQL` catalogs and statistics views.
//! Extension schemas, layouts, and permissions are discovered separately from
//! their installed catalogs; they are not inferred from the server version.

use super::execution::{
    QUERY_TIMEOUT, open_session, postgres_connection_error, postgres_query_cancelled,
};
use super::measurement::measure;
use super::{PgObservation, PgSources};
use anyhow::Context as _;
use kronika_source_pg::{Session, query};

// Read the login role (`session_user`), which owns the monitoring connection.
// USAGE checks privileges usable now, rather than membership requiring SET ROLE.
pub(super) const SERVER_PROBE_SQL: &str = concat!(
    "/* kronika:",
    env!("CARGO_PKG_VERSION"),
    " bins/kronika-collector/src/pg_sources/probe.rs */ ",
    "SELECT current_setting('server_version_num') AS server_version_num, ",
    "d.oid::text AS datid, d.datname::text AS database, ",
    "r.oid::text AS usesysid, r.rolname::text AS username, ",
    "pg_catalog.pg_has_role('pg_read_all_stats', 'USAGE') AS full_visibility ",
    "FROM pg_catalog.pg_database AS d CROSS JOIN pg_catalog.pg_roles AS r ",
    "WHERE d.datname = current_database() AND r.rolname = session_user"
);

#[derive(Debug, Clone)]
pub(super) struct GenerationProbe {
    pub(super) generation: u64,
    pub(super) major: u32,
    pub(super) datid: u32,
    pub(super) database: String,
    pub(super) usesysid: u32,
    pub(super) user: String,
    pub(super) full_visibility: bool,
}

impl PgSources {
    pub(super) async fn read_probe(
        &mut self,
        refresh: bool,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> Option<GenerationProbe> {
        let cached = self.probe.clone();
        let server = self.server.as_mut()?;
        let database = server.database_label().to_owned();
        let connection = server.connection_label(0);
        let session = match open_session(server, observe).await {
            Ok(session) => session,
            Err(_failure) => {
                self.clear_primary_connection();
                return None;
            }
        };
        let generation = session.generation();
        let same_generation = cached
            .as_ref()
            .is_some_and(|cached| cached.generation == generation);
        if same_generation && !refresh {
            return cached;
        }
        let mut measured = measure(observe, "server_probe", &connection, &database);
        let result = query::timeout(
            session,
            QUERY_TIMEOUT,
            read_generation_probe(session, generation, measured.stats_mut()),
        )
        .await;
        match result {
            Ok(Ok(probe)) => {
                server.remember_resolved_identity(&probe.user, &probe.database);
                measured.resolve_identity(server.connection_label(0), probe.database.clone());
                measured.success();
                self.server_database = Some(probe.database.clone());
                self.update_probe_cache(probe.clone(), same_generation);
                Some(probe)
            }
            Ok(Err(error)) => {
                let cancelled = postgres_query_cancelled(&error);
                if cancelled {
                    measured.server_timeout(format!("{error:#}"));
                } else {
                    measured.error(format!("{error:#}"));
                }
                if !cancelled && postgres_connection_error(&error) {
                    self.clear_primary_connection();
                }
                None
            }
            Err(_elapsed) => {
                measured.timeout();
                self.clear_primary_connection();
                None
            }
        }
    }
}

async fn read_generation_probe(
    session: Session<'_>,
    generation: u64,
    stats: &mut query::QueryStats,
) -> anyhow::Result<GenerationProbe> {
    let mut rows = query::read_simple_rows(session, SERVER_PROBE_SQL, stats, |row| {
        let version = row
            .try_get("server_version_num")?
            .context("server probe omitted server_version_num")?
            .parse::<u32>()
            .context("parse server_version_num from server probe")?;
        let datid = row
            .try_get("datid")?
            .context("server probe omitted current database oid")?
            .parse::<u32>()
            .context("parse current database oid from server probe")?;
        let database = row
            .try_get("database")?
            .context("server probe omitted current_database")?;
        let usesysid = row
            .try_get("usesysid")?
            .context("server probe omitted session role oid")?
            .parse::<u32>()
            .context("parse session role oid from server probe")?;
        let user = row
            .try_get("username")?
            .context("server probe omitted session_user")?;
        let visibility = row
            .try_get("full_visibility")?
            .context("server probe omitted pg_read_all_stats usability")?;
        let full_visibility = match visibility {
            "t" => true,
            "f" => false,
            other => anyhow::bail!("server probe returned {other:?} for visibility"),
        };
        Ok(GenerationProbe {
            generation,
            major: version / 10_000,
            datid,
            database: database.to_owned(),
            usesysid,
            user: user.to_owned(),
            full_visibility,
        })
    })
    .await?;
    anyhow::ensure!(rows.len() == 1, "server probe returned {} rows", rows.len());
    Ok(rows.remove(0))
}
