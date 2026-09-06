//! `pg_settings` for section `1_019_001`.
//!
//! One query returns every parameter except `primary_conninfo` and
//! `ssl_passphrase_command`, whose values may contain secrets. The layout has
//! been stable since PG 10. Every row names the database and login role whose
//! effective configuration was read.

use anyhow::{Context as _, Result};
use kronika_registry::{PgSettings, StrId, Ts};
use tokio_postgres::types::Type;

use crate::query::{self, QueryStats};
use crate::{Session, intern_opt};

const SETTINGS_QUERY: &str = marked!(
    "SELECT \
         (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us, \
         name, left(setting, 65536) AS setting, unit, source, \
         left(sourcefile, 65536) AS sourcefile, sourceline, \
         pending_restart, context, vartype, \
         left(boot_val, 65536) AS boot_val, \
         left(reset_val, 65536) AS reset_val \
     FROM pg_settings \
     WHERE name NOT IN ('primary_conninfo', 'ssl_passphrase_command') \
     ORDER BY name"
);

/// One `pg_settings` row as the server sent it, before interning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRow {
    /// Collection time, unix microseconds.
    pub ts: i64,
    /// OID of the database used by the metric session.
    pub datid: u32,
    /// Name of the database used by the metric session.
    pub datname: String,
    /// OID of the login role used by the metric session.
    pub usesysid: u32,
    /// Name of the login role used by the metric session.
    pub usename: String,
    /// Parameter name.
    pub name: String,
    /// Running value, in `unit` units.
    pub setting: String,
    /// Unit of `setting`; `None` for unitless parameters.
    pub unit: Option<String>,
    /// How the running value was set.
    pub source: String,
    /// Config file that set the value.
    pub sourcefile: Option<String>,
    /// Line within `sourcefile`.
    pub sourceline: Option<i32>,
    /// The value changed but takes effect only after a restart.
    pub pending_restart: bool,
    /// Required context to change the value.
    pub context: String,
    /// Value type.
    pub vartype: String,
    /// Compiled-in default.
    pub boot_val: Option<String>,
    /// Value `RESET` would restore.
    pub reset_val: Option<String>,
}

/// Every `pg_settings` row, ordered by name.
///
/// # Errors
///
/// Returns the error of the query.
pub async fn collect(
    session: Session<'_>,
    stats: &mut QueryStats,
    datid: u32,
    datname: &str,
    usesysid: u32,
    usename: &str,
) -> Result<Vec<SettingsRow>> {
    query::read_all(
        session,
        SETTINGS_QUERY,
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(SettingsRow {
                ts: row.try_get("ts_us")?,
                datid,
                datname: datname.to_owned(),
                usesysid,
                usename: usename.to_owned(),
                name: row.try_get("name")?,
                setting: row.try_get("setting")?,
                unit: row.try_get("unit")?,
                source: row.try_get("source")?,
                sourcefile: row.try_get("sourcefile")?,
                sourceline: row.try_get("sourceline")?,
                pending_restart: row.try_get("pending_restart")?,
                context: row.try_get("context")?,
                vartype: row.try_get("vartype")?,
                boot_val: row.try_get("boot_val")?,
                reset_val: row.try_get("reset_val")?,
            })
        },
    )
    .await
    .context("read pg_settings")
}

/// Intern the row's strings and build the section row.
///
/// # Errors
///
/// Passes on whatever the interner returned.
pub fn to_section<E>(
    row: &SettingsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgSettings, E> {
    Ok(PgSettings {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        usesysid: row.usesysid,
        usename: intern(row.usename.as_bytes())?,
        name: intern(row.name.as_bytes())?,
        setting: intern(row.setting.as_bytes())?,
        unit: intern_opt(&mut intern, row.unit.as_deref())?,
        source: intern(row.source.as_bytes())?,
        sourcefile: intern_opt(&mut intern, row.sourcefile.as_deref())?,
        sourceline: row.sourceline,
        pending_restart: row.pending_restart,
        context: intern(row.context.as_bytes())?,
        vartype: intern(row.vartype.as_bytes())?,
        boot_val: intern_opt(&mut intern, row.boot_val.as_deref())?,
        reset_val: intern_opt(&mut intern, row.reset_val.as_deref())?,
    })
}

#[cfg(test)]
mod tests;
