//! Lane values and rates with recorded continuity boundaries.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use super::{Counters, DiskIdentity, DiskSnapshot, LanePoint, LockGraph};

use crate::Window;

pub(super) fn current_points(
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

pub(super) fn points(counters: &Counters, ticks_per_second: i64, cpu_count: i64) -> Vec<LanePoint> {
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
            out.push(lane_point(
                "cpu_busy",
                ts,
                value.map(|value| value / capacity * 100.0),
            ));
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
            out.push(lane_point(key, ts, value));
        }
    }
    disk_points(counters, &mut out);
    for (ts, graph) in &counters.lock_graphs {
        let locks = LockGraph {
            waiting: graph.waiting.len(),
            blockers: graph.blockers.len(),
            prepared: graph.prepared,
        };
        out.push(LanePoint {
            key: "pg_lock_graph",
            ts: *ts,
            value: None,
            device: None,
            locks: Some(locks),
        });
    }
    for (key, stored, scale) in [
        ("net_rx", &counters.net_rx, 1.0),
        ("net_tx", &counters.net_tx, 1.0),
        ("net_drop", &counters.net_drop, 1.0),
        ("net_errors", &counters.net_errors, 1.0),
    ] {
        for (ts, value) in rate(stored, |value, seconds| value / scale / seconds) {
            out.push(lane_point(key, ts, value));
        }
    }
    for (key, stored) in [("mem_swap", &counters.swap), ("mem_oom", &counters.oom)] {
        for (ts, value) in nullable_rate(stored, |value, seconds| value / seconds) {
            out.push(lane_point(key, ts, value));
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
            out.push(lane_point(key, *ts, Some(*value)));
        }
    }
    container_points(counters, &mut out);
    out.sort_by_key(|point| (point.ts, point.key));
    out
}

const fn lane_point(key: &'static str, ts: i64, value: Option<f64>) -> LanePoint {
    LanePoint {
        key,
        ts,
        value,
        device: None,
        locks: None,
    }
}

fn disk_points(counters: &Counters, out: &mut Vec<LanePoint>) {
    let mut previous: Option<(i64, &DiskSnapshot)> = None;
    for (&ts, devices) in &counters.disks {
        let mut winner: Option<(f64, Option<f64>, DiskIdentity)> = None;
        if let Some((before, preceding)) = previous {
            for (key, current) in devices {
                let Some(prior) = preceding.get(key) else {
                    continue;
                };
                if current.identity.scope != Some(0) || prior.identity.scope != Some(0) {
                    continue;
                }
                let Some(busy) = disk_rate(current.busy, prior.busy, ts, before) else {
                    continue;
                };
                let busy = busy * 100.0;
                // One request is counted on every layer it crosses, so a partition, its
                // disk and a volume above them tie exactly. The tie names the device beneath.
                let leads =
                    winner
                        .as_ref()
                        .is_none_or(|(peak, _, leader)| match busy.total_cmp(peak) {
                            Ordering::Greater => true,
                            Ordering::Equal => sits_beneath(
                                &counters.disk_parents,
                                (leader.major, leader.minor),
                                *key,
                            ),
                            Ordering::Less => false,
                        });
                if leads {
                    winner = Some((
                        busy,
                        disk_rate(current.weighted, prior.weighted, ts, before),
                        current.identity.clone(),
                    ));
                }
            }
        }
        for (key, value) in [
            ("disk_busy", winner.as_ref().map(|(busy, _, _)| *busy)),
            (
                "disk_queue",
                winner.as_ref().and_then(|(_, queue, _)| *queue),
            ),
        ] {
            out.push(LanePoint {
                key,
                ts,
                value,
                device: winner.as_ref().map(|(_, _, identity)| identity.clone()),
                locks: None,
            });
        }
        previous = Some((ts, devices));
    }
}

/// Whether `candidate` lies beneath `upper` in the recorded block stack: the disk
/// under a partition, or a device an LVM, MD or dm volume lists among its slaves.
fn sits_beneath(
    parents: &BTreeMap<(i64, i64), BTreeSet<(i64, i64)>>,
    upper: (i64, i64),
    candidate: (i64, i64),
) -> bool {
    let mut seen = BTreeSet::from([upper]);
    let mut pending = vec![upper];
    while let Some(device) = pending.pop() {
        for &parent in parents.get(&device).into_iter().flatten() {
            if parent == candidate {
                return true;
            }
            if seen.insert(parent) {
                pending.push(parent);
            }
        }
    }
    false
}

fn disk_rate(current: Option<i64>, previous: Option<i64>, ts: i64, before: i64) -> Option<f64> {
    let delta = current?
        .checked_sub(previous?)
        .filter(|delta| *delta >= 0)?;
    let elapsed = ts.checked_sub(before).filter(|elapsed| *elapsed > 0)?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "recorded interval counter deltas fit f64"
    )]
    Some(delta as f64 * 1000.0 / elapsed as f64)
}

/// Resource lanes for the selected cgroup, using its recorded capacity.
fn container_points(counters: &Counters, out: &mut Vec<LanePoint>) {
    for (ts, cores) in group_rate(
        &counters.cg_cpu_usage,
        &counters.cg_cpu_boundaries,
        |value, seconds| value / 1_000_000.0 / seconds,
    ) {
        out.push(lane_point("cg_cpu_cores", ts, cores));
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
            out.push(lane_point("cg_cpu_share", ts, share));
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
            out.push(lane_point(key, ts, value));
        }
    }
    for (key, stored) in [
        ("cg_io_read", &counters.cg_io_read),
        ("cg_io_write", &counters.cg_io_write),
    ] {
        for (ts, value) in group_rate(stored, &counters.cg_io_boundaries, |value, seconds| {
            value / seconds
        }) {
            out.push(lane_point(key, ts, value));
        }
    }
    for (ts, value) in rate_samples(
        counters.cg_oom.iter().map(|(ts, value)| (*ts, *value)),
        counters.cg_oom.len(),
        |value, seconds| value / seconds,
        Some(&counters.cg_memory_boundaries),
    ) {
        out.push(lane_point("cg_oom", ts, value));
    }
    for (key, stored) in [
        ("cg_memory", &counters.cg_memory_share),
        ("cg_pids_share", &counters.cg_pids_share),
    ] {
        for (ts, value) in stored {
            out.push(lane_point(key, *ts, *value));
        }
    }
    for (key, stored) in [
        ("cg_memory_bytes", &counters.cg_memory_bytes),
        ("cg_pids", &counters.cg_pids),
    ] {
        for (ts, value) in stored {
            out.push(lane_point(key, *ts, Some(*value)));
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
pub(super) fn rate(
    stored: &BTreeMap<i64, i64>,
    scale: impl Fn(f64, f64) -> f64,
) -> Vec<(i64, Option<f64>)> {
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
