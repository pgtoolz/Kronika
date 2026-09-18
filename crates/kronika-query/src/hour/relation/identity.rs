//! Stable relation group identities and physical source locators.

use kronika_reader::{Dictionary, Row};
use serde_json::{Value, json};

use super::{
    GroupKey, GroupKeyValue, Metric, RelationKind, RelationSource, text_cell, unsigned_cell,
};

use crate::{QueryError, RelationGroup};

impl GroupKey {
    /// Decode the stable relation grouping key from one projected row.
    ///
    /// # Errors
    ///
    /// Returns a query decoding error when a referenced dictionary value is invalid.
    pub fn from_row(
        kind: RelationKind,
        group: RelationGroup,
        row: &Row,
        dictionary: &Dictionary,
    ) -> Result<Option<Self>, QueryError> {
        if group == RelationGroup::Tablespace {
            return Ok(unsigned_cell(row.get("tablespace_oid"))
                .map(|tablespace_oid| Self(GroupKeyValue::Tablespace { tablespace_oid })));
        }
        let (Some(datid), Some(datname)) = (
            unsigned_cell(row.get("datid")),
            text_cell(row.get("datname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        if group == RelationGroup::Database {
            return Ok(Some(Self(GroupKeyValue::Database { datid, datname })));
        }
        let Some(schemaname) = text_cell(row.get("schemaname"), dictionary)? else {
            return Ok(None);
        };
        if group == RelationGroup::Schema {
            return Ok(Some(Self(GroupKeyValue::Schema {
                datid,
                datname,
                schemaname,
            })));
        }
        let (Some(relid), Some(relname)) = (
            unsigned_cell(row.get("relid")),
            text_cell(row.get("relname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        if kind == RelationKind::Tables {
            return Ok(Some(Self(GroupKeyValue::Table {
                datid,
                datname,
                schemaname,
                relid,
                relname,
            })));
        }
        let (Some(indexrelid), Some(indexrelname)) = (
            unsigned_cell(row.get("indexrelid")),
            text_cell(row.get("indexrelname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        Ok(Some(Self(GroupKeyValue::Index {
            datid,
            datname,
            schemaname,
            relid,
            relname,
            indexrelid,
            indexrelname,
        })))
    }

    /// Render the grouping key using the existing JSON contract.
    #[must_use]
    pub fn json(&self, kind: RelationKind, group: RelationGroup) -> Value {
        match self.clone().for_group(kind, group).0 {
            GroupKeyValue::Database { datid, datname } => json!({
                "datid": datid.to_string(),
                "datname": datname,
            }),
            GroupKeyValue::Schema {
                datid,
                datname,
                schemaname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
            }),
            GroupKeyValue::Tablespace { tablespace_oid } => {
                json!({ "tablespace_oid": tablespace_oid.to_string() })
            }
            GroupKeyValue::Table {
                datid,
                datname,
                schemaname,
                relid,
                relname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
                "relid": relid.to_string(),
                "relname": relname,
            }),
            GroupKeyValue::Index {
                datid,
                datname,
                schemaname,
                relid,
                relname,
                indexrelid,
                indexrelname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
                "relid": relid.to_string(),
                "relname": relname,
                "indexrelid": indexrelid.to_string(),
                "indexrelname": indexrelname,
            }),
        }
    }

    pub(super) fn for_group(self, _kind: RelationKind, group: RelationGroup) -> Self {
        if group == RelationGroup::Object {
            return self;
        }
        Self(match group {
            RelationGroup::Object => unreachable!("object grouping returned above"),
            RelationGroup::Database => match self.0 {
                GroupKeyValue::Table { datid, datname, .. }
                | GroupKeyValue::Index { datid, datname, .. } => {
                    GroupKeyValue::Database { datid, datname }
                }
                key => key,
            },
            RelationGroup::Schema => match self.0 {
                GroupKeyValue::Table {
                    datid,
                    datname,
                    schemaname,
                    ..
                }
                | GroupKeyValue::Index {
                    datid,
                    datname,
                    schemaname,
                    ..
                } => GroupKeyValue::Schema {
                    datid,
                    datname,
                    schemaname,
                },
                key => key,
            },
            RelationGroup::Tablespace => match self.0 {
                key @ GroupKeyValue::Tablespace { .. } => key,
                _ => unreachable!("tablespace keys are formed directly from physical rows"),
            },
        })
    }

    /// Text value of a named key component, when present.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        match (&self.0, name) {
            (
                GroupKeyValue::Database { datname, .. }
                | GroupKeyValue::Schema { datname, .. }
                | GroupKeyValue::Table { datname, .. }
                | GroupKeyValue::Index { datname, .. },
                "datname",
            ) => Some(datname),
            (
                GroupKeyValue::Schema { schemaname, .. }
                | GroupKeyValue::Table { schemaname, .. }
                | GroupKeyValue::Index { schemaname, .. },
                "schemaname",
            ) => Some(schemaname),
            (
                GroupKeyValue::Table { relname, .. } | GroupKeyValue::Index { relname, .. },
                "relname",
            ) => Some(relname),
            (GroupKeyValue::Index { indexrelname, .. }, "indexrelname") => Some(indexrelname),
            _ => None,
        }
    }

    /// Numeric or text metric represented by a named key component.
    #[must_use]
    #[allow(
        clippy::unnested_or_patterns,
        reason = "each tuple arm keeps the requested field name attached to its variants"
    )]
    pub fn metric(&self, name: &str) -> Option<Metric> {
        match (&self.0, name) {
            (
                GroupKeyValue::Database { datid, .. } | GroupKeyValue::Schema { datid, .. },
                "datid",
            )
            | (GroupKeyValue::Table { datid, .. } | GroupKeyValue::Index { datid, .. }, "datid") => {
                Some(Metric::integer(i128::from(*datid)))
            }
            (
                GroupKeyValue::Database { datname, .. } | GroupKeyValue::Schema { datname, .. },
                "datname",
            )
            | (
                GroupKeyValue::Table { datname, .. } | GroupKeyValue::Index { datname, .. },
                "datname",
            ) => Some(Metric::text(datname.clone())),
            (GroupKeyValue::Schema { schemaname, .. }, "schemaname")
            | (
                GroupKeyValue::Table { schemaname, .. } | GroupKeyValue::Index { schemaname, .. },
                "schemaname",
            ) => Some(Metric::text(schemaname.clone())),
            (GroupKeyValue::Tablespace { tablespace_oid }, "tablespace_oid") => {
                Some(Metric::integer(i128::from(*tablespace_oid)))
            }
            (GroupKeyValue::Table { relid, .. } | GroupKeyValue::Index { relid, .. }, "relid") => {
                Some(Metric::integer(i128::from(*relid)))
            }
            (
                GroupKeyValue::Table { relname, .. } | GroupKeyValue::Index { relname, .. },
                "relname",
            ) => Some(Metric::text(relname.clone())),
            (GroupKeyValue::Index { indexrelid, .. }, "indexrelid") => {
                Some(Metric::integer(i128::from(*indexrelid)))
            }
            (GroupKeyValue::Index { indexrelname, .. }, "indexrelname") => {
                Some(Metric::text(indexrelname.clone()))
            }
            _ => None,
        }
    }

    pub(super) const fn is_tablespace(&self) -> bool {
        matches!(&self.0, GroupKeyValue::Tablespace { .. })
    }
}

impl RelationSource {
    /// Bind a row to its stable source coordinates.
    #[must_use]
    pub const fn new(
        segment_id: i64,
        context_index: usize,
        ordinal: u64,
        type_id: u32,
        timestamp: i64,
    ) -> Self {
        Self {
            segment_id,
            context_index,
            ordinal,
            type_id,
            timestamp,
        }
    }

    /// Segment containing the source row.
    #[must_use]
    pub const fn segment_id(self) -> i64 {
        self.segment_id
    }

    /// Selected context index used by snapshot cursors.
    #[must_use]
    pub const fn context_index(self) -> usize {
        self.context_index
    }

    /// Source-row ordinal.
    #[must_use]
    pub const fn ordinal(self) -> u64 {
        self.ordinal
    }

    /// Physical type identifier.
    #[must_use]
    pub const fn type_id(self) -> u32 {
        self.type_id
    }

    /// Source-row timestamp in microseconds.
    #[must_use]
    pub const fn timestamp(self) -> i64 {
        self.timestamp
    }
}
