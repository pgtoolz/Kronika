//! Actual segment and physical-section inventory.

use std::collections::BTreeMap;

use kronika_reader::{SegmentKind, SegmentSection};
use kronika_registry::{logical_section_name, section_implementation, section_name};
use serde_json::{Value, json};

use crate::dataset::{
    DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject, QueryDataset,
    SegmentBounds, SegmentSelection,
};
use crate::render::record;
use crate::{
    CatalogRequest, QueryContext, QueryError, QuerySink, SOURCE_OS, SOURCE_POSTGRESQL, Window,
    source_bit,
};

/// Typed recorded-range and logical-section facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogFacts {
    /// First and last recorded timestamps, or `None` when no segment exists.
    pub recorded_range: Option<(i64, i64)>,
    /// Logical recorded sections in name order.
    pub sections: Vec<CatalogSection>,
}

/// One logical recorded section summarized across selected segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSection {
    /// Stable registry logical-section name.
    pub logical_name: &'static str,
    /// Recorded source family, when the layout belongs to one.
    pub source_family: Option<&'static str>,
    /// Total physical rows across the selected segments.
    pub rows: u64,
    /// Total encoded section bytes across the selected segments.
    pub bytes: u64,
    /// Stable union of recorded layout fields in name order.
    pub fields: Vec<CatalogField>,
}

/// One field available in a recorded logical section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogField {
    /// Stable registry field name.
    pub name: &'static str,
    /// Registry value-class code.
    pub class: &'static str,
    /// Registry unit code, when the field declares one.
    pub unit: Option<&'static str>,
}

/// Read typed facts from the same captured catalog used by streamed queries.
///
/// # Errors
///
/// Returns a captured-source error when catalog discovery cannot complete.
pub fn catalog_facts(
    context: &QueryContext,
    request: CatalogRequest,
) -> Result<CatalogFacts, QueryError> {
    let prepared = PreparedCatalog::prepare(
        context.dataset.as_ref(),
        request,
        context.configured_sources,
        context.synthetic_demo,
    )?;
    Ok(prepared.facts())
}

pub(crate) struct PreparedCatalog {
    listing: DatasetListing,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
    present_sources: Option<u32>,
    metric_sources: Option<u32>,
}

impl PreparedCatalog {
    pub(crate) fn prepare(
        dataset: &dyn QueryDataset,
        request: CatalogRequest,
        configured_sources: u32,
        synthetic_demo: bool,
    ) -> Result<Self, QueryError> {
        let catalog = dataset.catalog()?;
        let listing = catalog.segments(SegmentSelection::new(SegmentBounds::inclusive(
            request.window.from,
            request.window.to,
        )))?;
        Ok(Self {
            listing,
            window: request.window,
            configured_sources,
            synthetic_demo,
            present_sources: None,
            metric_sources: None,
        })
    }

    pub(crate) const fn from_listing(
        listing: DatasetListing,
        window: Window,
        configured_sources: u32,
        synthetic_demo: bool,
    ) -> Self {
        Self {
            listing,
            window,
            configured_sources,
            synthetic_demo,
            present_sources: None,
            metric_sources: None,
        }
    }

    pub(crate) const fn with_present_sources(
        mut self,
        present_sources: u32,
        metric_sources: u32,
    ) -> Self {
        self.present_sources = Some(present_sources);
        self.metric_sources = Some(metric_sources);
        self
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        let present_sources = self.present_sources.unwrap_or_else(|| {
            self.listing
                .segments
                .iter()
                .flat_map(DatasetSegment::sections)
                .filter_map(|section| source_bit(section.type_id))
                .fold(0_u32, |present, bit| present | bit)
        });
        let metric_sources = self.metric_sources.unwrap_or_else(|| {
            self.listing
                .segments
                .iter()
                .flat_map(DatasetSegment::sections)
                .filter_map(|section| metric_source_bit(section.type_id))
                .fold(0_u32, |present, bit| present | bit)
        });
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "catalog",
                "from": self.window.from.map(|value| value.to_string()),
                "to": self.window.to.map(|value| value.to_string()),
                "demo": self.synthetic_demo.then_some("synthetic"),
                "source_families": source_family_values(
                    self.configured_sources,
                    present_sources,
                    metric_sources,
                ),
            }))?)
        {
            return Ok(());
        }
        for segment in &self.listing.segments {
            if sink.cancelled() {
                return Ok(());
            }
            let value = match segment.kind() {
                SegmentKind::Finished => finished(segment),
                SegmentKind::Active => active(segment)?,
            };
            if !sink.record(record(value)?) {
                return Ok(());
            }
        }
        for warning in &self.listing.warnings {
            if sink.cancelled() || !sink.record(record(warning_value(*warning))?) {
                return Ok(());
            }
        }
        Ok(())
    }

    fn facts(&self) -> CatalogFacts {
        type FieldFacts = (&'static str, Option<&'static str>);
        type SectionFacts = (
            u64,
            u64,
            Option<&'static str>,
            BTreeMap<&'static str, FieldFacts>,
        );

        let mut totals = BTreeMap::<&'static str, SectionFacts>::new();
        for segment in &self.listing.segments {
            for section in segment.sections() {
                let Some(logical_name) = logical_section_name(section.type_id) else {
                    continue;
                };
                if logical_name.starts_with("dict.") {
                    continue;
                }
                let Some(contract) = kronika_registry::contract(section.type_id) else {
                    continue;
                };
                let (rows, bytes, _source_family, fields) =
                    totals.entry(logical_name).or_insert_with(|| {
                        (
                            0,
                            0,
                            source_bit(section.type_id).and_then(source_name),
                            BTreeMap::new(),
                        )
                    });
                *rows = rows.saturating_add(section.rows);
                *bytes = bytes.saturating_add(section.bytes);
                for column in contract.columns {
                    fields.entry(column.name).or_insert_with(|| {
                        (
                            column.class.code(),
                            column.unit.map(kronika_registry::Unit::code),
                        )
                    });
                }
            }
        }

        let sections = totals
            .into_iter()
            .map(
                |(logical_name, (rows, bytes, source_family, fields))| CatalogSection {
                    logical_name,
                    source_family,
                    rows,
                    bytes,
                    fields: fields
                        .into_iter()
                        .map(|(name, (class, unit))| CatalogField { name, class, unit })
                        .collect(),
                },
            )
            .collect();
        let recorded_range = self
            .listing
            .segments
            .iter()
            .map(DatasetSegment::min_ts)
            .min()
            .zip(
                self.listing
                    .segments
                    .iter()
                    .map(DatasetSegment::max_ts)
                    .max(),
            );
        CatalogFacts {
            recorded_range,
            sections,
        }
    }
}

fn finished(segment: &DatasetSegment) -> Value {
    json!({
        "record": "finished_segment",
        "id": segment.id().to_string(),
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "sections": section_values(segment.sections()),
    })
}

fn active(segment: &DatasetSegment) -> Result<Value, QueryError> {
    let position = segment.active_position().ok_or_else(|| {
        QueryError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "active segment has no committed WAL position",
        )))
    })?;
    Ok(json!({
        "record": "active_segment",
        "id": segment.id().to_string(),
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "cursor": {
            "segment_id": segment.id().to_string(),
            "wal_position": position.to_string(),
        },
        "sections": section_values(segment.sections()),
    }))
}

fn section_values(sections: &[SegmentSection]) -> Vec<Value> {
    sections
        .iter()
        .map(|section| {
            json!({
                "logical_name": logical_section_name(section.type_id),
                "physical_name": section_name(section.type_id),
                "type_id": section.type_id.to_string(),
                "implementation": section_implementation(section.type_id),
                "source_family": source_bit(section.type_id).and_then(source_name),
                "rows": section.rows.to_string(),
                "bytes": section.bytes.to_string(),
            })
        })
        .collect()
}

fn source_family_values(configured: u32, present: u32, metrics: u32) -> Vec<Value> {
    [("os", SOURCE_OS), ("postgresql", SOURCE_POSTGRESQL)]
        .into_iter()
        .map(|(name, bit)| {
            json!({
                "name": name,
                "configured": configured & bit != 0,
                "present": present & bit != 0,
                "metrics_present": metrics & bit != 0,
            })
        })
        .collect()
}

pub(crate) fn metric_source_bit(type_id: u32) -> Option<u32> {
    let bit = source_bit(type_id)?;
    let is_log = logical_section_name(type_id).is_some_and(|name| name.starts_with("pg_log_"));
    (!is_log).then_some(bit)
}

const fn source_name(bit: u32) -> Option<&'static str> {
    match bit {
        SOURCE_OS => Some("os"),
        SOURCE_POSTGRESQL => Some("postgresql"),
        _ => None,
    }
}

fn warning_value(warning: DatasetWarning) -> Value {
    let affected = match warning.subject {
        DatasetWarningSubject::Segment(id) => json!({
            "kind": "segment",
            "id": id.to_string(),
        }),
        DatasetWarningSubject::ActiveJournal => json!({ "kind": "active_journal" }),
        DatasetWarningSubject::ForeignEntry {
            name_hash,
            name_len,
        } => json!({
            "kind": "foreign_entry",
            "name_hash": name_hash.to_string(),
            "name_len": name_len.to_string(),
        }),
        DatasetWarningSubject::Other => json!({ "kind": "store_object" }),
    };
    json!({
        "record": "warning",
        "code": warning.code,
        "affected": affected,
    })
}

#[cfg(test)]
#[path = "catalog/tests.rs"]
mod tests;
