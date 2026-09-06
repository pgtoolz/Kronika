//! Exact complete-set histories for the process summary cards.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use kronika_reader::{Cell, Row, Segment};
use kronika_registry::{contract, logical_section_name};
use serde_json::{Value, json};

use crate::render::record;
use crate::{DatasetSegment, HourSeriesRequest, QueryDataset, QueryError, QuerySink, Window};

pub(super) const SECTION: &str = "os_process_summary";

const PROCESS: &str = "os_process";
const ACTIVITY: &str = "pg_stat_activity";
const DERIVED_TYPE_ID: &str = "0";

const PROCESS_COLUMNS: [&str; 17] = [
    "ts",
    "pid",
    "starttime",
    "state",
    "num_threads",
    "utime",
    "stime",
    "rundelay_ns",
    "nvcsw",
    "nivcsw",
    "rmem_kb",
    "vmem_kb",
    "vswap_kb",
    "majflt",
    "read_bytes",
    "write_bytes",
    "syscr",
];

const PROCESS_OPTIONAL_COLUMNS: [&str; 1] = ["syscw"];

#[derive(Clone, Copy)]
struct FieldSpec {
    name: &'static str,
    unit: &'static str,
    nullable: bool,
}

const FIELDS: [FieldSpec; 16] = [
    field("processes", "count", false),
    field("threads", "count", true),
    field("runnable", "count", false),
    field("postgresql", "count", false),
    field("user_cores", "count", true),
    field("system_cores", "count", true),
    field("run_delay_ms_per_second", "milliseconds_per_second", true),
    field("context_switches_per_second", "per_second", true),
    field("resident_kib", "kibibytes", true),
    field("virtual_kib", "kibibytes", true),
    field("swap_kib", "kibibytes", true),
    field("major_faults_per_second", "per_second", true),
    field("read_bytes_per_second", "bytes_per_second", true),
    field("write_bytes_per_second", "bytes_per_second", true),
    field("read_calls_per_second", "per_second", true),
    field("write_calls_per_second", "per_second", true),
];

const fn field(name: &'static str, unit: &'static str, nullable: bool) -> FieldSpec {
    FieldSpec {
        name,
        unit,
        nullable,
    }
}

#[derive(Clone, Copy, Default)]
struct Counters {
    utime: Option<i128>,
    stime: Option<i128>,
    rundelay_ns: Option<i128>,
    nvcsw: Option<i128>,
    nivcsw: Option<i128>,
    majflt: Option<i128>,
    read_bytes: Option<i128>,
    write_bytes: Option<i128>,
    syscr: Option<i128>,
    syscw: Option<i128>,
}

#[derive(Clone, Copy)]
struct Previous {
    ts: i64,
    starttime: Option<i128>,
    counters: Counters,
}

struct MomentIndex {
    previous: BTreeMap<i64, i64>,
    last_by_segment: HashMap<i64, i64>,
}

#[derive(Default)]
struct ExactSum {
    value: i128,
    values: usize,
}

impl ExactSum {
    const fn add(&mut self, value: Option<i128>) {
        if let Some(value) = value {
            self.value += value;
            self.values += 1;
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "complete process totals are converted only after exact integer summation"
    )]
    fn value(&self) -> Option<f64> {
        (self.values != 0).then_some(self.value as f64)
    }
}

#[derive(Default)]
struct RateSum {
    value: f64,
    values: usize,
}

impl RateSum {
    fn add(&mut self, current: Option<i128>, previous: Option<i128>, seconds: Option<f64>) {
        let (Some(current), Some(previous), Some(seconds)) = (current, previous, seconds) else {
            return;
        };
        let delta = current - previous;
        if delta < 0 || seconds <= 0.0 {
            return;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "integer counter deltas are converted only after exact subtraction"
        )]
        let rate = delta as f64 / seconds;
        if rate.is_finite() {
            self.value += rate;
            self.values += 1;
        }
    }

    fn value(&self) -> Option<f64> {
        (self.values != 0).then_some(self.value)
    }
}

#[derive(Default)]
struct Summary {
    segment_id: i64,
    ticks_per_second: Option<f64>,
    processes: u64,
    threads: ExactSum,
    runnable: u64,
    postgresql: u64,
    utime: RateSum,
    stime: RateSum,
    rundelay_ns: RateSum,
    nvcsw: RateSum,
    nivcsw: RateSum,
    resident_kib: ExactSum,
    virtual_kib: ExactSum,
    swap_kib: ExactSum,
    majflt: RateSum,
    read_bytes: RateSum,
    write_bytes: RateSum,
    syscr: RateSum,
    syscw: RateSum,
}

impl Summary {
    fn values(&self, fields: &[FieldSpec]) -> Vec<Value> {
        fields
            .iter()
            .map(|field| match field.name {
                "processes" => count_value(self.processes),
                "threads" => finite(self.threads.value()),
                "runnable" => count_value(self.runnable),
                "postgresql" => count_value(self.postgresql),
                "user_cores" => finite(divide(self.utime.value(), self.ticks_per_second)),
                "system_cores" => finite(divide(self.stime.value(), self.ticks_per_second)),
                "run_delay_ms_per_second" => {
                    finite(divide(self.rundelay_ns.value(), Some(1_000_000.0)))
                }
                "context_switches_per_second" => finite(combine(&self.nvcsw, &self.nivcsw)),
                "resident_kib" => finite(self.resident_kib.value()),
                "virtual_kib" => finite(self.virtual_kib.value()),
                "swap_kib" => finite(self.swap_kib.value()),
                "major_faults_per_second" => finite(self.majflt.value()),
                "read_bytes_per_second" => finite(self.read_bytes.value()),
                "write_bytes_per_second" => finite(self.write_bytes.value()),
                "read_calls_per_second" => finite(self.syscr.value()),
                "write_calls_per_second" => finite(self.syscw.value()),
                _ => Value::Null,
            })
            .collect()
    }
}

fn combine(left: &RateSum, right: &RateSum) -> Option<f64> {
    match (left.value(), right.value()) {
        (None, None) => None,
        (left, right) => Some(left.unwrap_or(0.0) + right.unwrap_or(0.0)),
    }
}

fn divide(value: Option<f64>, divisor: Option<f64>) -> Option<f64> {
    let (Some(value), Some(divisor)) = (value, divisor) else {
        return None;
    };
    (divisor > 0.0).then_some(value / divisor)
}

fn finite(value: Option<f64>) -> Value {
    value
        .filter(|value| value.is_finite())
        .and_then(serde_json::Number::from_f64)
        .map_or(Value::Null, Value::Number)
}

fn count_value(value: u64) -> Value {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a host cannot expose 2^53 simultaneous processes"
    )]
    let value = value as f64;
    finite(Some(value))
}

pub(super) fn with_predecessors(
    all: &[DatasetSegment],
    mut selected: Vec<DatasetSegment>,
) -> Vec<DatasetSegment> {
    let first_process = selected
        .iter()
        .filter(|segment| has_section(segment, PROCESS))
        .map(DatasetSegment::id)
        .min();
    let Some(first_process) = first_process else {
        return selected;
    };
    for logical_name in [PROCESS, ACTIVITY] {
        if let Some(previous) = all
            .iter()
            .filter(|segment| segment.id() < first_process && has_section(segment, logical_name))
            .max_by_key(|segment| segment.id())
        {
            selected.push(previous.clone());
        }
    }
    selected.sort_by_key(DatasetSegment::id);
    selected.dedup_by_key(|segment| segment.id());
    selected
}

fn has_section(segment: &DatasetSegment, logical_name: &str) -> bool {
    segment.sections().iter().any(|section| {
        logical_section_name(section.type_id).is_some_and(|name| name == logical_name)
    })
}

pub(super) fn stream(
    dataset: &dyn QueryDataset,
    segments: &[DatasetSegment],
    window: Window,
    request: &HourSeriesRequest,
    sink: &mut dyn QuerySink,
) -> Result<(), QueryError> {
    let fields = selected_fields(request)?;
    let mut opened = Vec::with_capacity(segments.len());
    for segment in segments {
        if sink.cancelled() {
            return Ok(());
        }
        opened.push(dataset.open(segment)?);
    }
    let activities = activity_pids(&opened, sink)?;
    let moments = process_moments(&opened, sink)?;
    let summaries = summaries(&opened, &moments, &activities, sink)?;
    emit_summaries(&summaries, window, &fields, sink)
}

fn selected_fields(request: &HourSeriesRequest) -> Result<Vec<FieldSpec>, QueryError> {
    if let Some(filter) = request.filters.first() {
        return Err(QueryError::BadFilter(filter.column.clone()));
    }
    if request.type_id.is_some() {
        return Err(QueryError::BadFilter("type_id".to_owned()));
    }
    if request.fields.is_empty() {
        return Ok(FIELDS.to_vec());
    }
    request
        .fields
        .iter()
        .map(|name| {
            FIELDS
                .iter()
                .find(|field| field.name == name)
                .copied()
                .ok_or_else(|| QueryError::NoSuchColumn(name.clone()))
        })
        .collect()
}

fn activity_pids(
    segments: &[Segment],
    sink: &dyn QuerySink,
) -> Result<BTreeMap<i64, BTreeSet<i32>>, QueryError> {
    let mut snapshots = BTreeMap::<i64, BTreeSet<i32>>::new();
    for segment in segments {
        for (type_id, _rows) in segment.sections() {
            if logical_section_name(type_id) != Some(ACTIVITY) {
                continue;
            }
            let fields = existing_columns(type_id, &["ts", "pid"]);
            let mut connected = true;
            segment.visit_rows(type_id, &fields, 0, usize::MAX, |_ordinal, row| {
                if sink.cancelled() {
                    connected = false;
                    return false;
                }
                let (Some(ts), Some(pid)) = (timestamp(&row), i32_value(row.get("pid"))) else {
                    return true;
                };
                snapshots.entry(ts).or_default().insert(pid);
                true
            })?;
            if !connected {
                return Ok(snapshots);
            }
        }
    }
    Ok(snapshots)
}

fn process_moments(segments: &[Segment], sink: &dyn QuerySink) -> Result<MomentIndex, QueryError> {
    let mut moments = BTreeMap::<i64, i64>::new();
    let mut last_by_segment = HashMap::new();
    for segment in segments {
        for (type_id, _rows) in segment.sections() {
            if logical_section_name(type_id) != Some(PROCESS) {
                continue;
            }
            let fields = existing_columns(type_id, &["ts"]);
            let mut connected = true;
            segment.visit_rows(type_id, &fields, 0, usize::MAX, |_ordinal, row| {
                if sink.cancelled() {
                    connected = false;
                    return false;
                }
                if let Some(ts) = timestamp(&row) {
                    moments.entry(ts).or_insert_with(|| segment.id());
                    last_by_segment
                        .entry(segment.id())
                        .and_modify(|last: &mut i64| *last = (*last).max(ts))
                        .or_insert(ts);
                }
                true
            })?;
            if !connected {
                return Ok(MomentIndex {
                    previous: previous_moments(moments),
                    last_by_segment,
                });
            }
        }
    }
    Ok(MomentIndex {
        previous: previous_moments(moments),
        last_by_segment,
    })
}

fn previous_moments(moments: BTreeMap<i64, i64>) -> BTreeMap<i64, i64> {
    let mut previous = None;
    moments
        .into_keys()
        .map(|ts| {
            let before = previous.replace(ts).unwrap_or(i64::MIN);
            (ts, before)
        })
        .collect()
}

fn summaries(
    segments: &[Segment],
    moments: &MomentIndex,
    activities: &BTreeMap<i64, BTreeSet<i32>>,
    sink: &dyn QuerySink,
) -> Result<BTreeMap<i64, Summary>, QueryError> {
    let mut out = BTreeMap::<i64, Summary>::new();
    let mut previous = HashMap::<i32, Previous>::new();
    for segment in segments {
        let ticks_per_second = ticks_per_second(segment)?;
        let Some(last_segment_moment) = moments.last_by_segment.get(&segment.id()).copied() else {
            continue;
        };
        let mut state = std::mem::take(&mut previous);
        for (type_id, _rows) in segment.sections() {
            if logical_section_name(type_id) != Some(PROCESS) {
                continue;
            }
            let fields = existing_columns(
                type_id,
                &PROCESS_COLUMNS
                    .iter()
                    .chain(PROCESS_OPTIONAL_COLUMNS.iter())
                    .copied()
                    .collect::<Vec<_>>(),
            );
            let mut connected = true;
            segment.visit_rows(type_id, &fields, 0, usize::MAX, |_ordinal, row| {
                if sink.cancelled() {
                    connected = false;
                    return false;
                }
                let (Some(ts), Some(pid)) = (timestamp(&row), i32_value(row.get("pid"))) else {
                    return true;
                };
                let before = state.get(&pid).copied();
                if before.is_some_and(|stored| stored.ts >= ts) {
                    return true;
                }
                let counters = counters(&row);
                let expected = moments.previous.get(&ts).copied().unwrap_or(i64::MIN);
                let starttime = integer(row.get("starttime"));
                let predecessor = before
                    .as_ref()
                    .filter(|stored| stored.ts == expected && stored.starttime == starttime)
                    .filter(|stored| stored.starttime.is_some())
                    .copied();
                let seconds = predecessor.and_then(|stored| elapsed_seconds(stored.ts, ts));
                let pg_pids = activities.range(..=ts).next_back().map(|(_ts, pids)| pids);
                let summary = out.entry(ts).or_default();
                summary.segment_id = segment.id();
                summary.ticks_per_second = ticks_per_second;
                add_row(
                    summary,
                    &row,
                    &counters,
                    predecessor.as_ref().map(|stored| &stored.counters),
                    seconds,
                );
                if pg_pids.is_some_and(|pids| pids.contains(&pid)) {
                    summary.postgresql = summary.postgresql.saturating_add(1);
                }
                state.insert(
                    pid,
                    Previous {
                        ts,
                        starttime,
                        counters,
                    },
                );
                true
            })?;
            if !connected {
                return Ok(out);
            }
        }
        state.retain(|_identity, stored| stored.ts == last_segment_moment);
        previous = state;
    }
    Ok(out)
}

fn ticks_per_second(segment: &Segment) -> Result<Option<f64>, QueryError> {
    let mut stored = None;
    for (type_id, _rows) in segment.sections() {
        if logical_section_name(type_id) != Some("instance_metadata") {
            continue;
        }
        let fields = existing_columns(type_id, &["clock_ticks_per_sec"]);
        segment.visit_rows(type_id, &fields, 0, usize::MAX, |_ordinal, row| {
            stored = i128_value(row.get("clock_ticks_per_sec")).and_then(|value| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "clock ticks are a small integer"
                )]
                let value = value as f64;
                (value > 0.0).then_some(value)
            });
            false
        })?;
        if stored.is_some() {
            break;
        }
    }
    Ok(stored)
}

fn add_row(
    summary: &mut Summary,
    row: &Row,
    current: &Counters,
    previous: Option<&Counters>,
    seconds: Option<f64>,
) {
    summary.processes = summary.processes.saturating_add(1);
    summary.threads.add(integer(row.get("num_threads")));
    if i128_value(row.get("state")) == Some(i128::from(b'R')) {
        summary.runnable = summary.runnable.saturating_add(1);
    }
    summary.resident_kib.add(integer(row.get("rmem_kb")));
    summary.virtual_kib.add(integer(row.get("vmem_kb")));
    summary.swap_kib.add(integer(row.get("vswap_kb")));
    summary.utime.add(
        current.utime,
        previous.and_then(|value| value.utime),
        seconds,
    );
    summary.stime.add(
        current.stime,
        previous.and_then(|value| value.stime),
        seconds,
    );
    summary.rundelay_ns.add(
        current.rundelay_ns,
        previous.and_then(|value| value.rundelay_ns),
        seconds,
    );
    summary.nvcsw.add(
        current.nvcsw,
        previous.and_then(|value| value.nvcsw),
        seconds,
    );
    summary.nivcsw.add(
        current.nivcsw,
        previous.and_then(|value| value.nivcsw),
        seconds,
    );
    summary.majflt.add(
        current.majflt,
        previous.and_then(|value| value.majflt),
        seconds,
    );
    summary.read_bytes.add(
        current.read_bytes,
        previous.and_then(|value| value.read_bytes),
        seconds,
    );
    summary.write_bytes.add(
        current.write_bytes,
        previous.and_then(|value| value.write_bytes),
        seconds,
    );
    summary.syscr.add(
        current.syscr,
        previous.and_then(|value| value.syscr),
        seconds,
    );
    summary.syscw.add(
        current.syscw,
        previous.and_then(|value| value.syscw),
        seconds,
    );
}

fn counters(row: &Row) -> Counters {
    Counters {
        utime: integer(row.get("utime")),
        stime: integer(row.get("stime")),
        rundelay_ns: integer(row.get("rundelay_ns")),
        nvcsw: integer(row.get("nvcsw")),
        nivcsw: integer(row.get("nivcsw")),
        majflt: integer(row.get("majflt")),
        read_bytes: integer(row.get("read_bytes")),
        write_bytes: integer(row.get("write_bytes")),
        syscr: integer(row.get("syscr")),
        syscw: integer(row.get("syscw")),
    }
}

fn elapsed_seconds(before: i64, current: i64) -> Option<f64> {
    let elapsed = current.checked_sub(before)?;
    if elapsed <= 0 {
        return None;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval of 2^52 microseconds is 142 years"
    )]
    Some(elapsed as f64 / 1_000_000.0)
}

fn existing_columns(type_id: u32, requested: &[&'static str]) -> Vec<&'static str> {
    let Some(contract) = contract(type_id) else {
        return Vec::new();
    };
    requested
        .iter()
        .filter_map(|name| contract.column(name).map(|column| column.name))
        .collect()
}

fn timestamp(row: &Row) -> Option<i64> {
    i64_value(row.get("ts"))
}

const fn i32_value(cell: Option<&Cell>) -> Option<i32> {
    match cell {
        Some(Cell::I32(value)) => Some(*value),
        _ => None,
    }
}

const fn i64_value(cell: Option<&Cell>) -> Option<i64> {
    match cell {
        Some(Cell::I64(value) | Cell::Ts(value)) => Some(*value),
        _ => None,
    }
}

fn i128_value(cell: Option<&Cell>) -> Option<i128> {
    match cell {
        Some(Cell::I16(value)) => Some(i128::from(*value)),
        Some(Cell::I32(value)) => Some(i128::from(*value)),
        Some(Cell::I64(value) | Cell::Ts(value)) => Some(i128::from(*value)),
        Some(Cell::U32(value)) => Some(i128::from(*value)),
        Some(Cell::U64(value)) => Some(i128::from(*value)),
        _ => None,
    }
}

fn integer(cell: Option<&Cell>) -> Option<i128> {
    i128_value(cell)
}

fn emit_summaries(
    summaries: &BTreeMap<i64, Summary>,
    window: Window,
    fields: &[FieldSpec],
    sink: &mut dyn QuerySink,
) -> Result<(), QueryError> {
    let mut segment_id = None;
    let mut ordinal = 0_u64;
    for (ts, summary) in summaries {
        if !inside(*ts, window) {
            continue;
        }
        if sink.cancelled() {
            return Ok(());
        }
        if segment_id != Some(summary.segment_id) {
            segment_id = Some(summary.segment_id);
            ordinal = 0;
            if !sink.record(record(json!({
                "record": "series_segment",
                "segment": { "id": summary.segment_id.to_string() },
            }))?)
                || !sink.record(record(json!({
                    "record": "layout",
                    "layout": layout(fields),
                }))?)
            {
                return Ok(());
            }
        }
        if !sink.record(record(json!({
            "record": "row",
            "type_id": DERIVED_TYPE_ID,
            "ordinal": ordinal.to_string(),
            "timestamp": ts.to_string(),
            "identity": [],
            "values": summary.values(fields),
        }))?) {
            return Ok(());
        }
        ordinal = ordinal.saturating_add(1);
    }
    Ok(())
}

fn inside(ts: i64, window: Window) -> bool {
    window.from.is_none_or(|from| ts >= from) && window.to.is_none_or(|to| ts <= to)
}

fn layout(fields: &[FieldSpec]) -> Value {
    json!({
        "logical_name": SECTION,
        "physical_name": "derived_process_summary",
        "type_id": DERIVED_TYPE_ID,
        "implementation": "kronika",
        "identity": [],
        "columns": fields.iter().map(|field| json!({
            "name": field.name,
            "type": "f64",
            "class": "gauge",
            "unit": field.unit,
            "nullable": field.nullable,
        })).collect::<Vec<_>>(),
        "provenance": { "inputs": ["1100001", "1001001", "1001002", "1001004"] },
    })
}

#[cfg(test)]
mod tests {
    use super::{Counters, ExactSum, RateSum, Summary, combine, elapsed_seconds, previous_moments};
    use std::collections::BTreeMap;

    #[test]
    fn moments_bind_rates_to_the_immediately_previous_complete_snapshot() {
        let moments = previous_moments(BTreeMap::from([(10, 1), (20, 1), (40, 2)]));
        assert_eq!(
            moments,
            BTreeMap::from([(10, i64::MIN), (20, 10), (40, 20)])
        );
    }

    #[test]
    fn exact_sums_keep_null_distinct_from_zero_and_cover_more_than_one_page() {
        let mut sum = ExactSum::default();
        assert_eq!(sum.value(), None);
        for value in 0..1_005 {
            sum.add(Some(value));
        }
        assert_eq!(sum.value(), Some(504_510.0));
    }

    #[test]
    fn rates_keep_zero_and_reject_missing_resets_and_nonpositive_time() {
        let mut rates = RateSum::default();
        rates.add(Some(10), Some(10), Some(5.0));
        rates.add(Some(20), Some(10), Some(5.0));
        rates.add(None, Some(10), Some(5.0));
        rates.add(Some(5), Some(10), Some(5.0));
        rates.add(Some(20), Some(10), elapsed_seconds(10, 10));
        assert_eq!(rates.value(), Some(2.0));
    }

    #[test]
    fn context_switches_match_the_cards_partial_null_rule() {
        let mut left = RateSum::default();
        let right = RateSum::default();
        assert_eq!(combine(&left, &right), None);
        left.add(Some(20), Some(10), Some(5.0));
        assert_eq!(combine(&left, &right), Some(2.0));
    }

    #[test]
    fn all_sixteen_values_have_stable_positions() {
        let mut summary = Summary {
            processes: 2,
            runnable: 1,
            postgresql: 1,
            ticks_per_second: Some(100.0),
            ..Summary::default()
        };
        summary.threads.add(Some(4));
        summary.resident_kib.add(Some(8));
        summary.utime.add(Some(20), Some(10), Some(5.0));
        summary.syscw.add(Some(30), Some(20), Some(5.0));
        let values = summary.values(&super::FIELDS);
        assert_eq!(values.len(), 16);
        assert_eq!(values[0], 2.0);
        assert_eq!(values[1], 4.0);
        assert_eq!(values[2], 1.0);
        assert_eq!(values[3], 1.0);
        assert_eq!(values[4], 0.02);
        assert_eq!(values[8], 8.0);
        assert_eq!(values[15], 2.0);
    }

    #[test]
    fn counter_container_starts_unavailable() {
        let counters = Counters::default();
        assert_eq!(counters.utime, None);
        assert_eq!(counters.read_bytes, None);
    }
}
