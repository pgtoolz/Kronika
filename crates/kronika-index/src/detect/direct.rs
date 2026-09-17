mod postgres;

use std::collections::BTreeMap;

use kronika_reader::{Cell, Segment};

use super::{
    CPU_IDLE_FIELD, FindingBuilder, LOAD1_FIELD, MEM_AVAILABLE_FIELD, MOUNT_FREE_BYTES_FIELD,
    OOM_KILL_FIELD, OS_CGROUP_MEMORY_V1, OS_CGROUP_MEMORY_V3, OS_CPU, OS_LOADAVG, OS_MEMINFO,
    OS_MOUNTINFO, OS_VMSTAT, OVERALL_HEALTH_FIELD, WRAPAROUND_AGE_THRESHOLD, optional_i64,
};

use crate::Index;
use crate::build::BuildError;
use crate::findings::{Finding, FindingKind};
use crate::series::SeriesBlock;

#[derive(Debug, Clone, Copy)]
pub(super) struct CpuRaw {
    pub(super) timestamp: i64,
    pub(super) counters: [i64; 8],
}

#[derive(Debug, Default)]
struct CpuSnapshot {
    aggregate: Option<(u32, CpuRaw)>,
    online: u32,
}

#[derive(Debug, Clone, Copy)]
struct ActiveSnapshot {
    type_id: u32,
    row_ordinal: u32,
    count: u32,
}

impl FindingBuilder {
    #[cfg(feature = "posix")]
    pub(super) fn observe_prior_cpu(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if segment.rows_of(OS_CPU).is_none() {
            return Ok(());
        }
        segment.visit_rows(OS_CPU, cpu_columns(), 0, usize::MAX, |_ordinal, row| {
            if matches!(row.get("scope"), Some(Cell::U32(0)))
                && matches!(row.get("cpu_id"), Some(Cell::I32(-1)))
                && let Some(raw) = cpu_raw(&row)
            {
                self.cpu_before = Some(raw);
            }
            true
        })?;
        Ok(())
    }

    #[cfg(feature = "posix")]
    pub(super) fn observe_prior_oom(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if segment.rows_of(OS_VMSTAT).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            OS_VMSTAT,
            &["ts", "oom_kill", "scope"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if matches!(row.get("scope"), Some(Cell::U32(0)))
                    && let Some(Cell::Ts(timestamp)) = row.get("ts")
                {
                    self.oom_before = Some((*timestamp, optional_i64(row.get("oom_kill"))));
                }
                true
            },
        )?;
        Ok(())
    }

    #[cfg(feature = "posix")]
    pub(super) fn observe_prior_cgroup_oom(
        &mut self,
        segment: &Segment,
        type_id: u32,
        identities: &BTreeMap<i64, [Option<u64>; 1]>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        if type_id == OS_CGROUP_MEMORY_V3 {
            for (timestamp, _ordinal, counter) in selected_oom_samples(segment)? {
                cgroup_oom_increased(
                    &mut self.cgroup_oom_v3_before,
                    identities
                        .range(..=timestamp)
                        .next_back()
                        .map(|(_, id)| *id),
                    timestamp,
                    counter,
                );
            }
            return Ok(());
        }
        segment.visit_rows(
            type_id,
            &["ts", "cgroup_path", "oom_kill"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::StrId(cgroup_path)),
                    Some(Cell::I64(oom_kill)),
                ) = (row.get("ts"), row.get("cgroup_path"), row.get("oom_kill"))
                {
                    self.cgroup_oom_before
                        .insert((type_id, *cgroup_path), (*timestamp, *oom_kill));
                }
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn find_cpu_and_online_count(
        &mut self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(OS_CPU).is_none()
            || !(self.requested.contains(&OS_CPU) || self.requested.contains(&OS_LOADAVG))
        {
            return Ok(());
        }
        let mut snapshots = BTreeMap::<i64, CpuSnapshot>::new();
        segment.visit_rows(OS_CPU, cpu_columns(), 0, usize::MAX, |ordinal, row| {
            let Some(Cell::Ts(timestamp)) = row.get("ts") else {
                return true;
            };
            if !matches!(row.get("scope"), Some(Cell::U32(0))) {
                return true;
            }
            let snapshot = snapshots.entry(*timestamp).or_default();
            match row.get("cpu_id") {
                Some(Cell::I32(-1)) => {
                    if let (Some(raw), Some(row_ordinal)) =
                        (cpu_raw(&row), u32::try_from(ordinal).ok())
                    {
                        snapshot.aggregate = Some((row_ordinal, raw));
                    }
                }
                Some(Cell::I32(cpu_id)) if *cpu_id >= 0 => {
                    snapshot.online = snapshot.online.saturating_add(1);
                }
                _ => {}
            }
            true
        })?;

        if self.requested.contains(&OS_CPU) {
            let cpu_hits = hits.entry(OS_CPU).or_default();
            for snapshot in snapshots.values() {
                let Some((row_ordinal, current)) = snapshot.aggregate else {
                    continue;
                };
                if self
                    .cpu_before
                    .is_some_and(|before| cpu_busy_at_least_80(before, current))
                {
                    cpu_hits.push(known_bad(CPU_IDLE_FIELD, row_ordinal, current.timestamp));
                }
                self.cpu_before = Some(current);
            }
        }
        self.find_load_with_cpus(segment, &snapshots, hits)
    }

    fn find_load_with_cpus(
        &self,
        segment: &Segment,
        snapshots: &BTreeMap<i64, CpuSnapshot>,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_LOADAVG) || segment.rows_of(OS_LOADAVG).is_none() {
            return Ok(());
        }
        let load_hits = hits.entry(OS_LOADAVG).or_default();
        segment.visit_rows(
            OS_LOADAVG,
            &["ts", "load1", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::F64(load1)),
                    Some(Cell::U32(0)),
                    Some(row_ordinal),
                ) = (
                    row.get("ts"),
                    row.get("load1"),
                    row.get("scope"),
                    u32::try_from(ordinal).ok(),
                )
                else {
                    return true;
                };
                let online = snapshots
                    .get(timestamp)
                    .map_or(0, |snapshot| snapshot.online);
                if online != 0 && load1.is_finite() && *load1 >= 2.0 * f64::from(online) {
                    load_hits.push(known_bad(LOAD1_FIELD, row_ordinal, *timestamp));
                }
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn find_memory(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_MEMINFO) || segment.rows_of(OS_MEMINFO).is_none() {
            return Ok(());
        }
        let memory_hits = hits.entry(OS_MEMINFO).or_default();
        segment.visit_rows(
            OS_MEMINFO,
            &["ts", "mem_total", "mem_available", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::I64(total)),
                    Some(Cell::I64(available)),
                    Some(Cell::U32(0)),
                    Some(row_ordinal),
                ) = (
                    row.get("ts"),
                    row.get("mem_total"),
                    row.get("mem_available"),
                    row.get("scope"),
                    u32::try_from(ordinal).ok(),
                )
                else {
                    return true;
                };
                if *total > 0
                    && *available >= 0
                    && available <= total
                    && i128::from(*available) * 100 <= i128::from(*total) * 10
                {
                    memory_hits.push(known_bad(MEM_AVAILABLE_FIELD, row_ordinal, *timestamp));
                }
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn find_mounts(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_MOUNTINFO) || segment.rows_of(OS_MOUNTINFO).is_none() {
            return Ok(());
        }
        let mount_hits = hits.entry(OS_MOUNTINFO).or_default();
        segment.visit_rows(
            OS_MOUNTINFO,
            &["ts", "total_bytes", "free_bytes", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::I64(total)),
                    Some(Cell::I64(free)),
                    Some(Cell::U32(0)),
                    Some(row_ordinal),
                ) = (
                    row.get("ts"),
                    row.get("total_bytes"),
                    row.get("free_bytes"),
                    row.get("scope"),
                    u32::try_from(ordinal).ok(),
                )
                else {
                    return true;
                };
                if *total > 0
                    && *free >= 0
                    && free <= total
                    && i128::from(*total - *free) * 100 >= i128::from(*total) * 90
                {
                    mount_hits.push(known_bad(MOUNT_FREE_BYTES_FIELD, row_ordinal, *timestamp));
                }
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn find_oom(
        &mut self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_VMSTAT) || segment.rows_of(OS_VMSTAT).is_none() {
            return Ok(());
        }
        let oom_hits = hits.entry(OS_VMSTAT).or_default();
        segment.visit_rows(
            OS_VMSTAT,
            &["ts", "oom_kill", "scope"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (Some(Cell::Ts(timestamp)), Some(Cell::U32(0))) =
                    (row.get("ts"), row.get("scope"))
                else {
                    return true;
                };
                let current = optional_i64(row.get("oom_kill"));
                if let (Some((before_ts, Some(before))), Some(after), Some(row_ordinal)) =
                    (self.oom_before, current, u32::try_from(ordinal).ok())
                    && *timestamp > before_ts
                    && after > before
                {
                    oom_hits.push(known_bad(OOM_KILL_FIELD, row_ordinal, *timestamp));
                }
                self.oom_before = Some((*timestamp, current));
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn find_cgroup_oom(
        &mut self,
        segment: &Segment,
        type_id: u32,
        identities: &BTreeMap<i64, [Option<u64>; 1]>,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let cgroup_hits = hits.entry(type_id).or_default();
        let field_ordinal = cgroup_oom_kill_field(type_id);
        if type_id == OS_CGROUP_MEMORY_V3 {
            for (timestamp, ordinal, counter) in selected_oom_samples(segment)? {
                let increased = cgroup_oom_increased(
                    &mut self.cgroup_oom_v3_before,
                    identities
                        .range(..=timestamp)
                        .next_back()
                        .map(|(_, id)| *id),
                    timestamp,
                    counter,
                );
                if increased && let Ok(row_ordinal) = u32::try_from(ordinal) {
                    cgroup_hits.push(known_bad(field_ordinal, row_ordinal, timestamp));
                }
            }
            return Ok(());
        }
        segment.visit_rows(
            type_id,
            &["ts", "cgroup_path", "oom_kill"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::StrId(cgroup_path)),
                    Some(Cell::I64(oom_kill)),
                ) = (row.get("ts"), row.get("cgroup_path"), row.get("oom_kill"))
                else {
                    return true;
                };
                let key = (type_id, *cgroup_path);
                if let (Some((before_ts, before)), Some(row_ordinal)) = (
                    self.cgroup_oom_before.get(&key).copied(),
                    u32::try_from(ordinal).ok(),
                ) && *timestamp > before_ts
                    && *oom_kill > before
                {
                    cgroup_hits.push(Finding {
                        kind: FindingKind::KnownBad,
                        category: None,
                        field_ordinal,
                        row_ordinal,
                        timestamp: *timestamp,
                    });
                }
                self.cgroup_oom_before.insert(key, (*timestamp, *oom_kill));
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn find_overall_health(
        &self,
        index: &Index,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) {
        if !self.requested.contains(&0) {
            return;
        }
        let health_hits = hits.entry(0).or_default();
        for block in &index.blocks {
            if let SeriesBlock::OverallHealth(points) = block {
                for (ordinal, point) in points.iter().enumerate() {
                    if point.value.is_some_and(|value| value < 50)
                        && let Some(row_ordinal) = u32::try_from(ordinal).ok()
                    {
                        health_hits.push(known_bad(
                            OVERALL_HEALTH_FIELD,
                            row_ordinal,
                            point.timestamp,
                        ));
                    }
                }
            }
        }
    }
}

fn cpu_raw(row: &kronika_reader::Row) -> Option<CpuRaw> {
    let Some(Cell::Ts(timestamp)) = row.get("ts") else {
        return None;
    };
    let mut counters = [0_i64; 8];
    for (at, name) in [
        "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal",
    ]
    .into_iter()
    .enumerate()
    {
        let Some(Cell::I64(value)) = row.get(name) else {
            return None;
        };
        counters[at] = *value;
    }
    Some(CpuRaw {
        timestamp: *timestamp,
        counters,
    })
}

pub(super) fn cpu_busy_at_least_80(before: CpuRaw, current: CpuRaw) -> bool {
    if current.timestamp <= before.timestamp {
        return false;
    }
    let mut deltas = [0_i128; 8];
    for (at, (after, before)) in current
        .counters
        .into_iter()
        .zip(before.counters)
        .enumerate()
    {
        let delta = i128::from(after) - i128::from(before);
        if delta < 0 {
            return false;
        }
        deltas[at] = delta;
    }
    let busy = deltas[0] + deltas[1] + deltas[2] + deltas[5] + deltas[6] + deltas[7];
    let total: i128 = deltas.into_iter().sum();
    total > 0 && busy * 100 >= total * 80
}

const fn known_bad(field_ordinal: u16, row_ordinal: u32, timestamp: i64) -> Finding {
    Finding {
        kind: FindingKind::KnownBad,
        category: None,
        field_ordinal,
        row_ordinal,
        timestamp,
    }
}

pub(super) const fn crosses_wraparound_age(age: i64) -> bool {
    age >= WRAPAROUND_AGE_THRESHOLD
}

/// Compare adjacent selected-group samples; missing identity/counters break continuity.
pub(super) fn cgroup_oom_increased(
    before: &mut Option<super::CgroupOomSample>,
    identity: Option<[Option<u64>; 1]>,
    timestamp: i64,
    counter: Option<i64>,
) -> bool {
    let counter = counter.filter(|value| *value >= 0);
    let increased = match (*before, identity, counter) {
        (Some((before_id, before_ts, Some(before_value))), Some(id), Some(value)) => {
            id == before_id && timestamp > before_ts && value > before_value
        }
        _ => false,
    };
    *before = identity.map(|id| (id, timestamp, counter));
    increased
}

fn selected_oom_samples(segment: &Segment) -> Result<Vec<(i64, u64, Option<i64>)>, BuildError> {
    let mut samples = Vec::new();
    segment.visit_rows(
        OS_CGROUP_MEMORY_V3,
        &["ts", "oom_kill"],
        0,
        usize::MAX,
        |ordinal, row| {
            if let Some(Cell::Ts(timestamp)) = row.get("ts") {
                samples.push((*timestamp, ordinal, optional_i64(row.get("oom_kill"))));
            }
            true
        },
    )?;
    // The encoded section is sorted by path first; selected paths can change.
    samples.sort_unstable_by_key(|sample| (sample.0, sample.1));
    Ok(samples)
}

const fn cpu_columns() -> &'static [&'static str] {
    &[
        "ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal",
        "scope",
    ]
}

/// `1_202_002` inserted `shmem` ahead of `oom_kill`, shifting its ordinal.
const fn cgroup_oom_kill_field(type_id: u32) -> u16 {
    if type_id == OS_CGROUP_MEMORY_V1 {
        12
    } else {
        13
    }
}
