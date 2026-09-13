use std::collections::{BTreeMap, BTreeSet, HashSet};

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment};
use kronika_registry::{contract, logical_section_name};

use crate::{QueryError, Window};

#[cfg(test)]
mod tests;

pub(super) struct LanePoint {
    pub(super) key: &'static str,
    pub(super) ts: i64,
    pub(super) value: Option<f64>,
}

#[derive(Default)]
struct Counters {
    busy_ticks: BTreeMap<i64, i64>,
    cpu_units: BTreeMap<i64, (i64, i64)>,
    cg_io_devices: BTreeMap<i64, HashSet<(u64, i64, i64)>>,
    stall_cpu: BTreeMap<i64, i64>,
    stall_io: BTreeMap<i64, i64>,
    memory: BTreeMap<i64, f64>,
    disk_busy: BTreeMap<i64, i64>,
    disk_queue: BTreeMap<i64, i64>,
    net_rx: BTreeMap<i64, i64>,
    net_tx: BTreeMap<i64, i64>,
    net_drop: BTreeMap<i64, i64>,
    net_errors: BTreeMap<i64, i64>,
    swap: BTreeMap<i64, Option<i64>>,
    oom: BTreeMap<i64, Option<i64>>,
    running: BTreeMap<i64, f64>,
    waiting: BTreeMap<i64, f64>,
    lock_waiting: BTreeMap<i64, f64>,
    oldest_xact: BTreeMap<i64, f64>,
    // Counters and pressure for the cgroup selected by `os_cgroup_context`.
    cg_cpu_usage: BTreeMap<i64, i64>,
    cg_cpu_throttled: BTreeMap<i64, i64>,
    cg_cpu_capacity: BTreeMap<i64, Option<f64>>,
    cg_cpu_boundaries: BTreeSet<i64>,
    cg_memory_boundaries: BTreeSet<i64>,
    cg_io_boundaries: BTreeSet<i64>,
    cg_stall_cpu: BTreeMap<i64, i64>,
    cg_stall_memory: BTreeMap<i64, i64>,
    cg_stall_io: BTreeMap<i64, i64>,
    cg_memory_bytes: BTreeMap<i64, f64>,
    cg_memory_share: BTreeMap<i64, Option<f64>>,
    cg_oom: BTreeMap<i64, Option<i64>>,
    cg_io_read: BTreeMap<i64, i64>,
    cg_io_write: BTreeMap<i64, i64>,
    cg_pids: BTreeMap<i64, f64>,
    cg_pids_share: BTreeMap<i64, Option<f64>>,
}

impl Counters {
    fn retain_after(&mut self, finalized: i64) {
        retain_after(&mut self.busy_ticks, finalized);
        retain_after(&mut self.cpu_units, finalized);
        retain_after(&mut self.cg_io_devices, finalized);
        retain_after(&mut self.stall_cpu, finalized);
        retain_after(&mut self.stall_io, finalized);
        retain_after(&mut self.memory, finalized);
        retain_after(&mut self.disk_busy, finalized);
        retain_after(&mut self.disk_queue, finalized);
        retain_after(&mut self.net_rx, finalized);
        retain_after(&mut self.net_tx, finalized);
        retain_after(&mut self.net_drop, finalized);
        retain_after(&mut self.net_errors, finalized);
        retain_after(&mut self.swap, finalized);
        retain_after(&mut self.oom, finalized);
        retain_after(&mut self.running, finalized);
        retain_after(&mut self.waiting, finalized);
        retain_after(&mut self.lock_waiting, finalized);
        retain_after(&mut self.oldest_xact, finalized);
        retain_after(&mut self.cg_cpu_usage, finalized);
        retain_after(&mut self.cg_cpu_throttled, finalized);
        retain_after(&mut self.cg_cpu_capacity, finalized);
        for boundaries in [
            &mut self.cg_cpu_boundaries,
            &mut self.cg_memory_boundaries,
            &mut self.cg_io_boundaries,
        ] {
            if let Some(last) = boundaries.range(..=finalized).next_back().copied() {
                boundaries.retain(|ts| *ts >= last);
            }
        }
        retain_after(&mut self.cg_stall_cpu, finalized);
        retain_after(&mut self.cg_stall_memory, finalized);
        retain_after(&mut self.cg_stall_io, finalized);
        retain_after(&mut self.cg_memory_bytes, finalized);
        retain_after(&mut self.cg_memory_share, finalized);
        retain_after(&mut self.cg_oom, finalized);
        retain_after(&mut self.cg_io_read, finalized);
        retain_after(&mut self.cg_io_write, finalized);
        retain_after(&mut self.cg_pids, finalized);
        retain_after(&mut self.cg_pids_share, finalized);
    }
}

fn retain_after<T>(samples: &mut BTreeMap<i64, T>, finalized: i64) {
    if let Some((&previous, _)) = samples.range(..=finalized).next_back() {
        *samples = samples.split_off(&previous);
    }
}

/// Carries counter state without re-emitting the shared boundary row.
pub(super) struct State {
    counters: Counters,
    emitted_before: i64,
    cgroup_identity: BTreeMap<i64, [Option<u64>; 4]>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            counters: Counters::default(),
            emitted_before: i64::MIN,
            cgroup_identity: BTreeMap::new(),
        }
    }
}

/// What one segment says about itself next to its lanes.
pub(super) struct SegmentFacts {
    pub(super) postgresql_interval_seconds: Option<u64>,
    /// `instance_metadata.environment`: 0 machine, 1 container.
    pub(super) environment: Option<u32>,
    pub(super) os_enabled: Option<bool>,
    pub(super) postgresql_processes_shared: bool,
}

pub(super) fn collect(
    segment: &Segment,
    window: Window,
    state: &mut State,
    next_min_ts: Option<i64>,
) -> Result<(Vec<LanePoint>, SegmentFacts), QueryError> {
    let mut facts = Facts::default();
    let collection = kronika_index::collection_facts(segment)?;
    // Read clock facts and cgroup context first: resource rows need those paths and scope.
    for (type_id, _rows) in segment.sections() {
        match logical_section_name(type_id) {
            Some("instance_metadata") => read_metadata(segment, type_id, &mut facts)?,
            Some("os_cgroup_context") => {
                read_cgroup_context(
                    segment,
                    type_id,
                    &mut facts,
                    &mut state.counters,
                    &mut state.cgroup_identity,
                )?;
            }
            _other => {}
        }
    }
    refresh_identity_boundaries(
        &mut state.counters,
        &state.cgroup_identity,
        state.emitted_before,
    );
    for (type_id, _rows) in segment.sections() {
        let Some(name) = logical_section_name(type_id) else {
            continue;
        };
        match name {
            "os_cpu" => read_cpu(segment, type_id, &mut state.counters, &mut facts)?,
            "os_psi" => read_psi(segment, type_id, &mut state.counters)?,
            "os_meminfo" => read_memory(segment, type_id, &mut state.counters)?,
            "os_diskstats" => read_disk(segment, type_id, &mut state.counters)?,
            "os_netdev" => read_network(segment, type_id, &mut state.counters)?,
            "os_vmstat" => read_vmstat(segment, type_id, &mut state.counters)?,
            "os_cgroup_cpu" => read_cgroup_cpu(segment, type_id, &facts, &mut state.counters)?,
            "os_cgroup_memory" => {
                read_cgroup_memory(segment, type_id, &facts, &mut state.counters)?;
            }
            "os_cgroup_io" => read_cgroup_io(segment, type_id, &facts, &mut state.counters)?,
            "os_cgroup_pids" => {
                read_cgroup_pids(segment, type_id, &facts, &mut state.counters)?;
            }
            "pg_stat_activity" => read_activity(segment, type_id, &mut state.counters)?,
            _other => {}
        }
    }
    let cores = i64::try_from(facts.cores.len()).unwrap_or(0);
    for ts in &facts.cpu_samples {
        state
            .counters
            .cpu_units
            .insert(*ts, (facts.ticks_per_second, cores));
    }
    // Later segments can supply more rows at their minimum timestamp.
    let finalized = next_min_ts.map_or(i64::MAX, |ts| ts.saturating_sub(1));
    let current = current_points(
        &state.counters,
        facts.ticks_per_second,
        cores,
        state.emitted_before.saturating_add(1),
        finalized,
        window,
    );
    state.counters.retain_after(finalized);
    retain_after(&mut state.cgroup_identity, finalized);
    state.emitted_before = state.emitted_before.max(finalized);
    Ok((
        current,
        SegmentFacts {
            postgresql_interval_seconds: facts.postgresql_interval_seconds,
            environment: facts.environment,
            os_enabled: collection.os_enabled,
            postgresql_processes_shared: collection.postgresql_processes_shared,
        },
    ))
}

#[derive(Default)]
struct Facts {
    ticks_per_second: i64,
    cores: BTreeSet<i64>,
    cpu_samples: BTreeSet<i64>,
    postgresql_interval_seconds: Option<u64>,
    environment: Option<u32>,
    memberships: BTreeMap<i64, Membership>,
    memory_limits: BTreeMap<i64, Option<f64>>,
}

/// Controller paths and scope recorded by `os_cgroup_context`.
/// Paths are dictionary ids of the segment, so rows compare without text.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Membership {
    scope: Option<i64>,
    cpu: Option<u64>,
    memory: Option<u64>,
    io: Option<u64>,
    /// Cgroup v2 has one unified membership, which also identifies the TID row.
    pids: Option<u64>,
}

fn read_cgroup_context(
    segment: &Segment,
    type_id: u32,
    facts: &mut Facts,
    counters: &mut Counters,
    recorded_identities: &mut BTreeMap<i64, [Option<u64>; 4]>,
) -> Result<(), QueryError> {
    const FIELDS: [&str; 9] = [
        "ts",
        "cgroup_version",
        "cpu_path",
        "memory_path",
        "io_path",
        "cpuset_cpus",
        "effective_cpu_quota_usec",
        "effective_cpu_period_usec",
        "effective_memory_max",
    ];
    let names = with_columns(
        type_id,
        &FIELDS,
        &[
            "scope",
            "pids_path",
            "cpu_identity",
            "memory_identity",
            "io_identity",
            "pids_identity",
        ],
    );
    let mut identities = BTreeMap::<i64, [Option<u64>; 4]>::new();
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        facts.memberships.insert(ts, membership(&row));
        identities.insert(
            ts,
            [
                ("cpu_identity", "cpu_path"),
                ("memory_identity", "memory_path"),
                ("io_identity", "io_path"),
                ("pids_identity", "pids_path"),
            ]
            .map(|(identity, path)| string_id(&row, identity).or_else(|| string_id(&row, path))),
        );
        counters
            .cg_cpu_capacity
            .insert(ts, cgroup_cpu_capacity(&row));
        facts.memory_limits.insert(
            ts,
            number(&row, "effective_memory_max").filter(|limit| *limit > 0.0),
        );
        true
    })?;
    let ids = identities.values().flatten().flatten().copied().collect();
    let dictionary = segment.dictionary_for(&ids)?;
    for (ts, ids) in identities {
        let identity = ids.map(|id| id.filter(|id| matches!(dictionary.resolve(*id), Some(Resolved::Str(bytes)) if !bytes.is_empty())));
        recorded_identities.insert(ts, identity);
    }
    Ok(())
}

fn refresh_identity_boundaries(
    counters: &mut Counters,
    identities: &BTreeMap<i64, [Option<u64>; 4]>,
    finalized: i64,
) {
    for (index, boundaries) in [
        &mut counters.cg_cpu_boundaries,
        &mut counters.cg_memory_boundaries,
        &mut counters.cg_io_boundaries,
    ]
    .into_iter()
    .enumerate()
    {
        boundaries.retain(|ts| *ts <= finalized);
        let mut previous = None;
        for (&ts, identity) in identities {
            if previous.is_some_and(|before| before != identity[index]) {
                boundaries.insert(ts);
            }
            previous = Some(identity[index]);
        }
    }
}

fn membership(row: &Row) -> Membership {
    let cpu = string_id(row, "cpu_path");
    let memory = string_id(row, "memory_path");
    let io = string_id(row, "io_path");
    let pids = string_id(row, "pids_path").or_else(|| {
        (integer(row, "cgroup_version") == Some(2))
            .then_some(cpu.or(memory).or(io))
            .flatten()
    });
    Membership {
        scope: integer(row, "scope"),
        cpu,
        memory,
        io,
        pids,
    }
}

fn cgroup_cpu_capacity(row: &Row) -> Option<f64> {
    let resolve = if row.contract().type_id.get() == 1_205_002 {
        kronika_index::observed_cgroup_cpu_capacity
    } else {
        kronika_index::cgroup_cpu_capacity
    };
    resolve(
        integer(row, "cpuset_cpus"),
        integer(row, "effective_cpu_quota_usec"),
        integer(row, "effective_cpu_period_usec"),
    )
}

/// Whether a row belongs to the selected group for this controller.
fn member_row(row: &Row, membership: Option<&Membership>, path: Option<u64>) -> bool {
    let (Some(membership), Some(path)) = (membership, path) else {
        return false;
    };
    string_id(row, "cgroup_path") == Some(path) && integer(row, "scope") == membership.scope
}

fn read_cgroup_cpu(
    segment: &Segment,
    type_id: u32,
    facts: &Facts,
    counters: &mut Counters,
) -> Result<(), QueryError> {
    let names = with_columns(
        type_id,
        &["ts", "cgroup_path", "usage_usec", "throttled_usec"],
        &["scope"],
    );
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let membership = timestamp(&row, "ts").and_then(|ts| {
            facts
                .memberships
                .range(..=ts)
                .next_back()
                .map(|(_, value)| value)
        });
        let path = membership.and_then(|value| value.cpu);
        if !member_row(&row, membership, path) {
            return true;
        }
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        if let Some(usage) = integer(&row, "usage_usec") {
            counters.cg_cpu_usage.insert(ts, usage);
        }
        if let Some(throttled) = integer(&row, "throttled_usec") {
            counters.cg_cpu_throttled.insert(ts, throttled);
        }
        true
    })?;
    Ok(())
}

fn read_cgroup_memory(
    segment: &Segment,
    type_id: u32,
    facts: &Facts,
    counters: &mut Counters,
) -> Result<(), QueryError> {
    let names = with_columns(
        type_id,
        &["ts", "cgroup_path", "current", "oom_kill"],
        &["scope"],
    );
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let membership = timestamp(&row, "ts").and_then(|ts| {
            facts
                .memberships
                .range(..=ts)
                .next_back()
                .map(|(_, value)| value)
        });
        let path = membership.and_then(|value| value.memory);
        if !member_row(&row, membership, path) {
            return true;
        }
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        if let Some(current) = number(&row, "current") {
            counters.cg_memory_bytes.insert(ts, current);
            let limit = facts
                .memory_limits
                .range(..=ts)
                .next_back()
                .and_then(|(_, limit)| *limit);
            counters
                .cg_memory_share
                .insert(ts, limit.map(|limit| current / limit * 100.0));
        }
        counters.cg_oom.insert(ts, integer(&row, "oom_kill"));
        true
    })?;
    Ok(())
}

fn read_cgroup_io(
    segment: &Segment,
    type_id: u32,
    facts: &Facts,
    counters: &mut Counters,
) -> Result<(), QueryError> {
    let names = with_columns(
        type_id,
        &["ts", "cgroup_path", "major", "minor", "rbytes", "wbytes"],
        &["scope", "cgroup_identity"],
    );
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let membership = timestamp(&row, "ts").and_then(|ts| {
            facts
                .memberships
                .range(..=ts)
                .next_back()
                .map(|(_, value)| value)
        });
        let path = membership.and_then(|value| value.io);
        if !member_row(&row, membership, path) {
            return true;
        }
        record_cgroup_io(counters, &row);
        true
    })?;
    Ok(())
}

fn record_cgroup_io(counters: &mut Counters, row: &Row) {
    let Some(ts) = timestamp(row, "ts") else {
        return;
    };
    let Some(device) = integer(row, "major").zip(integer(row, "minor")) else {
        return;
    };
    let Some(identity) =
        string_id(row, "cgroup_identity").or_else(|| string_id(row, "cgroup_path"))
    else {
        return;
    };
    if !counters
        .cg_io_devices
        .entry(ts)
        .or_default()
        .insert((identity, device.0, device.1))
    {
        return;
    }
    // One cgroup has a row per device; the lane is the sum over devices.
    if let Some(read) = number(row, "rbytes") {
        add(&mut counters.cg_io_read, ts, read);
    }
    if let Some(written) = number(row, "wbytes") {
        add(&mut counters.cg_io_write, ts, written);
    }
}

fn read_cgroup_pids(
    segment: &Segment,
    type_id: u32,
    facts: &Facts,
    counters: &mut Counters,
) -> Result<(), QueryError> {
    let names = with_columns(
        type_id,
        &["ts", "cgroup_path", "current", "max"],
        &["scope"],
    );
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let membership = timestamp(&row, "ts").and_then(|ts| {
            facts
                .memberships
                .range(..=ts)
                .next_back()
                .map(|(_, value)| value)
        });
        let path = membership.and_then(|value| value.pids);
        if !member_row(&row, membership, path) {
            return true;
        }
        let (Some(ts), Some(current)) = (timestamp(&row, "ts"), number(&row, "current")) else {
            return true;
        };
        counters.cg_pids.insert(ts, current);
        let max = number(&row, "max").filter(|max| *max > 0.0);
        counters
            .cg_pids_share
            .insert(ts, max.map(|max| current / max * 100.0));
        true
    })?;
    Ok(())
}

fn read_metadata(segment: &Segment, type_id: u32, facts: &mut Facts) -> Result<(), QueryError> {
    let names = with_columns(
        type_id,
        &["clock_ticks_per_sec"],
        &["postgresql_interval_seconds", "environment"],
    );
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        if let Some(ticks) = number(&row, "clock_ticks_per_sec") {
            #[expect(clippy::cast_possible_truncation, reason = "a hundred, in practice")]
            {
                facts.ticks_per_second = ticks as i64;
            }
        }
        if let Some(Cell::U64(seconds)) = row.get("postgresql_interval_seconds") {
            facts.postgresql_interval_seconds = Some(*seconds);
        }
        if let Some(Cell::U32(environment)) = row.get("environment") {
            facts.environment = Some(*environment);
        }
        true
    })?;
    Ok(())
}

fn read_cpu(
    segment: &Segment,
    type_id: u32,
    counters: &mut Counters,
    facts: &mut Facts,
) -> Result<(), QueryError> {
    const FIELDS: [&str; 8] = [
        "ts", "cpu_id", "user", "nice", "system", "irq", "softirq", "steal",
    ];
    let names = with_columns(type_id, &FIELDS, &["scope"]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        if let Some(id) = number(&row, "cpu_id")
            && id >= 0.0
        {
            #[expect(clippy::cast_possible_truncation, reason = "core indexes are small")]
            facts.cores.insert(id as i64);
        }
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        if !matches!(row.get("cpu_id"), Some(Cell::I32(-1)))
            || !matches!(row.get("scope"), None | Some(Cell::U32(0)))
        {
            return true;
        }
        if let Some(busy) = cpu_busy_ticks(&row) {
            facts.cpu_samples.insert(ts);
            counters.busy_ticks.insert(ts, busy);
        }
        true
    })?;
    Ok(())
}

fn cpu_busy_ticks(row: &Row) -> Option<i64> {
    ["user", "nice", "system", "irq", "softirq", "steal"]
        .iter()
        .try_fold(0_i64, |total, name| {
            let Cell::I64(value) = row.get(name)? else {
                return None;
            };
            total.checked_add(*value)
        })
}

fn read_psi(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), QueryError> {
    let names = with_columns(type_id, &["ts", "resource", "some_total"], &["scope"]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let (Some(ts), Some(resource), Some(total)) = (
            timestamp(&row, "ts"),
            integer(&row, "resource"),
            integer(&row, "some_total"),
        ) else {
            return true;
        };
        let scope = integer(&row, "scope").unwrap_or(0);
        if let Some(stalls) = pressure_lane(counters, scope, resource) {
            stalls.insert(ts, total);
        }
        true
    })?;
    Ok(())
}

/// Host lanes use scope 0; container lanes use selected cgroup scope 3 (legacy)
/// or 4. Resources: 0 CPU, 1 memory, 2 I/O; host memory pressure has no lane.
const fn pressure_lane(
    counters: &mut Counters,
    scope: i64,
    resource: i64,
) -> Option<&mut BTreeMap<i64, i64>> {
    match (scope, resource) {
        (0, 0) => Some(&mut counters.stall_cpu),
        (0, 2) => Some(&mut counters.stall_io),
        (3 | 4, 0) => Some(&mut counters.cg_stall_cpu),
        (3 | 4, 1) => Some(&mut counters.cg_stall_memory),
        (3 | 4, 2) => Some(&mut counters.cg_stall_io),
        _other => None,
    }
}

fn read_memory(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), QueryError> {
    let names = with_columns(type_id, &["ts", "mem_total", "mem_available"], &[]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let (Some(ts), Some(total), Some(available)) = (
            timestamp(&row, "ts"),
            number(&row, "mem_total"),
            number(&row, "mem_available"),
        ) else {
            return true;
        };
        if total > 0.0 {
            counters
                .memory
                .insert(ts, (total - available) / total * 100.0);
        }
        true
    })?;
    Ok(())
}

fn read_disk(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), QueryError> {
    let names = with_columns(
        type_id,
        &["ts", "io_time_ms", "io_weighted_time_ms"],
        &["scope"],
    );
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let (Some(ts), Some(busy), Some(weighted)) = (
            timestamp(&row, "ts"),
            number(&row, "io_time_ms"),
            number(&row, "io_weighted_time_ms"),
        ) else {
            return true;
        };
        add(&mut counters.disk_busy, ts, busy);
        add(&mut counters.disk_queue, ts, weighted);
        true
    })?;
    Ok(())
}

fn read_network(
    segment: &Segment,
    type_id: u32,
    counters: &mut Counters,
) -> Result<(), QueryError> {
    const FIELDS: [&str; 9] = [
        "ts", "rx_bytes", "tx_bytes", "rx_drop", "tx_drop", "rx_errs", "tx_errs", "rx_fifo",
        "tx_fifo",
    ];
    let names = with_columns(type_id, &FIELDS, &[]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        for (store, columns) in [
            (&mut counters.net_rx, ["rx_bytes"].as_slice()),
            (&mut counters.net_tx, ["tx_bytes"].as_slice()),
            (
                &mut counters.net_drop,
                ["rx_drop", "tx_drop", "rx_fifo", "tx_fifo"].as_slice(),
            ),
            (&mut counters.net_errors, ["rx_errs", "tx_errs"].as_slice()),
        ] {
            let total: f64 = columns.iter().filter_map(|name| number(&row, name)).sum();
            add(store, ts, total);
        }
        true
    })?;
    Ok(())
}

fn read_vmstat(segment: &Segment, type_id: u32, counters: &mut Counters) -> Result<(), QueryError> {
    let names = with_columns(type_id, &["ts"], &["pswpin", "pswpout", "oom_kill"]);
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let Some(ts) = timestamp(&row, "ts") else {
            return true;
        };
        counters
            .swap
            .insert(ts, counter_sum(&row, &["pswpin", "pswpout"]));
        counters.oom.insert(ts, counter_sum(&row, &["oom_kill"]));
        true
    })?;
    Ok(())
}

fn counter_sum(row: &Row, columns: &[&str]) -> Option<i64> {
    columns.iter().try_fold(0_i64, |total, column| {
        let Cell::I64(value) = row.get(column)? else {
            return None;
        };
        total.checked_add(*value)
    })
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "kernel counters stay below 2^63"
)]
fn add(store: &mut BTreeMap<i64, i64>, ts: i64, value: f64) {
    let value = value as i64;
    store
        .entry(ts)
        .and_modify(|total| *total += value)
        .or_insert(value);
}

#[derive(Debug, PartialEq, Eq)]
struct ActivitySample {
    ts: Option<i64>,
    backend_type: Option<u64>,
    state: Option<u64>,
    wait_event_type: Option<u64>,
    leader: bool,
    xact_start: Option<i64>,
}

fn activity_sample(row: &Row) -> ActivitySample {
    ActivitySample {
        ts: timestamp(row, "ts"),
        backend_type: string_id(row, "backend_type"),
        state: string_id(row, "state"),
        wait_event_type: string_id(row, "wait_event_type"),
        leader: row.get("leader_pid").is_some_and(present),
        xact_start: timestamp(row, "xact_start"),
    }
}

fn read_activity(
    segment: &Segment,
    type_id: u32,
    counters: &mut Counters,
) -> Result<(), QueryError> {
    let names = with_columns(
        type_id,
        &[
            "ts",
            "state",
            "wait_event_type",
            "backend_type",
            "xact_start",
        ],
        &["leader_pid"],
    );
    let mut samples = Vec::new();
    let mut ids = HashSet::new();
    segment.visit_rows(type_id, &names, 0, usize::MAX, |_ordinal, row| {
        let sample = activity_sample(&row);
        ids.extend(
            [sample.backend_type, sample.state, sample.wait_event_type]
                .into_iter()
                .flatten(),
        );
        samples.push(sample);
        true
    })?;
    let dictionary = segment.dictionary_for(&ids)?;
    for sample in samples {
        record_activity_sample(
            counters,
            &sample,
            text(sample.backend_type, &dictionary),
            text(sample.state, &dictionary),
            text(sample.wait_event_type, &dictionary),
        );
    }
    Ok(())
}

fn record_activity_sample(
    counters: &mut Counters,
    sample: &ActivitySample,
    kind: Option<&[u8]>,
    state: Option<&[u8]>,
    wait_event_type: Option<&[u8]>,
) {
    let Some(ts) = sample.ts else {
        return;
    };
    counters.running.entry(ts).or_insert(0.0);
    counters.waiting.entry(ts).or_insert(0.0);
    counters.lock_waiting.entry(ts).or_insert(0.0);
    // Lock-wait expiry counts all backends, unlike the client-only public lane.
    if wait_event_type == Some(b"Lock".as_slice()) {
        *counters.lock_waiting.entry(ts).or_insert(0.0) += 1.0;
    }
    // Count client backends, not their parallel workers, in public activity.
    if kind != Some(b"client backend".as_slice()) || sample.leader {
        return;
    }
    if let Some(started) = sample.xact_start {
        #[expect(clippy::cast_precision_loss, reason = "an hour is far below 2^53")]
        let age = (ts - started) as f64 / 1_000_000.0;
        counters
            .oldest_xact
            .entry(ts)
            .and_modify(|current| {
                if age > *current {
                    *current = age;
                }
            })
            .or_insert_with(|| age.max(0.0));
    }
    if state != Some(b"active".as_slice()) {
        return;
    }
    let lane = if wait_event_type.is_some() {
        &mut counters.waiting
    } else {
        &mut counters.running
    };
    *lane.entry(ts).or_insert(0.0) += 1.0;
}

fn current_points(
    counters: &Counters,
    ticks_per_second: i64,
    cpu_count: i64,
    from: i64,
    to: i64,
    window: Window,
) -> Vec<LanePoint> {
    points(counters, ticks_per_second, cpu_count)
        .into_iter()
        .filter(|point| point.ts >= from && point.ts <= to && window.contains(point.ts))
        .collect()
}

fn points(counters: &Counters, ticks_per_second: i64, cpu_count: i64) -> Vec<LanePoint> {
    let mut out = Vec::new();
    for (ts, value) in rate(&counters.busy_ticks, |value, seconds| value / seconds) {
        let (ticks, cores) = counters
            .cpu_units
            .get(&ts)
            .copied()
            .unwrap_or((ticks_per_second, cpu_count));
        if ticks > 0 && cores > 0 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "core counts and clock rates are small"
            )]
            let capacity = (ticks * cores) as f64;
            out.push(LanePoint {
                key: "cpu_busy",
                ts,
                value: value.map(|value| value / capacity * 100.0),
            });
        }
    }
    for (key, stalls) in [
        ("cpu_stall", &counters.stall_cpu),
        ("io_stall", &counters.stall_io),
    ] {
        let stalled = rate(stalls, |value, seconds| {
            value / 1_000_000.0 / seconds * 100.0
        });
        for (ts, value) in stalled {
            out.push(LanePoint { key, ts, value });
        }
    }
    // io_time_ms per elapsed second is device busy percent.
    for (ts, value) in rate(&counters.disk_busy, |value, seconds| {
        (value / 1000.0 / seconds * 100.0).min(100.0)
    }) {
        out.push(LanePoint {
            key: "disk_busy",
            ts,
            value,
        });
    }
    for (key, stored, scale) in [
        ("disk_queue", &counters.disk_queue, 1000.0),
        ("net_rx", &counters.net_rx, 1.0),
        ("net_tx", &counters.net_tx, 1.0),
        ("net_drop", &counters.net_drop, 1.0),
        ("net_errors", &counters.net_errors, 1.0),
    ] {
        for (ts, value) in rate(stored, |value, seconds| value / scale / seconds) {
            out.push(LanePoint { key, ts, value });
        }
    }
    for (key, stored) in [("mem_swap", &counters.swap), ("mem_oom", &counters.oom)] {
        for (ts, value) in nullable_rate(stored, |value, seconds| value / seconds) {
            out.push(LanePoint { key, ts, value });
        }
    }
    for (key, stored) in [
        ("memory", &counters.memory),
        ("pg_running", &counters.running),
        ("pg_waiting", &counters.waiting),
        ("pg_lock_waiting", &counters.lock_waiting),
        ("pg_oldest_xact", &counters.oldest_xact),
    ] {
        for (ts, value) in stored {
            out.push(LanePoint {
                key,
                ts: *ts,
                value: Some(*value),
            });
        }
    }
    container_points(counters, &mut out);
    out.sort_by_key(|point| (point.ts, point.key));
    out
}

/// Resource lanes for the selected cgroup, using its recorded capacity.
fn container_points(counters: &Counters, out: &mut Vec<LanePoint>) {
    for (ts, cores) in group_rate(
        &counters.cg_cpu_usage,
        &counters.cg_cpu_boundaries,
        |value, seconds| value / 1_000_000.0 / seconds,
    ) {
        out.push(LanePoint {
            key: "cg_cpu_cores",
            ts,
            value: cores,
        });
        // A share needs a recorded capacity; host cores never substitute.
        if !counters.cg_cpu_capacity.is_empty() {
            let share = cores.and_then(|cores| {
                counters
                    .cg_cpu_capacity
                    .range(..=ts)
                    .next_back()
                    .and_then(|(_, capacity)| *capacity)
                    .map(|capacity| cores / capacity * 100.0)
            });
            out.push(LanePoint {
                key: "cg_cpu_share",
                ts,
                value: share,
            });
        }
    }
    for (key, stalls, boundaries) in [
        (
            "cg_cpu_throttle",
            &counters.cg_cpu_throttled,
            &counters.cg_cpu_boundaries,
        ),
        (
            "cg_cpu_psi",
            &counters.cg_stall_cpu,
            &counters.cg_cpu_boundaries,
        ),
        (
            "cg_mem_psi",
            &counters.cg_stall_memory,
            &counters.cg_memory_boundaries,
        ),
        (
            "cg_io_psi",
            &counters.cg_stall_io,
            &counters.cg_io_boundaries,
        ),
    ] {
        for (ts, value) in group_rate(stalls, boundaries, |value, seconds| {
            value / 1_000_000.0 / seconds * 100.0
        }) {
            out.push(LanePoint { key, ts, value });
        }
    }
    for (key, stored) in [
        ("cg_io_read", &counters.cg_io_read),
        ("cg_io_write", &counters.cg_io_write),
    ] {
        for (ts, value) in group_rate(stored, &counters.cg_io_boundaries, |value, seconds| {
            value / seconds
        }) {
            out.push(LanePoint { key, ts, value });
        }
    }
    for (ts, value) in rate_samples(
        counters.cg_oom.iter().map(|(ts, value)| (*ts, *value)),
        counters.cg_oom.len(),
        |value, seconds| value / seconds,
        Some(&counters.cg_memory_boundaries),
    ) {
        out.push(LanePoint {
            key: "cg_oom",
            ts,
            value,
        });
    }
    for (key, stored) in [
        ("cg_memory", &counters.cg_memory_share),
        ("cg_pids_share", &counters.cg_pids_share),
    ] {
        for (ts, value) in stored {
            out.push(LanePoint {
                key,
                ts: *ts,
                value: *value,
            });
        }
    }
    for (key, stored) in [
        ("cg_memory_bytes", &counters.cg_memory_bytes),
        ("cg_pids", &counters.cg_pids),
    ] {
        for (ts, value) in stored {
            out.push(LanePoint {
                key,
                ts: *ts,
                value: Some(*value),
            });
        }
    }
}

fn group_rate(
    stored: &BTreeMap<i64, i64>,
    boundaries: &BTreeSet<i64>,
    scale: impl Fn(f64, f64) -> f64,
) -> Vec<(i64, Option<f64>)> {
    rate_samples(
        stored.iter().map(|(ts, value)| (*ts, Some(*value))),
        stored.len(),
        scale,
        Some(boundaries),
    )
}

/// Returns per-second deltas; the first or unusable sample is null.
fn rate(stored: &BTreeMap<i64, i64>, scale: impl Fn(f64, f64) -> f64) -> Vec<(i64, Option<f64>)> {
    rate_samples(
        stored.iter().map(|(ts, value)| (*ts, Some(*value))),
        stored.len(),
        scale,
        None,
    )
}

/// Returns per-second deltas and starts again after an explicit null sample.
fn nullable_rate(
    stored: &BTreeMap<i64, Option<i64>>,
    scale: impl Fn(f64, f64) -> f64,
) -> Vec<(i64, Option<f64>)> {
    rate_samples(
        stored.iter().map(|(ts, value)| (*ts, *value)),
        stored.len(),
        scale,
        None,
    )
}

fn rate_samples(
    samples: impl Iterator<Item = (i64, Option<i64>)>,
    len: usize,
    scale: impl Fn(f64, f64) -> f64,
    boundaries: Option<&BTreeSet<i64>>,
) -> Vec<(i64, Option<f64>)> {
    let mut out = Vec::with_capacity(len);
    let mut earlier: Option<(i64, i64)> = None;
    for (ts, value) in samples {
        let Some(value) = value else {
            out.push((ts, None));
            earlier = None;
            continue;
        };
        let point = if let Some((before_ts, before)) = earlier {
            #[expect(clippy::cast_precision_loss, reason = "an hour is far below 2^53")]
            let seconds = (ts - before_ts) as f64 / 1_000_000.0;
            #[expect(clippy::cast_precision_loss, reason = "counters stay below 2^53")]
            let delta = (value - before) as f64;
            (seconds > 0.0
                && delta >= 0.0
                && !boundaries.is_some_and(|points| {
                    points
                        .range((
                            std::ops::Bound::Excluded(before_ts),
                            std::ops::Bound::Included(ts),
                        ))
                        .next()
                        .is_some()
                }))
            .then(|| scale(delta, seconds))
        } else {
            None
        };
        out.push((ts, point));
        earlier = Some((ts, value));
    }
    out
}

fn with_columns(
    type_id: u32,
    required: &[&'static str],
    optional: &[&'static str],
) -> Vec<&'static str> {
    let Some(contract) = contract(type_id) else {
        return Vec::new();
    };
    let mut names: Vec<&'static str> = required
        .iter()
        .filter_map(|name| contract.column(name).map(|column| column.name))
        .collect();
    names.extend(
        optional
            .iter()
            .filter_map(|name| contract.column(name).map(|column| column.name)),
    );
    names
}

fn timestamp(row: &Row, column: &str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::Ts(stored)) => Some(*stored),
        _other => None,
    }
}

#[expect(clippy::cast_precision_loss, reason = "counters stay below 2^53")]
fn number(row: &Row, column: &str) -> Option<f64> {
    match row.get(column) {
        Some(Cell::I16(value)) => Some(f64::from(*value)),
        Some(Cell::I32(value)) => Some(f64::from(*value)),
        Some(Cell::I64(value) | Cell::Ts(value)) => Some(*value as f64),
        Some(Cell::U32(value)) => Some(f64::from(*value)),
        Some(Cell::U64(value)) => Some(*value as f64),
        Some(Cell::F64(value)) => Some(*value),
        _other => None,
    }
}

fn integer(row: &Row, column: &str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::I16(value)) => Some(i64::from(*value)),
        Some(Cell::I32(value)) => Some(i64::from(*value)),
        Some(Cell::I64(value) | Cell::Ts(value)) => Some(*value),
        Some(Cell::U32(value)) => Some(i64::from(*value)),
        Some(Cell::U64(value)) => i64::try_from(*value).ok(),
        _other => None,
    }
}

const fn present(cell: &Cell) -> bool {
    !matches!(cell, Cell::Null)
}

fn string_id(row: &Row, column: &str) -> Option<u64> {
    match row.get(column) {
        Some(Cell::StrId(id)) => Some(*id),
        _other => None,
    }
}

fn text(id: Option<u64>, dictionary: &Dictionary) -> Option<&[u8]> {
    match dictionary.resolve(id?) {
        Some(Resolved::Str(bytes)) => Some(bytes),
        _other => None,
    }
}
