use std::collections::BTreeMap;

use kronika_reader::{Cell, Segment};

use crate::Index;
use crate::build::{ActiveBackendSample, BuildError};
use crate::cpu_capacity::RecordedCpuCapacity;
use crate::findings::{Finding, FindingKind};
use crate::series::SeriesBlock;

use super::{
    ARCHIVER_FAILED_COUNT_FIELD, CHECKSUM_FAILURES_FIELD, CPU_IDLE_FIELD, DATABASE_DEADLOCKS_FIELD,
    FROZEN_XID_AGE_FIELD, FindingBuilder, LOAD1_FIELD, LOCKS_BLOCKED_BY_FIELD, MEM_AVAILABLE_FIELD,
    MIN_MXID_AGE_FIELD, MOUNT_FREE_BYTES_FIELD, OOM_KILL_FIELD, OS_CGROUP_MEMORY_V1, OS_CPU,
    OS_LOADAVG, OS_MEMINFO, OS_MOUNTINFO, OS_VMSTAT, OVERALL_HEALTH_FIELD, PG_LOG_SLOW_QUERIES,
    PG_STAT_ARCHIVER, SESSIONS_FATAL_FIELD, SESSIONS_KILLED_FIELD, SLOW_QUERY_DURATION_FIELD,
    WRAPAROUND_AGE_THRESHOLD, activity_layouts, has_checksum, has_sessions, optional_i64,
};

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
    pub(super) fn observe_prior_database_counters(
        &mut self,
        segment: &Segment,
        type_id: u32,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let has_checksum = has_checksum(type_id);
        let has_sessions = has_sessions(type_id);
        let mut fields: Vec<&str> = vec!["ts", "datid", "deadlocks"];
        if has_checksum {
            fields.push("checksum_failures");
        }
        if has_sessions {
            fields.push("sessions_fatal");
            fields.push("sessions_killed");
        }
        segment.visit_rows(type_id, &fields, 0, usize::MAX, |_ordinal, row| {
            let (Some(Cell::Ts(timestamp)), Some(Cell::U32(datid)), Some(Cell::I64(deadlocks))) =
                (row.get("ts"), row.get("datid"), row.get("deadlocks"))
            else {
                return true;
            };
            let key = (type_id, *datid);
            self.deadlocks_before.insert(key, (*timestamp, *deadlocks));
            if has_checksum {
                self.checksum_failures_before.insert(
                    key,
                    (*timestamp, optional_i64(row.get("checksum_failures"))),
                );
            }
            if has_sessions
                && let (Some(Cell::I64(fatal)), Some(Cell::I64(killed))) =
                    (row.get("sessions_fatal"), row.get("sessions_killed"))
            {
                self.sessions_before
                    .insert(key, (*timestamp, *fatal, *killed));
            }
            true
        })?;
        Ok(())
    }

    #[cfg(feature = "posix")]
    pub(super) fn observe_prior_archiver(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if segment.rows_of(PG_STAT_ARCHIVER).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            PG_STAT_ARCHIVER,
            &["ts", "failed_count"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if let (Some(Cell::Ts(timestamp)), Some(Cell::I64(failed_count))) =
                    (row.get("ts"), row.get("failed_count"))
                {
                    self.archiver_before = Some((*timestamp, *failed_count));
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
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
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

    pub(super) fn find_slow_queries(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&PG_LOG_SLOW_QUERIES)
            || segment.rows_of(PG_LOG_SLOW_QUERIES).is_none()
        {
            return Ok(());
        }
        let query_hits = hits.entry(PG_LOG_SLOW_QUERIES).or_default();
        segment.visit_rows(
            PG_LOG_SLOW_QUERIES,
            &["ts", "max_duration_ms"],
            0,
            usize::MAX,
            |ordinal, row| {
                if let (Some(Cell::Ts(timestamp)), Some(Cell::F64(duration)), Some(row_ordinal)) = (
                    row.get("ts"),
                    row.get("max_duration_ms"),
                    u32::try_from(ordinal).ok(),
                ) && duration.is_finite()
                    && *duration >= 5_000.0
                {
                    query_hits.push(known_bad(
                        SLOW_QUERY_DURATION_FIELD,
                        row_ordinal,
                        *timestamp,
                    ));
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

    pub(super) fn find_archiver_failures(
        &mut self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&PG_STAT_ARCHIVER)
            || segment.rows_of(PG_STAT_ARCHIVER).is_none()
        {
            return Ok(());
        }
        let archiver_hits = hits.entry(PG_STAT_ARCHIVER).or_default();
        segment.visit_rows(
            PG_STAT_ARCHIVER,
            &["ts", "failed_count"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (Some(Cell::Ts(timestamp)), Some(Cell::I64(failed_count))) =
                    (row.get("ts"), row.get("failed_count"))
                else {
                    return true;
                };
                if let (Some((before_ts, before)), Some(row_ordinal)) =
                    (self.archiver_before, u32::try_from(ordinal).ok())
                    && *timestamp > before_ts
                    && *failed_count > before
                {
                    archiver_hits.push(known_bad(
                        ARCHIVER_FAILED_COUNT_FIELD,
                        row_ordinal,
                        *timestamp,
                    ));
                }
                self.archiver_before = Some((*timestamp, *failed_count));
                true
            },
        )?;
        Ok(())
    }

    /// Absolute ages need no predecessor. Null checksum counters are skipped.
    /// Fatal and killed session counters are tested separately.
    pub(super) fn find_database_counters(
        &mut self,
        segment: &Segment,
        type_id: u32,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&type_id) || segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let has_checksum = has_checksum(type_id);
        let has_sessions = has_sessions(type_id);
        let mut fields: Vec<&str> =
            vec!["ts", "datid", "deadlocks", "frozen_xid_age", "min_mxid_age"];
        if has_checksum {
            fields.push("checksum_failures");
        }
        if has_sessions {
            fields.push("sessions_fatal");
            fields.push("sessions_killed");
        }
        let database_hits = hits.entry(type_id).or_default();
        segment.visit_rows(type_id, &fields, 0, usize::MAX, |ordinal, row| {
            let (
                Some(Cell::Ts(timestamp)),
                Some(Cell::U32(datid)),
                Some(Cell::I64(deadlocks)),
                Some(row_ordinal),
            ) = (
                row.get("ts"),
                row.get("datid"),
                row.get("deadlocks"),
                u32::try_from(ordinal).ok(),
            )
            else {
                return true;
            };
            let key = (type_id, *datid);
            push_wraparound_findings(database_hits, &row, row_ordinal, *timestamp);
            self.check_deadlocks(database_hits, key, row_ordinal, *timestamp, *deadlocks);
            if has_checksum {
                self.check_checksum_failures(database_hits, key, row_ordinal, *timestamp, &row);
            }
            if has_sessions {
                self.check_sessions(database_hits, key, row_ordinal, *timestamp, &row);
            }
            true
        })?;
        Ok(())
    }

    fn check_deadlocks(
        &mut self,
        hits: &mut Vec<Finding>,
        key: (u32, u32),
        row_ordinal: u32,
        timestamp: i64,
        deadlocks: i64,
    ) {
        if let Some((before_ts, before)) = self.deadlocks_before.get(&key).copied()
            && timestamp > before_ts
            && deadlocks > before
        {
            hits.push(known_bad(DATABASE_DEADLOCKS_FIELD, row_ordinal, timestamp));
        }
        self.deadlocks_before.insert(key, (timestamp, deadlocks));
    }

    fn check_checksum_failures(
        &mut self,
        hits: &mut Vec<Finding>,
        key: (u32, u32),
        row_ordinal: u32,
        timestamp: i64,
        row: &kronika_reader::Row,
    ) {
        let current = optional_i64(row.get("checksum_failures"));
        if let (Some((before_ts, Some(before))), Some(after)) =
            (self.checksum_failures_before.get(&key).copied(), current)
            && timestamp > before_ts
            && after > before
        {
            hits.push(known_bad(CHECKSUM_FAILURES_FIELD, row_ordinal, timestamp));
        }
        self.checksum_failures_before
            .insert(key, (timestamp, current));
    }

    fn check_sessions(
        &mut self,
        hits: &mut Vec<Finding>,
        key: (u32, u32),
        row_ordinal: u32,
        timestamp: i64,
        row: &kronika_reader::Row,
    ) {
        let (Some(Cell::I64(fatal)), Some(Cell::I64(killed))) =
            (row.get("sessions_fatal"), row.get("sessions_killed"))
        else {
            return;
        };
        if let Some((before_ts, before_fatal, before_killed)) =
            self.sessions_before.get(&key).copied()
            && timestamp > before_ts
        {
            if *fatal > before_fatal {
                hits.push(known_bad(SESSIONS_FATAL_FIELD, row_ordinal, timestamp));
            }
            if *killed > before_killed {
                hits.push(known_bad(SESSIONS_KILLED_FIELD, row_ordinal, timestamp));
            }
        }
        self.sessions_before
            .insert(key, (timestamp, *fatal, *killed));
    }

    pub(super) fn find_cgroup_oom(
        &mut self,
        segment: &Segment,
        type_id: u32,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let cgroup_hits = hits.entry(type_id).or_default();
        let field_ordinal = cgroup_oom_kill_field(type_id);
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

    pub(super) fn find_lock_contention(
        &self,
        segment: &Segment,
        type_id: u32,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        if !self.requested.contains(&type_id) || segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let lock_hits = hits.entry(type_id).or_default();
        segment.visit_rows(
            type_id,
            &["ts", "blocked_by"],
            0,
            usize::MAX,
            |ordinal, row| {
                if let (
                    Some(Cell::Ts(timestamp)),
                    Some(Cell::ListI32(blocked_by)),
                    Some(row_ordinal),
                ) = (
                    row.get("ts"),
                    row.get("blocked_by"),
                    u32::try_from(ordinal).ok(),
                ) && !blocked_by.is_empty()
                {
                    lock_hits.push(known_bad(LOCKS_BLOCKED_BY_FIELD, row_ordinal, *timestamp));
                }
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn find_active_backends(
        &self,
        samples: &BTreeMap<u32, Vec<ActiveBackendSample>>,
        postgres_cpus: &RecordedCpuCapacity,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) {
        let requested: Vec<u32> = activity_layouts()
            .into_iter()
            .filter(|type_id| self.requested.contains(type_id))
            .collect();
        if requested.is_empty() {
            return;
        }
        let mut combined = BTreeMap::<i64, Option<ActiveSnapshot>>::new();
        for type_id in requested {
            let Some(samples) = samples.get(&type_id) else {
                continue;
            };
            for sample in samples {
                let Some(row_ordinal) = sample.first_active_ordinal else {
                    continue;
                };
                combined
                    .entry(sample.timestamp)
                    .and_modify(|sample| *sample = None)
                    .or_insert(Some(ActiveSnapshot {
                        type_id,
                        row_ordinal,
                        count: sample.count,
                    }));
            }
        }
        for (timestamp, sample) in combined {
            if let Some(sample) = sample
                && let Some(cpus) = postgres_cpus.at(timestamp)
                && f64::from(sample.count) > 2.0 * cpus
            {
                hits.entry(sample.type_id).or_default().push(known_bad(
                    activity_state_field(sample.type_id),
                    sample.row_ordinal,
                    timestamp,
                ));
            }
        }
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

fn push_wraparound_findings(
    hits: &mut Vec<Finding>,
    row: &kronika_reader::Row,
    row_ordinal: u32,
    timestamp: i64,
) {
    if optional_i64(row.get("frozen_xid_age")).is_some_and(crosses_wraparound_age) {
        hits.push(known_bad(FROZEN_XID_AGE_FIELD, row_ordinal, timestamp));
    }
    if optional_i64(row.get("min_mxid_age")).is_some_and(crosses_wraparound_age) {
        hits.push(known_bad(MIN_MXID_AGE_FIELD, row_ordinal, timestamp));
    }
}

const fn cpu_columns() -> &'static [&'static str] {
    &[
        "ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal",
        "scope",
    ]
}

const fn activity_state_field(type_id: u32) -> u16 {
    if type_id == 1_001_001 { 7 } else { 8 }
}

/// `1_202_002` inserted `shmem` ahead of `oom_kill`, shifting its ordinal.
const fn cgroup_oom_kill_field(type_id: u32) -> u16 {
    if type_id == OS_CGROUP_MEMORY_V1 {
        12
    } else {
        13
    }
}
