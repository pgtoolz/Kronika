//! The databases a per-database section has to visit.
//!
//! `pg_stat_user_tables` and its neighbours only ever report the database the
//! connection is attached to, so those sections need one connection per
//! database. Discovery refreshes the list periodically: a database created
//! between scans starts being collected, and one that was dropped stops.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result};
use tokio_postgres::types::Type;

use crate::query::QueryStats;
use crate::{Pool, Session, query};

const ENUMERATE_QUERY: &str = marked!(
    "SELECT oid, datname::text AS datname, \
            datname = pg_catalog.current_database() AS is_current \
     FROM pg_catalog.pg_database \
     WHERE datname = pg_catalog.current_database() \
        OR (datallowconn AND NOT datistemplate \
            AND pg_catalog.has_database_privilege(oid, 'CONNECT')) \
     ORDER BY datname"
);

/// A database this collector can connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Database {
    /// Database oid, the `datid` every per-database row carries.
    pub oid: u32,
    /// Database name, which is also the key of its connection.
    pub name: String,
    /// Whether the bootstrap connection is already attached to this database.
    pub is_current: bool,
}

/// Every database this role can use for collection, ordered by name.
///
/// Templates are left out unless the bootstrap connection is already using
/// one: opening another session to a template blocks `CREATE DATABASE`, and a
/// template holds no workload worth recording.
///
/// # Errors
///
/// Returns the error of the query.
pub async fn enumerate(session: Session<'_>, stats: &mut QueryStats) -> Result<Vec<Database>> {
    query::read_all(
        session,
        ENUMERATE_QUERY,
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(Database {
                oid: row.try_get("oid")?,
                name: row.try_get("datname")?,
                is_current: row.try_get("is_current")?,
            })
        },
    )
    .await
    .context("list the databases to collect")
}

/// Bring `pools` in line with `found`, using `primary` as the connection
/// template.
///
/// A database that appeared gets a pool; one that disappeared has its pool
/// dropped, which closes the connection. Pools that stay keep their healthy
/// session, so a tick does not restart every connection. The current database
/// keeps using the bootstrap pool rather than opening a duplicate connection.
pub fn refresh(pools: &mut BTreeMap<String, Pool>, found: &[Database], primary: &Pool) {
    pools.retain(|name, _pool| {
        found
            .iter()
            .any(|database| !database.is_current && &database.name == name)
    });
    for database in found.iter().filter(|database| !database.is_current) {
        pools
            .entry(database.name.clone())
            .or_insert_with(|| primary.on_database(&database.name));
    }
}

#[cfg(test)]
mod tests;
