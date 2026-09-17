//! Findings derived from `PostgreSQL` counters and recorded events.

use std::collections::BTreeMap;

use kronika_reader::{Cell, Segment};

use super::{ActiveSnapshot, crosses_wraparound_age, known_bad};

use crate::build::{ActiveBackendSample, BuildError};
use crate::cpu_capacity::RecordedCpuCapacity;
use crate::detect::{
    ARCHIVER_FAILED_COUNT_FIELD, CHECKSUM_FAILURES_FIELD, DATABASE_DEADLOCKS_FIELD,
    FROZEN_XID_AGE_FIELD, FindingBuilder, LOCKS_BLOCKED_BY_FIELD, MIN_MXID_AGE_FIELD,
    PG_LOG_SLOW_QUERIES, PG_STAT_ARCHIVER, SESSIONS_FATAL_FIELD, SESSIONS_KILLED_FIELD,
    SLOW_QUERY_DURATION_FIELD, activity_layouts, has_checksum, has_sessions, optional_i64,
};
use crate::findings::Finding;

impl FindingBuilder {
    #[cfg(feature = "posix")]
    pub(in crate::detect) fn observe_prior_database_counters(
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
    pub(in crate::detect) fn observe_prior_archiver(
        &mut self,
        segment: &Segment,
    ) -> Result<(), BuildError> {
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
    pub(in crate::detect) fn find_slow_queries(
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
    pub(in crate::detect) fn find_archiver_failures(
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
    pub(in crate::detect) fn find_database_counters(
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
    pub(in crate::detect) fn find_lock_contention(
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

    pub(in crate::detect) fn find_active_backends(
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

const fn activity_state_field(type_id: u32) -> u16 {
    if type_id == 1_001_001 { 7 } else { 8 }
}
