//! Database-local extension inventory and safe object qualification.
//!
//! `pg_extension` is local to one database. Discovery therefore runs this one
//! catalog query in every database and caches the result outside this module.

use anyhow::{Context as _, Result};
use tokio_postgres::SimpleQueryRow;

use crate::Session;
use crate::query::QueryStats;

/// One compact inventory of the two supported extensions.
///
/// Function checks include the exact input and result types and the current
/// role's execute privilege. Plan layouts are checked against the same
/// function's named output arguments; extra compatible outputs are harmless.
const INVENTORY_QUERY: &str = marked!(
    "WITH installed AS (\
         SELECT e.oid AS extension_oid, e.extname::text, e.extversion::text, \
                e.extnamespace AS schema_oid, n.nspname::text AS schema_name \
         FROM pg_catalog.pg_extension e \
         JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
         WHERE e.extname IN ('pg_stat_statements', 'pg_store_plans')\
     ), extension_functions AS (\
         SELECT i.extension_oid, p.oid AS function_oid, p.proname, p.proargtypes, \
                p.proallargtypes, p.proargmodes, p.proargnames, \
                p.prorettype, p.proretset, \
                pg_catalog.has_function_privilege(p.oid, 'EXECUTE') AS can_execute \
         FROM installed i \
         JOIN pg_catalog.pg_depend d \
           ON d.refclassid = 'pg_catalog.pg_extension'::pg_catalog.regclass \
          AND d.refobjid = i.extension_oid \
          AND d.classid = 'pg_catalog.pg_proc'::pg_catalog.regclass \
          AND d.deptype = 'e' \
         JOIN pg_catalog.pg_proc p ON p.oid = d.objid \
         WHERE p.pronamespace = i.schema_oid\
     ), required_columns(layout, attname, atttypid) AS (VALUES \
         ('ossc', 'userid', 'pg_catalog.oid'::pg_catalog.regtype), \
         ('ossc', 'dbid', 'pg_catalog.oid'::pg_catalog.regtype), \
         ('ossc', 'queryid', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'planid', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'plan', 'pg_catalog.text'::pg_catalog.regtype), \
         ('ossc', 'calls', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'total_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'min_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'max_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'mean_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'stddev_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'rows', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'shared_blks_hit', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'shared_blks_read', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'shared_blks_dirtied', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'shared_blks_written', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'local_blks_hit', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'local_blks_read', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'local_blks_dirtied', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'local_blks_written', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'temp_blks_read', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'temp_blks_written', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('ossc', 'shared_blk_read_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'shared_blk_write_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'local_blk_read_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'local_blk_write_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'temp_blk_read_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'temp_blk_write_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('ossc', 'first_call', 'pg_catalog.timestamptz'::pg_catalog.regtype), \
         ('ossc', 'last_call', 'pg_catalog.timestamptz'::pg_catalog.regtype), \
         ('datasentinel', 'relids', 'pg_catalog.oid[]'::pg_catalog.regtype), \
         ('datasentinel', 'cmd_type', 'pg_catalog.text'::pg_catalog.regtype), \
         ('vadv', 'userid', 'pg_catalog.oid'::pg_catalog.regtype), \
         ('vadv', 'dbid', 'pg_catalog.oid'::pg_catalog.regtype), \
         ('vadv', 'queryid', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'planid', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'queryid_stat_statements', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'plan', 'pg_catalog.text'::pg_catalog.regtype), \
         ('vadv', 'calls', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'slow_log_calls', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'total_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'min_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'max_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'mean_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'stddev_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'rows', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'shared_blks_hit', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'shared_blks_read', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'shared_blks_dirtied', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'shared_blks_written', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'local_blks_hit', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'local_blks_read', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'local_blks_dirtied', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'local_blks_written', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'temp_blks_read', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'temp_blks_written', 'pg_catalog.int8'::pg_catalog.regtype), \
         ('vadv', 'blk_read_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'blk_write_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'first_call', 'pg_catalog.timestamptz'::pg_catalog.regtype), \
         ('vadv', 'last_call', 'pg_catalog.timestamptz'::pg_catalog.regtype), \
         ('vadv', 'total_plan_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'min_plan_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'max_plan_time', 'pg_catalog.float8'::pg_catalog.regtype), \
         ('vadv', 'mean_plan_time', 'pg_catalog.float8'::pg_catalog.regtype)\
     ), function_outputs AS (\
         SELECT f.function_oid, f.proargnames[output.position] AS attname, \
                f.proallargtypes[output.position] AS atttypid \
         FROM extension_functions f \
         CROSS JOIN LATERAL pg_catalog.generate_subscripts(\
             f.proallargtypes, 1\
         ) AS output(position) \
         WHERE f.proargmodes[output.position] IN ('o', 'b', 't')\
     ), function_capabilities AS (\
         SELECT f.*, \
                NOT EXISTS (\
                    SELECT 1 FROM required_columns wanted \
                    WHERE wanted.layout = 'ossc' AND NOT EXISTS (\
                        SELECT 1 FROM function_outputs actual \
                        WHERE actual.function_oid = f.function_oid \
                          AND actual.attname = wanted.attname \
                          AND actual.atttypid = wanted.atttypid\
                    )\
                ) AS ossc_columns, \
                NOT EXISTS (\
                    SELECT 1 FROM required_columns wanted \
                    WHERE wanted.layout = 'vadv' AND NOT EXISTS (\
                        SELECT 1 FROM function_outputs actual \
                        WHERE actual.function_oid = f.function_oid \
                          AND actual.attname = wanted.attname \
                          AND actual.atttypid = wanted.atttypid\
                    )\
                ) AS vadv_columns \
         FROM extension_functions f\
     ) \
     SELECT i.extname, i.extversion, i.schema_name, \
            pg_catalog.has_schema_privilege(i.schema_oid, 'USAGE') AS schema_usable, \
            pg_catalog.pg_has_role('pg_read_all_stats', 'USAGE') AS full_visibility, \
            coalesce((SELECT bool_or(pg_catalog.has_table_privilege(c.oid, 'SELECT') \
                                    AND EXISTS (SELECT 1 FROM pg_catalog.pg_attribute a \
                                                WHERE a.attrelid = c.oid AND a.attname = 'dealloc' \
                                                  AND a.atttypid = 'pg_catalog.int8'::pg_catalog.regtype \
                                                  AND NOT a.attisdropped) \
                                    AND EXISTS (SELECT 1 FROM pg_catalog.pg_attribute a \
                                                WHERE a.attrelid = c.oid AND a.attname = 'stats_reset' \
                                                  AND a.atttypid = 'pg_catalog.timestamptz'::pg_catalog.regtype \
                                                  AND NOT a.attisdropped)) \
                      FROM pg_catalog.pg_depend d \
                      JOIN pg_catalog.pg_class c ON c.oid = d.objid \
                      WHERE d.refclassid = 'pg_catalog.pg_extension'::pg_catalog.regclass \
                        AND d.refobjid = i.extension_oid \
                        AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                        AND d.deptype = 'e' \
                        AND c.relnamespace = i.schema_oid \
                        AND c.relname = 'pg_stat_statements_info'), false) \
                AS statements_info, \
            coalesce((SELECT bool_or(pg_catalog.has_table_privilege(c.oid, 'SELECT') \
                                    AND EXISTS (SELECT 1 FROM pg_catalog.pg_attribute a \
                                                WHERE a.attrelid = c.oid AND a.attname = 'dealloc' \
                                                  AND a.atttypid = 'pg_catalog.int8'::pg_catalog.regtype \
                                                  AND NOT a.attisdropped) \
                                    AND EXISTS (SELECT 1 FROM pg_catalog.pg_attribute a \
                                                WHERE a.attrelid = c.oid AND a.attname = 'stats_reset' \
                                                  AND a.atttypid = 'pg_catalog.timestamptz'::pg_catalog.regtype \
                                                  AND NOT a.attisdropped)) \
                      FROM pg_catalog.pg_depend d \
                      JOIN pg_catalog.pg_class c ON c.oid = d.objid \
                      WHERE d.refclassid = 'pg_catalog.pg_extension'::pg_catalog.regclass \
                        AND d.refobjid = i.extension_oid \
                        AND d.classid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                        AND d.deptype = 'e' \
                        AND c.relnamespace = i.schema_oid \
                        AND c.relname = 'pg_store_plans_info'), false) \
                AS store_plans_info, \
            coalesce((SELECT bool_or(f.can_execute AND f.proretset \
                                    AND f.prorettype = 'pg_catalog.record'::pg_catalog.regtype \
                                    AND f.proargtypes = '16'::pg_catalog.oidvector) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_stat_statements'), false) \
                AS statements_reader, \
            coalesce((SELECT bool_or(f.can_execute AND f.proretset \
                                    AND f.prorettype = 'pg_catalog.record'::pg_catalog.regtype \
                                    AND f.proargtypes = ''::pg_catalog.oidvector \
                                    AND f.ossc_columns) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_store_plans'), false) \
                AS store_plans_zero_arg, \
            coalesce((SELECT bool_or(f.can_execute AND f.proretset \
                                    AND f.prorettype = 'pg_catalog.record'::pg_catalog.regtype \
                                    AND f.proargtypes = '16'::pg_catalog.oidvector \
                                    AND f.vadv_columns) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_store_plans'), false) \
                AS store_plans_bool_arg, \
            coalesce((SELECT bool_or(f.can_execute AND NOT f.proretset \
                                    AND f.prorettype = 'pg_catalog.text'::pg_catalog.regtype \
                                    AND f.proargtypes = '26 26 20 20'::pg_catalog.oidvector) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_store_plans_get_plan'), false) \
                AS store_plans_key_getter, \
            coalesce((SELECT bool_or(f.can_execute AND NOT f.proretset \
                                    AND f.prorettype = 'pg_catalog.text'::pg_catalog.regtype \
                                    AND f.proargtypes = '25'::pg_catalog.oidvector) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_store_plans_textplan'), false) \
                AS store_plans_text_converter, \
            coalesce((SELECT bool_or(f.ossc_columns \
                                    AND f.proargtypes = ''::pg_catalog.oidvector) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_store_plans'), false) \
                AS store_plans_ossc_columns, \
            coalesce((SELECT bool_or(f.vadv_columns \
                                    AND f.proargtypes = '16'::pg_catalog.oidvector) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_store_plans'), false) \
                AS store_plans_vadv_columns, \
            coalesce((SELECT bool_or(NOT EXISTS (\
                                        SELECT 1 FROM required_columns wanted \
                                        WHERE wanted.layout = 'datasentinel' AND NOT EXISTS (\
                                            SELECT 1 FROM function_outputs actual \
                                            WHERE actual.function_oid = f.function_oid \
                                              AND actual.attname = wanted.attname \
                                              AND actual.atttypid = wanted.atttypid\
                                        )\
                                    ) AND f.proargtypes = ''::pg_catalog.oidvector) \
                      FROM function_capabilities f \
                      WHERE f.extension_oid = i.extension_oid \
                        AND f.proname = 'pg_store_plans'), false) \
                AS store_plans_datasentinel_columns \
     FROM installed i ORDER BY i.extname"
);

/// An installed extension version, as `pg_extension.extversion` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtensionVersion {
    /// Major component, before the first dot.
    pub major: u32,
    /// Minor component; `0` when the version is a bare major.
    pub minor: u32,
}

/// A schema name read from `pg_namespace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionSchema(String);

impl ExtensionSchema {
    /// Keep a catalog schema name for later safe qualification.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The unquoted catalog spelling, for logs and comparisons.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }

    /// Qualify a fixed extension object and quote both identifiers.
    #[must_use]
    pub fn qualify(&self, object: &str) -> String {
        format!("{}.{}", quote_identifier(&self.0), quote_identifier(object))
    }
}

/// One installed extension and the exact interfaces usable by this role.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag records an independently checked catalog capability"
)]
pub struct InventoryEntry {
    /// Extension name from `pg_extension`.
    pub name: String,
    /// Extension-controlled version string.
    pub extversion: String,
    /// Installation schema from `pg_namespace`.
    pub schema: ExtensionSchema,
    /// Whether the current role has `USAGE` on the installation schema.
    pub schema_usable: bool,
    /// Whether the current role can immediately use `pg_read_all_stats`.
    pub full_visibility: bool,
    /// Whether an exact readable `pg_stat_statements_info` view exists.
    pub statements_info: bool,
    /// Whether an exact readable `pg_store_plans_info` view exists.
    pub store_plans_info: bool,
    /// Whether the executable boolean `pg_stat_statements` reader exists.
    pub statements_reader: bool,
    /// Whether the executable zero-argument plan reader exists.
    pub store_plans_zero_arg: bool,
    /// Whether the executable boolean-argument plan reader exists.
    pub store_plans_bool_arg: bool,
    /// Whether the executable four-key plan getter exists.
    pub store_plans_key_getter: bool,
    /// Whether the executable one-text-argument plan converter exists.
    pub store_plans_text_converter: bool,
    /// Whether the zero-argument reader has every OSSC-compatible output.
    pub store_plans_ossc_columns: bool,
    /// Whether the boolean reader has every vadv output.
    pub store_plans_vadv_columns: bool,
    /// Whether the zero-argument reader has Datasentinel's extra outputs.
    pub store_plans_datasentinel_columns: bool,
}

/// Read `major.minor` from an `extversion` string.
///
/// Anything after the second component is ignored: no extension this collector
/// reads changes its columns in a patch release, and a suffix like `1.10-beta`
/// still names the 1.10 column set.
#[must_use]
pub fn parse_version(text: &str) -> Option<ExtensionVersion> {
    let mut parts = text.split('.');
    let major = digits(parts.next()?)?;
    let minor = parts.next().map_or(Some(0), digits)?;
    Some(ExtensionVersion { major, minor })
}

/// The leading run of digits as a number, or `None` when there is none.
fn digits(text: &str) -> Option<u32> {
    let leading: String = text.chars().take_while(char::is_ascii_digit).collect();
    leading.parse().ok()
}

fn quote_identifier(identifier: &str) -> String {
    let mut quoted = String::with_capacity(identifier.len().saturating_add(2));
    quoted.push('"');
    for character in identifier.chars() {
        if character == '"' {
            quoted.push('"');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

fn column<'a>(row: &'a SimpleQueryRow, name: &str) -> Result<&'a str> {
    row.get(name)
        .with_context(|| format!("extension inventory omitted {name}"))
}

fn boolean(row: &SimpleQueryRow, name: &str) -> Result<bool> {
    match column(row, name)? {
        "t" => Ok(true),
        "f" => Ok(false),
        value => anyhow::bail!("extension inventory returned {value:?} for boolean {name}"),
    }
}

fn entry_from_row(row: &SimpleQueryRow) -> Result<InventoryEntry> {
    Ok(InventoryEntry {
        name: column(row, "extname")?.to_owned(),
        extversion: column(row, "extversion")?.to_owned(),
        schema: ExtensionSchema::new(column(row, "schema_name")?),
        schema_usable: boolean(row, "schema_usable")?,
        full_visibility: boolean(row, "full_visibility")?,
        statements_info: boolean(row, "statements_info")?,
        store_plans_info: boolean(row, "store_plans_info")?,
        statements_reader: boolean(row, "statements_reader")?,
        store_plans_zero_arg: boolean(row, "store_plans_zero_arg")?,
        store_plans_bool_arg: boolean(row, "store_plans_bool_arg")?,
        store_plans_key_getter: boolean(row, "store_plans_key_getter")?,
        store_plans_text_converter: boolean(row, "store_plans_text_converter")?,
        store_plans_ossc_columns: boolean(row, "store_plans_ossc_columns")?,
        store_plans_vadv_columns: boolean(row, "store_plans_vadv_columns")?,
        store_plans_datasentinel_columns: boolean(row, "store_plans_datasentinel_columns")?,
    })
}

/// Inventory the supported extensions installed in the connected database.
///
/// The Simple Query Protocol deliberately leaves no prepared statement behind.
///
/// # Errors
///
/// Returns a catalog query or result-decoding error.
pub async fn inventory(
    session: Session<'_>,
    stats: &mut QueryStats,
) -> Result<Vec<InventoryEntry>> {
    crate::query::read_simple_rows(session, INVENTORY_QUERY, stats, entry_from_row)
        .await
        .context("inventory PostgreSQL extensions")
}

#[cfg(test)]
mod tests;
