//! Exact grouping rules shared by the Events transports.

use std::collections::{HashMap, HashSet};

use icu_collator::options::{AlternateHandling, CaseLevel, CollatorOptions, Strength};
use icu_collator::{Collator, CollatorBorrowed};
use icu_locale_core::locale;
use serde_json::{Value, json};

use super::{
    EventDataRow, EventGroup, EventSource, EventStat, EventTier, MINUTE_COLUMNS, MINUTE_MICROS,
};
use crate::QueryError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RowOrder {
    timestamp: i64,
    encounter: u64,
}

struct Summary {
    representative: EventDataRow,
    representative_order: RowOrder,
    representative_score: f64,
    first_ts: i64,
    last_ts: i64,
    minutes: Vec<f64>,
    count: f64,
}

impl Summary {
    fn new(row: EventDataRow, order: RowOrder, from: i64, weight: f64, score: f64) -> Self {
        let timestamp = row.timestamp;
        let mut summary = Self {
            representative: row,
            representative_order: order,
            representative_score: score,
            first_ts: timestamp,
            last_ts: timestamp,
            minutes: vec![0.0; MINUTE_COLUMNS],
            count: 0.0,
        };
        summary.add_minute(timestamp, from, weight);
        summary.count = weight;
        summary
    }

    fn observe_earliest(&mut self, row: EventDataRow, order: RowOrder, from: i64, weight: f64) {
        self.observe_values(row.timestamp, from, weight);
        if order < self.representative_order {
            self.representative = row;
            self.representative_order = order;
        }
    }

    fn observe_first(&mut self, row: &EventDataRow, from: i64, weight: f64) {
        self.observe_values(row.timestamp, from, weight);
    }

    fn observe_physical(&mut self, row: EventDataRow, order: RowOrder, from: i64, weight: f64) {
        self.observe_values(row.timestamp, from, weight);
        if order.encounter < self.representative_order.encounter {
            self.representative = row;
            self.representative_order = order;
        }
    }

    fn observe_scored(
        &mut self,
        row: EventDataRow,
        order: RowOrder,
        from: i64,
        weight: f64,
        score: f64,
    ) {
        self.observe_values(row.timestamp, from, weight);
        let score_order = score.total_cmp(&self.representative_score);
        if score_order.is_gt() || (score_order.is_eq() && order < self.representative_order) {
            self.representative = row;
            self.representative_order = order;
            self.representative_score = score;
        }
    }

    fn observe_values(&mut self, timestamp: i64, from: i64, weight: f64) {
        self.first_ts = self.first_ts.min(timestamp);
        self.last_ts = self.last_ts.max(timestamp);
        self.count += weight;
        self.add_minute(timestamp, from, weight);
    }

    fn add_minute(&mut self, timestamp: i64, from: i64, weight: f64) {
        if let Some(delta) = timestamp.checked_sub(from) {
            let bucket = delta.div_euclid(MINUTE_MICROS);
            if let Ok(bucket) = usize::try_from(bucket)
                && let Some(slot) = self.minutes.get_mut(bucket)
            {
                *slot += weight;
            }
        }
    }

    fn finish(
        self,
        key: String,
        source: EventSource,
        count: Option<f64>,
        tier: EventTier,
        label: Option<String>,
        stat: EventStat,
    ) -> EventGroup {
        let representative_ts = self.representative.timestamp;
        let detail_locator = super::row_key::detail_locator(
            source.as_str(),
            self.representative.segment_id,
            representative_ts,
            self.representative.type_id,
            self.representative.row_ordinal,
            self.representative.identity,
        );
        EventGroup {
            key,
            section: source.as_str().to_owned(),
            tier,
            label,
            count: count.unwrap_or(self.count),
            first_ts: self.first_ts,
            last_ts: self.last_ts,
            representative_ts,
            minutes: self.minutes,
            stat,
            detail_locator,
        }
    }
}

enum SharedText {
    Value(String),
    Mixed,
}

impl SharedText {
    fn new(value: Option<String>) -> Self {
        match value {
            Some(value) if !value.is_empty() => Self::Value(value),
            _ => Self::Mixed,
        }
    }

    fn observe(&mut self, value: Option<&str>) {
        if let Self::Value(shared) = self
            && value != Some(shared.as_str())
        {
            *self = Self::Mixed;
        }
    }

    fn finish(self) -> Option<String> {
        match self {
            Self::Value(value) => Some(value),
            Self::Mixed => None,
        }
    }
}

struct ErrorState {
    summary: Summary,
    database: SharedText,
    username: SharedText,
}

struct SlowState {
    summary: Summary,
    total_ms: f64,
}

struct AutovacuumState {
    summary: Summary,
    runs: usize,
    total_ms: Option<f64>,
    tuples_removed: Option<f64>,
    last_order: RowOrder,
    tuples_dead: Option<f64>,
}

struct CheckpointState {
    summary: Summary,
    starts: usize,
    completes: usize,
    timed: usize,
    max_sync_ms: Option<f64>,
    buffers: Option<f64>,
}

struct WarningState {
    summary: Summary,
    seconds_apart: Option<f64>,
}

#[derive(Clone)]
struct WaitMatch {
    order: RowOrder,
    holders: String,
}

struct LockState {
    summary: Summary,
    waits: usize,
    waiters: HashSet<String>,
    targets: HashMap<String, RowOrder>,
    max_ms: Option<f64>,
}

struct LifecycleState {
    index: usize,
    summary: Summary,
    lifecycle: f64,
    pid: Option<f64>,
    signal: Option<f64>,
    mode: Option<String>,
}

struct PgbouncerState {
    summary: Summary,
    database: SharedText,
    username: SharedText,
    host: SharedText,
    source_file: SharedText,
}

pub(super) struct EventGroups {
    from: i64,
    encounter: u64,
    errors: HashMap<String, ErrorState>,
    slow: HashMap<String, SlowState>,
    autovacuum: HashMap<String, AutovacuumState>,
    checkpoints: Option<CheckpointState>,
    checkpoint_warnings: Option<WarningState>,
    locks: HashMap<String, LockState>,
    latest_waits: HashMap<String, WaitMatch>,
    standalone_acquired: Option<LockState>,
    lifecycle: Vec<LifecycleState>,
    pgbouncer: HashMap<String, PgbouncerState>,
}

impl EventGroups {
    pub(super) fn new(from: i64) -> Self {
        Self {
            from,
            encounter: 0,
            errors: HashMap::new(),
            slow: HashMap::new(),
            autovacuum: HashMap::new(),
            checkpoints: None,
            checkpoint_warnings: None,
            locks: HashMap::new(),
            latest_waits: HashMap::new(),
            standalone_acquired: None,
            lifecycle: Vec::new(),
            pgbouncer: HashMap::new(),
        }
    }

    pub(super) fn observe(&mut self, source: EventSource, row: EventDataRow) {
        let order = RowOrder {
            timestamp: row.timestamp,
            encounter: self.encounter,
        };
        self.encounter = self.encounter.saturating_add(1);
        self.observe_at(source, row, order);
    }

    fn observe_at(&mut self, source: EventSource, row: EventDataRow, order: RowOrder) {
        match source {
            EventSource::Errors => self.observe_error(row, order),
            EventSource::SlowQueries => self.observe_slow(row, order),
            EventSource::Autovacuum => self.observe_autovacuum(row, order),
            EventSource::Checkpoints => self.observe_checkpoint(row, order),
            EventSource::LockWaits => self.observe_lock(row, order),
            EventSource::Lifecycle => self.observe_lifecycle(row, order),
            EventSource::Pgbouncer => self.observe_pgbouncer(row, order),
            EventSource::TempFiles => {}
        }
    }

    fn observe_error(&mut self, row: EventDataRow, order: RowOrder) {
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            text(&row, "severity").unwrap_or_default(),
            text(&row, "category").unwrap_or_default(),
            text(&row, "pattern").unwrap_or_default()
        );
        let weight = number(&row, "count").unwrap_or(1.0);
        match self.errors.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                let database = SharedText::new(text(&row, "database"));
                let username = SharedText::new(text(&row, "username"));
                entry.insert(ErrorState {
                    summary: Summary::new(row, order, self.from, weight, 0.0),
                    database,
                    username,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                let database = text(&row, "database");
                let username = text(&row, "username");
                state.database.observe(database.as_deref());
                state.username.observe(username.as_deref());
                state
                    .summary
                    .observe_earliest(row, order, self.from, weight);
            }
        }
    }

    fn observe_slow(&mut self, row: EventDataRow, order: RowOrder) {
        let key = text(&row, "pattern").unwrap_or_default();
        let weight = number(&row, "count").unwrap_or(1.0);
        let score = number(&row, "max_duration_ms").unwrap_or(0.0);
        let duration = number(&row, "total_duration_ms").unwrap_or(0.0);
        match self.slow.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(SlowState {
                    summary: Summary::new(row, order, self.from, weight, score),
                    total_ms: duration,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                state.total_ms += duration;
                state
                    .summary
                    .observe_scored(row, order, self.from, weight, score);
            }
        }
    }

    fn observe_autovacuum(&mut self, row: EventDataRow, order: RowOrder) {
        let key = format!(
            "{}\u{1f}{}",
            text(&row, "kind").unwrap_or_default(),
            text(&row, "relation").unwrap_or_default()
        );
        let elapsed_ms = number(&row, "elapsed_ms");
        let tuples_removed = number(&row, "tuples_removed");
        let tuples_dead = number(&row, "tuples_dead_not_removable");
        match self.autovacuum.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(AutovacuumState {
                    summary: Summary::new(row, order, self.from, 1.0, 0.0),
                    runs: 1,
                    total_ms: elapsed_ms,
                    tuples_removed,
                    last_order: order,
                    tuples_dead,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                state.runs += 1;
                add_optional(&mut state.total_ms, elapsed_ms);
                add_optional(&mut state.tuples_removed, tuples_removed);
                if order >= state.last_order {
                    state.last_order = order;
                    state.tuples_dead = tuples_dead;
                }
                state.summary.observe_earliest(row, order, self.from, 1.0);
            }
        }
    }

    fn observe_checkpoint(&mut self, row: EventDataRow, order: RowOrder) {
        let phase = number(&row, "phase");
        if phase == Some(2.0) {
            let seconds_apart = number(&row, "seconds_apart");
            if let Some(state) = &mut self.checkpoint_warnings {
                state.seconds_apart = min_optional(state.seconds_apart, seconds_apart);
                state.summary.observe_first(&row, self.from, 1.0);
            } else {
                self.checkpoint_warnings = Some(WarningState {
                    summary: Summary::new(row, order, self.from, 1.0, 0.0),
                    seconds_apart,
                });
            }
            return;
        }

        let starts = usize::from(phase == Some(0.0));
        let completes = usize::from(phase == Some(1.0));
        let timed = usize::from(
            phase == Some(0.0)
                && text(&row, "reason").is_some_and(|reason| reason.contains("time")),
        );
        let sync_ms = (phase == Some(1.0))
            .then(|| number(&row, "sync_ms"))
            .flatten();
        let buffers = (phase == Some(1.0))
            .then(|| number(&row, "buffers_written"))
            .flatten();
        if let Some(state) = &mut self.checkpoints {
            state.starts += starts;
            state.completes += completes;
            state.timed += timed;
            state.max_sync_ms = max_optional(state.max_sync_ms, sync_ms);
            add_optional(&mut state.buffers, buffers);
            state.summary.observe_first(&row, self.from, 1.0);
        } else {
            self.checkpoints = Some(CheckpointState {
                summary: Summary::new(row, order, self.from, 1.0, 0.0),
                starts,
                completes,
                timed,
                max_sync_ms: sync_ms,
                buffers,
            });
        }
    }

    fn observe_lock(&mut self, row: EventDataRow, order: RowOrder) {
        if number(&row, "kind") == Some(0.0) {
            let holders = text(&row, "holding_pids").unwrap_or_default();
            let join = lock_join(&row);
            self.observe_waiting_lock(holders.clone(), row, order);
            let candidate = WaitMatch { order, holders };
            match self.latest_waits.entry(join) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(candidate);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if candidate.order > entry.get().order {
                        entry.insert(candidate);
                    }
                }
            }
            return;
        }

        let join = lock_join(&row);
        let holders = self
            .latest_waits
            .get(&join)
            .filter(|candidate| candidate.order <= order)
            .map(|candidate| candidate.holders.clone());
        if let Some(holders) = holders {
            if let Some(state) = self.locks.get_mut(&holders) {
                state.observe(row, order, self.from, LockObservation::MatchedAcquired);
            }
        } else if let Some(state) = &mut self.standalone_acquired {
            state.observe(row, order, self.from, LockObservation::StandaloneAcquired);
        } else {
            self.standalone_acquired = Some(LockState::new(row, order, self.from, false));
        }
    }

    fn observe_waiting_lock(&mut self, holders: String, row: EventDataRow, order: RowOrder) {
        if let Some(state) = self.locks.get_mut(&holders) {
            state.observe(row, order, self.from, LockObservation::Waiting);
        } else {
            self.locks
                .insert(holders, LockState::new(row, order, self.from, true));
        }
    }

    fn observe_lifecycle(&mut self, row: EventDataRow, order: RowOrder) {
        let lifecycle = number(&row, "kind").unwrap_or(0.0);
        let pid = number(&row, "pid");
        let signal = number(&row, "signal");
        let mode = text(&row, "shutdown_mode");
        let index = self.lifecycle.len();
        self.lifecycle.push(LifecycleState {
            index,
            summary: Summary::new(row, order, self.from, 1.0, 0.0),
            lifecycle,
            pid,
            signal,
            mode,
        });
    }

    fn observe_pgbouncer(&mut self, row: EventDataRow, order: RowOrder) {
        let key = format!(
            "{}\u{1f}{}",
            text(&row, "level").unwrap_or_default(),
            text(&row, "text").unwrap_or_default()
        );
        match self.pgbouncer.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                let database = SharedText::new(text(&row, "database"));
                let username = SharedText::new(text(&row, "username"));
                let host = SharedText::new(text(&row, "host"));
                let source_file = SharedText::new(text(&row, "source_file"));
                entry.insert(PgbouncerState {
                    summary: Summary::new(row, order, self.from, 1.0, 0.0),
                    database,
                    username,
                    host,
                    source_file,
                });
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                let database = text(&row, "database");
                let username = text(&row, "username");
                let host = text(&row, "host");
                let source_file = text(&row, "source_file");
                state.database.observe(database.as_deref());
                state.username.observe(username.as_deref());
                state.host.observe(host.as_deref());
                state.source_file.observe(source_file.as_deref());
                state.summary.observe_earliest(row, order, self.from, 1.0);
            }
        }
    }

    pub(super) fn finish(self, threshold_ms: Option<f64>) -> Result<Vec<EventGroup>, QueryError> {
        let mut entries = Vec::with_capacity(self.retained_rows());
        finish_primary_groups(
            &mut entries,
            self.errors,
            self.slow,
            self.autovacuum,
            threshold_ms,
        );
        finish_other_groups(
            &mut entries,
            self.checkpoints,
            self.checkpoint_warnings,
            self.locks,
            self.standalone_acquired,
            self.lifecycle,
            self.pgbouncer,
        );

        let collator = event_collator()?;
        entries.sort_by(|left, right| {
            tier_order(left.tier)
                .cmp(&tier_order(right.tier))
                .then_with(|| right.count.total_cmp(&left.count))
                .then_with(|| right.last_ts.cmp(&left.last_ts))
                .then_with(|| collator.compare(&left.key, &right.key))
                .then_with(|| left.key.cmp(&right.key))
        });
        for entry in &mut entries {
            if let EventStat::Pgbouncer { level, .. } = &entry.stat {
                let digest = entry
                    .detail_locator
                    .identity
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("missing");
                entry.key = format!("pgbouncer:{level}:{digest}");
            }
        }
        Ok(entries)
    }

    pub(super) fn retained_rows(&self) -> usize {
        self.errors.len()
            + self.slow.len()
            + self.autovacuum.len()
            + usize::from(self.checkpoints.is_some())
            + usize::from(self.checkpoint_warnings.is_some())
            + self.locks.len()
            + usize::from(self.standalone_acquired.is_some())
            + self.lifecycle.len()
            + self.pgbouncer.len()
    }
}

fn finish_primary_groups(
    entries: &mut Vec<EventGroup>,
    errors: HashMap<String, ErrorState>,
    slow: HashMap<String, SlowState>,
    autovacuum: HashMap<String, AutovacuumState>,
    threshold_ms: Option<f64>,
) {
    for (key, state) in errors {
        let severity = number(&state.summary.representative, "severity").unwrap_or(0.0);
        let category = number(&state.summary.representative, "category");
        let sqlstate = text(&state.summary.representative, "sqlstate");
        let label = text(&state.summary.representative, "pattern");
        entries.push(state.summary.finish(
            format!("errors:{key}"),
            EventSource::Errors,
            None,
            error_tier(severity),
            label,
            EventStat::Errors {
                severity,
                category,
                sqlstate,
                database: state.database.finish(),
                username: state.username.finish(),
            },
        ));
    }
    for (key, state) in slow {
        let max_ms = number(&state.summary.representative, "max_duration_ms").unwrap_or(0.0);
        entries.push(state.summary.finish(
            format!("slow:{key}"),
            EventSource::SlowQueries,
            None,
            EventTier::Notable,
            Some(key),
            EventStat::Slow {
                max_ms,
                total_ms: state.total_ms,
                threshold_ms,
            },
        ));
    }
    for (key, state) in autovacuum {
        let analyze = number(&state.summary.representative, "kind") == Some(1.0);
        let label = text(&state.summary.representative, "relation");
        entries.push(state.summary.finish(
            format!("autovacuum:{key}"),
            EventSource::Autovacuum,
            None,
            EventTier::Routine,
            label,
            EventStat::Autovacuum {
                analyze,
                runs: state.runs,
                total_ms: state.total_ms,
                tuples_removed: state.tuples_removed,
                tuples_dead: state.tuples_dead,
            },
        ));
    }
}

fn finish_other_groups(
    entries: &mut Vec<EventGroup>,
    checkpoints: Option<CheckpointState>,
    checkpoint_warnings: Option<WarningState>,
    locks: HashMap<String, LockState>,
    standalone_acquired: Option<LockState>,
    lifecycle: Vec<LifecycleState>,
    pgbouncer: HashMap<String, PgbouncerState>,
) {
    if let Some(state) = checkpoints {
        let count = state.starts.max(state.completes);
        entries.push(state.summary.finish(
            "checkpoints".to_owned(),
            EventSource::Checkpoints,
            Some(count_number(count)),
            EventTier::Routine,
            None,
            EventStat::Checkpoints {
                completes: state.completes,
                timed: state.timed,
                requested: state.starts - state.timed,
                max_sync_ms: state.max_sync_ms,
                buffers: state.buffers,
            },
        ));
    }
    if let Some(state) = checkpoint_warnings {
        entries.push(state.summary.finish(
            "checkpoints:warning".to_owned(),
            EventSource::Checkpoints,
            None,
            EventTier::Notable,
            None,
            EventStat::CheckpointWarning {
                seconds_apart: state.seconds_apart,
            },
        ));
    }
    for (holders, state) in locks {
        entries.push(state.finish(holders, false));
    }
    if let Some(state) = standalone_acquired {
        entries.push(state.finish(String::new(), true));
    }
    for state in lifecycle {
        let row_ordinal = state.summary.representative.row_ordinal;
        entries.push(state.summary.finish(
            format!("lifecycle:{}:{row_ordinal}", state.index),
            EventSource::Lifecycle,
            None,
            lifecycle_tier(state.lifecycle),
            None,
            EventStat::Lifecycle {
                lifecycle: state.lifecycle,
                pid: state.pid,
                signal: state.signal,
                mode: state.mode,
            },
        ));
    }
    for (key, state) in pgbouncer {
        let level = number(&state.summary.representative, "level").unwrap_or(3.0);
        let label = text(&state.summary.representative, "text");
        entries.push(state.summary.finish(
            format!("pgbouncer:{key}"),
            EventSource::Pgbouncer,
            None,
            pgbouncer_tier(level),
            label,
            EventStat::Pgbouncer {
                level,
                database: state.database.finish(),
                username: state.username.finish(),
                host: state.host.finish(),
                source_file: state.source_file.finish(),
            },
        ));
    }
}

#[derive(Clone, Copy)]
enum LockObservation {
    Waiting,
    MatchedAcquired,
    StandaloneAcquired,
}

impl LockState {
    fn new(row: EventDataRow, order: RowOrder, from: i64, waiting: bool) -> Self {
        let mut waiters = HashSet::new();
        waiters.insert(text(&row, "pid").unwrap_or_default());
        let mut targets = HashMap::new();
        if let Some(target) = text(&row, "lock_target")
            && !target.is_empty()
        {
            targets.insert(
                target,
                if waiting {
                    order
                } else {
                    RowOrder {
                        timestamp: 0,
                        encounter: order.encounter,
                    }
                },
            );
        }
        Self {
            max_ms: number(&row, "duration_ms"),
            summary: Summary::new(row, order, from, 1.0, 0.0),
            waits: usize::from(waiting),
            waiters,
            targets,
        }
    }

    fn observe(
        &mut self,
        row: EventDataRow,
        order: RowOrder,
        from: i64,
        observation: LockObservation,
    ) {
        self.waits += usize::from(matches!(observation, LockObservation::Waiting));
        self.waiters.insert(text(&row, "pid").unwrap_or_default());
        if let Some(target) = text(&row, "lock_target")
            && !target.is_empty()
        {
            let target_order = match observation {
                LockObservation::Waiting | LockObservation::MatchedAcquired => order,
                LockObservation::StandaloneAcquired => RowOrder {
                    timestamp: 0,
                    encounter: order.encounter,
                },
            };
            self.targets
                .entry(target)
                .and_modify(|existing| *existing = (*existing).min(target_order))
                .or_insert(target_order);
        }
        self.max_ms = max_optional(self.max_ms, number(&row, "duration_ms"));
        match observation {
            LockObservation::Waiting | LockObservation::MatchedAcquired => {
                self.summary.observe_earliest(row, order, from, 1.0);
            }
            LockObservation::StandaloneAcquired => {
                self.summary.observe_physical(row, order, from, 1.0);
            }
        }
    }

    fn finish(self, holders: String, acquired: bool) -> EventGroup {
        let mut targets = self.targets.into_iter().collect::<Vec<_>>();
        targets.sort_by_key(|(_target, order)| *order);
        let targets = targets.into_iter().map(|(target, _order)| target).collect();
        let count = if acquired {
            self.summary.count
        } else {
            count_number(self.waits.max(1))
        };
        self.summary.finish(
            format!("locks:{}", if acquired { "acquired" } else { &holders }),
            EventSource::LockWaits,
            Some(count),
            EventTier::Notable,
            None,
            EventStat::Locks {
                holders: (!holders.is_empty()).then_some(holders),
                acquired,
                waiters: self.waiters.len(),
                max_ms: self.max_ms,
                targets,
            },
        )
    }
}

fn lock_join(row: &EventDataRow) -> String {
    format!(
        "{}\u{1f}{}",
        text(row, "pid").unwrap_or_default(),
        text(row, "lock_target").unwrap_or_default()
    )
}

fn add_optional(total: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0.0) + value);
    }
}

fn max_optional(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left.total_cmp(&right).is_lt() {
            right
        } else {
            left
        }),
        (left, right) => left.or(right),
    }
}

fn min_optional(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left.total_cmp(&right).is_gt() {
            right
        } else {
            left
        }),
        (left, right) => left.or(right),
    }
}

pub(super) fn event_collator() -> Result<CollatorBorrowed<'static>, QueryError> {
    let mut options = CollatorOptions::default();
    options.strength = Some(Strength::Tertiary);
    options.alternate_handling = Some(AlternateHandling::NonIgnorable);
    options.case_level = Some(CaseLevel::Off);
    Collator::try_new(locale!("en-US").into(), options).map_err(|error| {
        QueryError::Unreadable(Box::new(std::io::Error::other(format!(
            "create en-US event-key collator: {error}"
        ))))
    })
}

fn text(row: &EventDataRow, field: &str) -> Option<String> {
    row.values.get(field).and_then(raw_text)
}

fn raw_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Object(object) if object.get("representation") == Some(&json!("text")) => object
            .get("stored_text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        Value::Object(object) if object.get("representation") == Some(&json!("bytes")) => object
            .get("stored_base64")
            .or_else(|| object.get("base64"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        value => Some(value.to_string()),
    }
}

fn number(row: &EventDataRow, field: &str) -> Option<f64> {
    let value = row.values.get(field)?;
    let parsed = match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) if !value.trim().is_empty() => value.parse().ok(),
        _ => None,
    }?;
    parsed.is_finite().then_some(parsed)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "the wire contract is a JavaScript number and event counts use that representation"
)]
const fn count_number(value: usize) -> f64 {
    value as f64
}

#[derive(Default)]
pub(super) struct SlowThreshold {
    latest: Option<(RowOrder, Option<f64>)>,
    encounter: u64,
}

impl SlowThreshold {
    pub(super) fn observe(&mut self, row: &EventDataRow) {
        let order = RowOrder {
            timestamp: row.timestamp,
            encounter: self.encounter,
        };
        self.encounter = self.encounter.saturating_add(1);
        if row.values.get("name").and_then(raw_text).as_deref()
            != Some("log_min_duration_statement")
        {
            return;
        }
        let setting = row
            .values
            .get("setting")
            .and_then(raw_text)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(
                |setting| match row.values.get("unit").and_then(raw_text).as_deref() {
                    Some("s") => setting * 1_000.0,
                    Some("min") => setting * 60_000.0,
                    _ => setting,
                },
            );
        if self
            .latest
            .as_ref()
            .is_none_or(|(latest, _value)| order.timestamp > latest.timestamp)
        {
            self.latest = Some((order, setting));
        }
    }

    pub(super) fn finish(self) -> Option<f64> {
        self.latest.and_then(|(_order, value)| value)
    }
}

const fn tier_order(tier: EventTier) -> u8 {
    match tier {
        EventTier::Critical => 0,
        EventTier::Notable => 1,
        EventTier::Routine => 2,
    }
}

fn error_tier(code: f64) -> EventTier {
    match code {
        1.0 | 2.0 => EventTier::Critical,
        3.0 | 4.0 => EventTier::Routine,
        _ => EventTier::Notable,
    }
}

fn lifecycle_tier(code: f64) -> EventTier {
    match code {
        0.0 => EventTier::Critical,
        _ => EventTier::Notable,
    }
}

fn pgbouncer_tier(code: f64) -> EventTier {
    match code {
        0.0 => EventTier::Critical,
        1.0 | 2.0 => EventTier::Notable,
        _ => EventTier::Routine,
    }
}
