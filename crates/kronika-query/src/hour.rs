//! One composed hour of catalog, series, index, and lane records.

use std::sync::Arc;

use kronika_index::{finding_keys_for_sections, series_keys_for_sections};
use kronika_reader::{Cell, SegmentKind};
use kronika_registry::{ColumnClass, contract};
use serde_json::json;

use crate::catalog::{PreparedCatalog, metric_source_bit};
use crate::history::stream_plans;
use crate::index::stream_series;
use crate::projection::plans;
use crate::render::record;
use crate::source_bit;
use crate::{
    ActiveCursor, DataRequest, DatasetListing, DatasetSegment, HourPart, HourRequest,
    HourSeriesRequest, IndexProvider, QueryDataset, QueryError, QuerySink, QueryStability,
    SegmentBounds, SegmentRequest, SegmentSelection, Window,
};

mod lanes;
mod postgres_summary;
pub(crate) mod process_summary;
mod relation;

pub use relation::{
    GroupKey, Metric, RelationAggregate, RelationField, RelationKind, RelationSource,
    index_scan_rate_is_zero, key_fields, output_fields,
};

#[cfg(test)]
mod tests;

const SERIES: &str = "health";
const HOUR: i64 = 3_600_000_000;
const ALL_SOURCES: u32 = crate::SOURCE_OS | crate::SOURCE_POSTGRESQL;

#[derive(Default)]
struct SourcePresence {
    any: u32,
    metrics: u32,
}

pub(crate) struct PreparedHour {
    dataset: Arc<dyn QueryDataset>,
    indexes: Option<Arc<dyn IndexProvider>>,
    catalog: Option<PreparedCatalog>,
    listed: Vec<DatasetSegment>,
    segments: Vec<DatasetSegment>,
    window: Window,
    hours: Vec<i64>,
    series: Option<HourSeriesRequest>,
    part: HourPart,
    shape: String,
    validator_segments: Option<Vec<DatasetSegment>>,
}

pub(crate) fn prepare(
    dataset: Arc<dyn QueryDataset>,
    indexes: Option<Arc<dyn IndexProvider>>,
    request: HourRequest,
    configured_sources: u32,
    synthetic_demo: bool,
) -> Result<PreparedHour, QueryError> {
    let requested = request.window;
    let discovery = dataset.catalog()?;
    let hours = if request.part == HourPart::Lanes {
        Vec::new()
    } else {
        hours_of_ranges(discovery.ranges().iter().copied())
    };
    let window = requested.from.map_or_else(
        || latest_hour(&hours),
        |from| Window {
            from: Some(from),
            to: Some(requested.to.unwrap_or_else(|| hour_end(from))),
        },
    );
    let stored = discovery.segments(SegmentSelection::new(if request.series.is_some() {
        SegmentBounds::all()
    } else {
        SegmentBounds::inclusive(window.from, window.to)
    }))?;
    drop(discovery);
    let clean = stored.warnings.is_empty();
    let listed = stored.segments;
    let mut segments = listed
        .iter()
        .filter(|segment| overlaps_window(segment.min_ts(), segment.max_ts(), window))
        .cloned()
        .collect::<Vec<_>>();
    segments.sort_by_key(DatasetSegment::min_ts);
    if request.part == HourPart::Lanes {
        let expected = request.segments.as_deref().ok_or(QueryError::BadCursor)?;
        pin_segments(dataset.as_ref(), &mut segments, expected, request.active)?;
    }
    let shape = format!(
        "window={window:?};hours={hours:?};series={:?};part={:?};segments={:?};active={:?};sources={configured_sources};demo={synthetic_demo}",
        request.series, request.part, request.segments, request.active,
    );
    let validator_segments = if request.part == HourPart::Base || segments.is_empty() || !clean {
        None
    } else {
        let candidates = if request.series.is_some() {
            listed
                .iter()
                .filter(|segment| window.to.is_none_or(|to| segment.min_ts() <= to))
                .cloned()
                .collect()
        } else {
            segments.clone()
        };
        (!candidates.is_empty()
            && candidates
                .iter()
                .all(|segment| segment.kind() == SegmentKind::Finished))
        .then_some(candidates)
    };
    let catalog = (request.series.is_none() && request.part != HourPart::Lanes).then(|| {
        PreparedCatalog::from_listing(
            DatasetListing {
                segments: segments.clone(),
                warnings: stored.warnings,
            },
            window,
            configured_sources,
            synthetic_demo,
        )
    });
    Ok(PreparedHour {
        dataset,
        indexes,
        catalog,
        listed,
        segments,
        window,
        hours,
        series: request.series,
        part: request.part,
        shape,
        validator_segments,
    })
}

fn pin_segments(
    dataset: &dyn QueryDataset,
    segments: &mut [DatasetSegment],
    expected: &[i64],
    cursor: Option<ActiveCursor>,
) -> Result<(), QueryError> {
    if !segments
        .iter()
        .map(DatasetSegment::id)
        .eq(expected.iter().copied())
    {
        return Err(QueryError::BadCursor);
    }
    let active = segments
        .iter()
        .position(|segment| segment.kind() == SegmentKind::Active);
    match (active, cursor) {
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => Err(QueryError::BadCursor),
        (Some(index), Some(cursor)) => {
            if segments[index].id() != cursor.segment_id {
                return Err(QueryError::BadCursor);
            }
            segments[index] = dataset
                .at_active_position(&segments[index], cursor.wal_position)
                .map_err(|_error| QueryError::BadCursor)?;
            Ok(())
        }
    }
}

fn overlaps_window(min_ts: i64, max_ts: i64, window: Window) -> bool {
    window.from.is_none_or(|from| max_ts >= from) && window.to.is_none_or(|to| min_ts <= to)
}

fn hours_of_ranges(ranges: impl IntoIterator<Item = (i64, i64)>) -> Vec<i64> {
    let mut hours = Vec::new();
    for (min_ts, max_ts) in ranges {
        if min_ts > max_ts {
            continue;
        }
        for bucket in min_ts.div_euclid(HOUR)..=max_ts.div_euclid(HOUR) {
            if let Some(hour) = bucket.checked_mul(HOUR) {
                hours.push(hour);
            }
        }
    }
    hours.sort_unstable();
    hours.dedup();
    hours
}

fn latest_hour(hours: &[i64]) -> Window {
    hours.last().map_or_else(Window::default, |from| Window {
        from: Some(*from),
        to: Some(hour_end(*from)),
    })
}

const fn hour_end(from: i64) -> i64 {
    from.saturating_add(HOUR - 1)
}

impl PreparedHour {
    pub(crate) fn stability(&self) -> QueryStability {
        if self.validator_segments.is_some() {
            QueryStability::Immutable
        } else if self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished)
        {
            QueryStability::Revalidate
        } else {
            QueryStability::Mutable
        }
    }

    pub(crate) fn validator_input(&self) -> Option<(&'static str, &str, &[DatasetSegment])> {
        self.validator_segments
            .as_deref()
            .map(|segments| ("hour", self.shape.as_str(), segments))
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        if sink.cancelled() {
            return Ok(());
        }
        if self.part != HourPart::Lanes
            && !sink.record(record(json!({
                "record": "hour",
                "from": self.window.from.map(|value| value.to_string()),
                "to": self.window.to.map(|value| value.to_string()),
                "available_hours": self.hours.iter().map(ToString::to_string).collect::<Vec<_>>(),
            }))?)
        {
            return Ok(());
        }
        let Self {
            dataset,
            indexes,
            catalog,
            listed,
            segments,
            window,
            series,
            part,
            ..
        } = self;
        if let Some(series) = series {
            if series.section == postgres_summary::SECTION {
                postgres_summary::validate(&series)?;
                let segments = postgres_summary::with_previous(&listed, segments);
                return postgres_summary::stream(
                    dataset.as_ref(),
                    &segments,
                    window,
                    series.scope,
                    sink,
                );
            }
            if series.group.is_some() {
                return relation::stream_history(dataset.as_ref(), &listed, window, &series, sink);
            }
            if series.section == process_summary::SECTION {
                let segments = process_summary::with_predecessors(&listed, segments);
                return process_summary::stream(dataset.as_ref(), &segments, window, &series, sink);
            }
            for segment in &segments {
                if sink.cancelled()
                    || !emit_series(dataset.as_ref(), segment, window, &series, sink)?
                {
                    return Ok(());
                }
            }
            return Ok(());
        }
        if let Some(catalog) = catalog {
            let presence = source_presence(dataset.as_ref(), &segments, window, sink)?;
            catalog
                .with_present_sources(presence.any, presence.metrics)
                .stream(sink)?;
        }
        let mut lane_state = lanes::State::default();
        for segment in &segments {
            if sink.cancelled() {
                return Ok(());
            }
            if part != HourPart::Lanes
                && !emit_index(
                    indexes.as_deref().ok_or_else(missing_index_provider)?,
                    segment,
                    window,
                    sink,
                )?
            {
                return Ok(());
            }
            if part != HourPart::Base
                && !emit_lanes(
                    dataset.as_ref(),
                    segment,
                    window,
                    &mut lane_state,
                    part == HourPart::Lanes,
                    sink,
                )?
            {
                return Ok(());
            }
        }
        Ok(())
    }
}

fn missing_index_provider() -> QueryError {
    QueryError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this query context has no derived-index provider",
    )))
}

fn emit_index(
    indexes: &dyn IndexProvider,
    segment: &DatasetSegment,
    window: Window,
    sink: &mut dyn QuerySink,
) -> Result<bool, QueryError> {
    let mut keys = series_keys_for_sections(segment.sections(), SERIES);
    keys.extend(series_keys_for_sections(
        segment.sections(),
        "pg_stat_activity",
    ));
    keys.extend(finding_keys_for_sections(segment.sections()));
    keys.sort_unstable();
    keys.dedup();
    let resource = indexes.load(segment, SERIES, &keys)?;
    if !sink.record(record(json!({
        "record": "index",
        "segment": { "id": segment.id().to_string() },
        "logical_name": SERIES,
        "checksum": resource.index.checksum.map(|value| format!("{value:08x}")),
    }))?) {
        return Ok(false);
    }
    stream_series(SERIES, resource, Some(window), sink)
}

fn source_presence(
    dataset: &dyn QueryDataset,
    segments: &[DatasetSegment],
    window: Window,
    sink: &dyn QuerySink,
) -> Result<SourcePresence, QueryError> {
    let mut presence = SourcePresence::default();
    for segment in segments
        .iter()
        .filter(|segment| window.contains(segment.min_ts()) && window.contains(segment.max_ts()))
    {
        if sink.cancelled() || (presence.any == ALL_SOURCES && presence.metrics == ALL_SOURCES) {
            break;
        }
        for section in segment.sections().iter().filter(|section| section.rows > 0) {
            if presence.any == ALL_SOURCES && presence.metrics == ALL_SOURCES {
                break;
            }
            let Some(any) = source_bit(section.type_id) else {
                continue;
            };
            if contract(section.type_id).is_some_and(|contract| {
                contract
                    .columns
                    .iter()
                    .any(|column| column.class == ColumnClass::Timestamp)
            }) {
                presence.any |= any;
                if let Some(metrics) = metric_source_bit(section.type_id) {
                    presence.metrics |= metrics;
                }
            }
        }
    }
    for descriptor in segments
        .iter()
        .filter(|segment| !window.contains(segment.min_ts()) || !window.contains(segment.max_ts()))
    {
        if sink.cancelled() || (presence.any == ALL_SOURCES && presence.metrics == ALL_SOURCES) {
            break;
        }
        let needed = descriptor.sections().iter().any(|section| {
            let Some(any) = source_bit(section.type_id) else {
                return false;
            };
            presence.any & any == 0
                || metric_source_bit(section.type_id).is_some_and(|bit| presence.metrics & bit == 0)
        });
        if !needed {
            continue;
        }
        let segment = dataset.open(descriptor)?;
        for section in descriptor.sections() {
            if sink.cancelled() {
                break;
            }
            let type_id = section.type_id;
            let Some(any) = source_bit(type_id) else {
                continue;
            };
            let metrics = metric_source_bit(type_id);
            if presence.any & any != 0 && metrics.is_none_or(|bit| presence.metrics & bit != 0) {
                continue;
            }
            let Some(timestamp) = contract(type_id).and_then(|contract| {
                contract
                    .columns
                    .iter()
                    .find(|column| column.class == ColumnClass::Timestamp)
            }) else {
                continue;
            };
            let mut found = false;
            segment.visit_rows(type_id, &[timestamp.name], 0, usize::MAX, |_ordinal, row| {
                if sink.cancelled() {
                    return false;
                }
                if matches!(row.get(timestamp.name), Some(Cell::Ts(value)) if window.contains(*value))
                {
                    found = true;
                    return false;
                }
                true
            })?;
            if found {
                presence.any |= any;
                if let Some(metrics) = metrics {
                    presence.metrics |= metrics;
                }
            }
        }
    }
    Ok(presence)
}

fn emit_series(
    dataset: &dyn QueryDataset,
    descriptor: &DatasetSegment,
    window: Window,
    series: &HourSeriesRequest,
    sink: &mut dyn QuerySink,
) -> Result<bool, QueryError> {
    let segment = dataset.open(descriptor)?;
    let request = DataRequest {
        segment: SegmentRequest {
            segment_id: descriptor.id(),
            section: series.section.clone(),
        },
        fields: series.fields.clone(),
        filters: series.filters.clone(),
        type_id: series.type_id,
        after: None,
    };
    match plans(&segment, &request, true) {
        Ok(plans) => {
            if !sink.record(record(json!({
                "record": "series_segment",
                "segment": { "id": descriptor.id().to_string() },
            }))?) {
                return Ok(false);
            }
            stream_plans(&segment, &series.section, &plans, Some(window), sink)
        }
        Err(QueryError::NoSuchSection) => Ok(true),
        Err(error) => Err(error),
    }
}

fn emit_lanes(
    dataset: &dyn QueryDataset,
    descriptor: &DatasetSegment,
    window: Window,
    state: &mut lanes::State,
    include_context: bool,
    sink: &mut dyn QuerySink,
) -> Result<bool, QueryError> {
    let segment = dataset.open(descriptor)?;
    let (points, facts) = lanes::collect(&segment, window, state)?;
    if include_context && !sink.record(record(json!({
        "record": "lane_context",
        "segment_id": descriptor.id().to_string(),
        "postgresql_interval_seconds": facts.postgresql_interval_seconds.map(|value| value.to_string()),
        "environment": facts.environment,
    }))?) {
        return Ok(false);
    }
    for point in points {
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "lane",
                "segment_id": descriptor.id().to_string(),
                "lane": point.key,
                "ts": point.ts.to_string(),
                "value": point.value,
            }))?)
        {
            return Ok(false);
        }
    }
    Ok(true)
}
