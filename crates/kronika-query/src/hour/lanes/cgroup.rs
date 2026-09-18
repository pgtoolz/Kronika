//! Recorded cgroup membership and resource-counter collection.

use std::collections::BTreeMap;

use kronika_reader::{Resolved, Row, Segment};

use super::{
    Counters, Facts, Membership, add, integer, number, string_id, timestamp, with_columns,
};

use crate::QueryError;

pub(super) fn read_cgroup_context(
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

pub(super) fn refresh_identity_boundaries(
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

pub(super) fn membership(row: &Row) -> Membership {
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

pub(super) fn cgroup_cpu_capacity(row: &Row) -> Option<f64> {
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
pub(super) fn member_row(row: &Row, membership: Option<&Membership>, path: Option<u64>) -> bool {
    let (Some(membership), Some(path)) = (membership, path) else {
        return false;
    };
    string_id(row, "cgroup_path") == Some(path) && integer(row, "scope") == membership.scope
}

pub(super) fn read_cgroup_cpu(
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

pub(super) fn read_cgroup_memory(
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

pub(super) fn read_cgroup_io(
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

pub(super) fn record_cgroup_io(counters: &mut Counters, row: &Row) {
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

pub(super) fn read_cgroup_pids(
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
