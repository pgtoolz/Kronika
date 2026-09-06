mod direct;

use std::collections::{BTreeMap, BTreeSet};

#[cfg(feature = "posix")]
use kronika_reader::SegmentRef;
use kronika_reader::{Cell, Segment};

use crate::Index;
use crate::build::{ActiveBackendSample, BuildError};
use crate::cpu_capacity::RecordedCpuCapacity;
use crate::findings::{
    Finding, FindingBlock, FindingKind, MAX_FINDINGS_PER_BLOCK, MAX_LOG_ERROR_CATEGORY,
    PG_LOG_ERRORS_TYPE_ID,
};
use crate::series::{SeriesBlock, SeriesKey, SeriesKind};

use self::direct::CpuRaw;

const OS_CPU: u32 = 1_102_001;
const OS_MEMINFO: u32 = 1_104_001;
const OS_LOADAVG: u32 = 1_105_001;
const OS_VMSTAT: u32 = 1_106_001;
const OS_MOUNTINFO: u32 = 1_112_002;
const PG_LOCKS_V1: u32 = 1_011_001;
const PG_LOCKS_V2: u32 = 1_011_002;
const PG_STAT_ARCHIVER: u32 = 1_008_001;
const OS_CGROUP_MEMORY_V1: u32 = 1_202_001;
const OS_CGROUP_MEMORY_V2: u32 = 1_202_002;
const PG_LOG_SLOW_QUERIES: u32 = 2_004_001;
const PG_LOG_EVENT_LAYOUTS: [u32; 6] = [
    PG_LOG_ERRORS_TYPE_ID,
    2_002_001,
    2_003_001,
    PG_LOG_SLOW_QUERIES,
    2_005_002,
    2_006_001,
];
#[cfg(feature = "posix")]
const FIFTEEN_MINUTES_US: i64 = 15 * 60 * 1_000_000;

const OVERALL_HEALTH_FIELD: u16 = 1;
const CPU_IDLE_FIELD: u16 = 5;
const MEM_AVAILABLE_FIELD: u16 = 3;
const LOAD1_FIELD: u16 = 1;
const OOM_KILL_FIELD: u16 = 11;
const MOUNT_FREE_BYTES_FIELD: u16 = 9;
const SLOW_QUERY_DURATION_FIELD: u16 = 6;
const LOCKS_BLOCKED_BY_FIELD: u16 = 2;
const DATABASE_DEADLOCKS_FIELD: u16 = 16;
const FROZEN_XID_AGE_FIELD: u16 = 20;
const MIN_MXID_AGE_FIELD: u16 = 21;
const CHECKSUM_FAILURES_FIELD: u16 = 25;
const SESSIONS_FATAL_FIELD: u16 = 32;
const SESSIONS_KILLED_FIELD: u16 = 33;
const ARCHIVER_FAILED_COUNT_FIELD: u16 = 4;
const LOG_ERROR_CATEGORY_FIELD: u16 = 4;
const DATA_CORRUPTION_CATEGORY: u8 = 5;
/// `PostgreSQL`'s own `vacuum_failsafe_age` / `vacuum_multixact_failsafe_age` default.
const WRAPAROUND_AGE_THRESHOLD: i64 = 1_600_000_000;
const EVENT_TIMESTAMP_FIELD: u16 = 0;

#[derive(Debug)]
pub(crate) struct FindingBuilder {
    requested: BTreeSet<u32>,
    #[cfg(feature = "posix")]
    cutoff: i64,
    cpu_before: Option<CpuRaw>,
    oom_before: Option<(i64, Option<i64>)>,
    archiver_before: Option<(i64, i64)>,
    deadlocks_before: BTreeMap<(u32, u32), (i64, i64)>,
    checksum_failures_before: BTreeMap<(u32, u32), (i64, Option<i64>)>,
    sessions_before: BTreeMap<(u32, u32), (i64, i64, i64)>,
    cgroup_oom_before: BTreeMap<(u32, u64), (i64, i64)>,
}

impl FindingBuilder {
    /// Discover only concrete series that occur in the target segment.
    pub(crate) fn new(segment: &Segment, requested: &[SeriesKey]) -> Self {
        #[cfg(not(feature = "posix"))]
        let _ = segment;
        let requested: BTreeSet<u32> = requested
            .iter()
            .filter(|key| key.kind == SeriesKind::Findings)
            .map(|key| key.type_id)
            .filter(|type_id| finding_layout(*type_id))
            .collect();
        Self {
            requested,
            #[cfg(feature = "posix")]
            cutoff: segment.min_ts().saturating_sub(FIFTEEN_MINUTES_US),
            cpu_before: None,
            oom_before: None,
            archiver_before: None,
            deadlocks_before: BTreeMap::new(),
            checksum_failures_before: BTreeMap::new(),
            sessions_before: BTreeMap::new(),
            cgroup_oom_before: BTreeMap::new(),
        }
    }

    /// Earliest preferred timestamp for adjacent counter inputs.
    #[cfg(feature = "posix")]
    pub(crate) const fn window_start(&self) -> i64 {
        self.cutoff
    }

    /// Whether a prior segment can contribute a required predecessor.
    #[cfg(feature = "posix")]
    pub(crate) fn needs(&self, segment: &SegmentRef) -> bool {
        segment.sections().iter().any(|section| {
            self.requested.contains(&section.type_id)
                && needs_prior_rows(section.type_id)
                && section.rows != 0
        })
    }

    #[cfg(feature = "posix")]
    pub(crate) fn observe_prior(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if self.requested.contains(&OS_CPU) {
            self.observe_prior_cpu(segment)?;
        }
        if self.requested.contains(&OS_VMSTAT) {
            self.observe_prior_oom(segment)?;
        }
        if self.requested.contains(&PG_STAT_ARCHIVER) {
            self.observe_prior_archiver(segment)?;
        }
        for type_id in database_layouts() {
            if self.requested.contains(&type_id) {
                self.observe_prior_database_counters(segment, type_id)?;
            }
        }
        for type_id in cgroup_memory_layouts() {
            if self.requested.contains(&type_id) {
                self.observe_prior_cgroup_oom(segment, type_id)?;
            }
        }
        Ok(())
    }

    fn find_log_events(
        &self,
        segment: &Segment,
        hits: &mut BTreeMap<u32, Vec<Finding>>,
    ) -> Result<(), BuildError> {
        for type_id in PG_LOG_EVENT_LAYOUTS {
            if !self.requested.contains(&type_id) || segment.rows_of(type_id).is_none() {
                continue;
            }
            let event_hits = hits.entry(type_id).or_default();
            let fields: &[&str] = if type_id == PG_LOG_ERRORS_TYPE_ID {
                &["ts", "category"]
            } else {
                &["ts"]
            };
            let mut invalid_category = false;
            segment.visit_rows(type_id, fields, 0, usize::MAX, |ordinal, row| {
                if let (Some(Cell::Ts(timestamp)), Some(row_ordinal)) =
                    (row.get("ts"), u32::try_from(ordinal).ok())
                {
                    let category = if type_id == PG_LOG_ERRORS_TYPE_ID {
                        let Some(Cell::U32(value)) = row.get("category") else {
                            invalid_category = true;
                            return false;
                        };
                        let Ok(value @ 0..=MAX_LOG_ERROR_CATEGORY) = u8::try_from(*value) else {
                            invalid_category = true;
                            return false;
                        };
                        Some(value)
                    } else {
                        None
                    };
                    event_hits.push(Finding {
                        kind: FindingKind::Event,
                        field_ordinal: EVENT_TIMESTAMP_FIELD,
                        row_ordinal,
                        timestamp: *timestamp,
                        category,
                    });
                    if category == Some(DATA_CORRUPTION_CATEGORY) {
                        event_hits.push(Finding {
                            kind: FindingKind::KnownBad,
                            field_ordinal: LOG_ERROR_CATEGORY_FIELD,
                            row_ordinal,
                            timestamp: *timestamp,
                            category,
                        });
                    }
                }
                true
            })?;
            if invalid_category {
                return Err(BuildError::InvalidLogErrorCategory);
            }
        }
        Ok(())
    }

    /// Build every requested finding block, including empty supported blocks.
    pub(crate) fn finish(
        mut self,
        segment: &Segment,
        index: &Index,
        active_samples: &BTreeMap<u32, Vec<ActiveBackendSample>>,
        postgres_cpus: &RecordedCpuCapacity,
    ) -> Result<Vec<SeriesBlock>, BuildError> {
        let mut hits = BTreeMap::<u32, Vec<Finding>>::new();
        for type_id in &self.requested {
            hits.insert(*type_id, Vec::new());
        }

        self.find_log_events(segment, &mut hits)?;
        self.find_cpu_and_online_count(segment, &mut hits)?;
        self.find_memory(segment, &mut hits)?;
        self.find_mounts(segment, &mut hits)?;
        self.find_slow_queries(segment, &mut hits)?;
        self.find_oom(segment, &mut hits)?;
        self.find_archiver_failures(segment, &mut hits)?;
        for type_id in database_layouts() {
            if self.requested.contains(&type_id) {
                self.find_database_counters(segment, type_id, &mut hits)?;
            }
        }
        for type_id in pg_locks_layouts() {
            if self.requested.contains(&type_id) {
                self.find_lock_contention(segment, type_id, &mut hits)?;
            }
        }
        for type_id in cgroup_memory_layouts() {
            if self.requested.contains(&type_id) {
                self.find_cgroup_oom(segment, type_id, &mut hits)?;
            }
        }
        self.find_active_backends(active_samples, postgres_cpus, &mut hits);
        self.find_overall_health(index, &mut hits);

        Ok(hits
            .into_iter()
            .map(|(type_id, findings)| SeriesBlock::Findings(block(type_id, findings)))
            .collect())
    }
}

pub(crate) fn finding_layout(type_id: u32) -> bool {
    PG_LOG_EVENT_LAYOUTS.contains(&type_id)
        || matches!(
            type_id,
            0 | OS_CPU
                | OS_MEMINFO
                | OS_LOADAVG
                | OS_VMSTAT
                | OS_MOUNTINFO
                | PG_LOCKS_V1
                | PG_LOCKS_V2
                | PG_STAT_ARCHIVER
                | OS_CGROUP_MEMORY_V1
                | OS_CGROUP_MEMORY_V2
                | 1_001_001
                | 1_001_002
                | 1_001_004
                | 1_005_001..=1_005_004
        )
}

#[cfg(feature = "posix")]
const fn needs_prior_rows(type_id: u32) -> bool {
    matches!(
        type_id,
        OS_CPU
            | OS_VMSTAT
            | PG_STAT_ARCHIVER
            | OS_CGROUP_MEMORY_V1
            | OS_CGROUP_MEMORY_V2
            | 1_005_001..=1_005_004
    )
}

fn block(type_id: u32, mut findings: Vec<Finding>) -> FindingBlock {
    findings.sort_unstable_by_key(|finding| {
        (
            finding.timestamp,
            finding.row_ordinal,
            finding.field_ordinal,
            finding.kind,
        )
    });
    findings.dedup();
    let total_hits = u32::try_from(findings.len()).unwrap_or(u32::MAX);
    findings.truncate(MAX_FINDINGS_PER_BLOCK);
    FindingBlock {
        type_id,
        total_hits,
        truncated: usize::try_from(total_hits).map_or(true, |total| total > findings.len()),
        findings,
    }
}

const fn optional_i64(cell: Option<&Cell>) -> Option<i64> {
    match cell {
        Some(Cell::I64(value)) => Some(*value),
        _ => None,
    }
}

const fn database_layouts() -> [u32; 4] {
    [1_005_001, 1_005_002, 1_005_003, 1_005_004]
}

const fn pg_locks_layouts() -> [u32; 2] {
    [PG_LOCKS_V1, PG_LOCKS_V2]
}

/// `checksum_failures` is absent on the base `1_005_001` layout.
pub(super) const fn has_checksum(type_id: u32) -> bool {
    matches!(type_id, 1_005_002..=1_005_004)
}

/// `sessions_fatal` / `sessions_killed` arrived with PG14.
pub(super) const fn has_sessions(type_id: u32) -> bool {
    matches!(type_id, 1_005_003 | 1_005_004)
}

const fn cgroup_memory_layouts() -> [u32; 2] {
    [OS_CGROUP_MEMORY_V1, OS_CGROUP_MEMORY_V2]
}

const fn activity_layouts() -> [u32; 3] {
    [1_001_001, 1_001_002, 1_001_004]
}

#[cfg(test)]
mod tests;
