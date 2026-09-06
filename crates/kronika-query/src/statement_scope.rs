//! Statements issued by the collector start with `/* kronika:`.

use std::collections::HashSet;

use kronika_reader::{ReaderError, Row, Segment};
use kronika_registry::{Cell, logical_section_name};

#[cfg(test)]
mod tests;

/// Exact byte prefix in front of every statement the collector runs itself.
pub const COLLECTOR_STATEMENT_PREFIX: &[u8] = b"/* kronika:";

/// Logical section whose rows carry the statement text the scope inspects.
pub const STATEMENTS_SECTION: &str = "pg_stat_statements";

const QUERY_COLUMN: &str = "query";

/// Which statements a statement-bearing result includes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum StatementScope {
    /// Every recorded statement, including the collector's own.
    #[default]
    All,
    /// Only statements whose text does not start with the collector prefix.
    Workload,
}

impl StatementScope {
    /// Parse the public `scope` parameter value.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "all" => Some(Self::All),
            "workload" => Some(Self::Workload),
            _ => None,
        }
    }

    /// Public parameter value of this scope.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Workload => "workload",
        }
    }

    /// Whether the row-bearing section supports this scope.
    #[must_use]
    pub fn allows_rows(self, section: &str) -> bool {
        self == Self::All || section == STATEMENTS_SECTION
    }

    /// Whether the composed series supports this scope.
    #[must_use]
    pub fn allows_series(self, section: Option<&str>) -> bool {
        self == Self::All || section == Some("postgresql_summary")
    }

    /// Whether rows of `logical_name` are filtered under this scope.
    #[must_use]
    pub fn filters(self, logical_name: &str) -> bool {
        self == Self::Workload && logical_name == STATEMENTS_SECTION
    }
}

/// Segment dictionary IDs whose stored bytes begin with the collector prefix.
#[derive(Debug, Default)]
pub(crate) struct CollectorStatements {
    ids: HashSet<u64>,
}

impl CollectorStatements {
    pub(crate) fn scan(segment: &Segment) -> Result<Self, ReaderError> {
        if segment.layouts(STATEMENTS_SECTION).next().is_none() {
            return Ok(Self::default());
        }
        Ok(Self {
            ids: segment.dictionary_ids_with_prefix(COLLECTOR_STATEMENT_PREFIX)?,
        })
    }

    /// Whether `row` is one of the collector's own statements.
    pub(crate) fn excludes(&self, row: &Row) -> bool {
        matches!(row.get(QUERY_COLUMN), Some(Cell::StrId(id)) if self.ids.contains(id))
    }
}

pub(crate) fn statement_key(row: &Row) -> Option<[i64; 3]> {
    let query_column =
        if logical_section_name(row.contract().type_id.get()) == Some("pg_store_plans") {
            plan_statement_query_id_columns(row.contract().type_id.get())[0]
        } else {
            "queryid"
        };
    let Cell::I64(queryid) = row.get(query_column)? else {
        return None;
    };
    let (Cell::U32(dbid), Cell::U32(userid)) = (row.get("dbid")?, row.get("userid")?) else {
        return None;
    };
    (*queryid != 0).then_some([i64::from(*dbid), i64::from(*userid), *queryid])
}

pub(crate) const fn plan_statement_query_id_columns(type_id: u32) -> &'static [&'static str] {
    match type_id {
        1_004_001 => &["queryid_stat_statements"],
        _ => &["queryid"],
    }
}
