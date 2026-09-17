//! Relation metric folds with structural absence and reset handling.

use std::collections::BTreeMap;

use kronika_reader::{Cell, Dictionary, Row};

use super::{
    Availability, BoolAggregate, CounterReadings, GroupKey, Input, MaximumAggregate, Metric,
    OrderedNumber, RateAggregate, RateValue, RelationAggregate, RelationKind, RelationSource,
    TimestampAggregate, TimestampObservation, add_ordered, bool_cell, counter_input, gauge_input,
    integer_cell, number_is_zero, text_cell,
};

use crate::hour::relation::fields::{
    INDEX_FLAGS, INDEX_GAUGES, INDEX_RATES, INDEX_TIMESTAMPS, TABLE_GAUGES, TABLE_MAXIMA,
    TABLE_RATES, TABLE_STRUCTURAL_GAUGES, TABLE_STRUCTURAL_RATES, TABLE_TIMESTAMPS,
};
use crate::projection::Plan;
use crate::{QueryError, RelationGroup};

impl Availability {
    pub(super) fn add(&mut self, input: Input) {
        match (*self, input) {
            (Self::Unavailable, _) | (_, Input::Unavailable) => *self = Self::Unavailable,
            (Self::Empty | Self::Value(_), Input::Neutral) => {}
            (Self::Empty, Input::Value(value)) => *self = Self::Value(value),
            (Self::Value(left), Input::Value(right)) => {
                *self = add_ordered(Some(left), right).map_or(Self::Unavailable, Self::Value);
            }
        }
    }

    pub(super) const fn value(self) -> Option<OrderedNumber> {
        match self {
            Self::Value(value) => Some(value),
            Self::Empty | Self::Unavailable => None,
        }
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.add(match other {
            Self::Empty => Input::Neutral,
            Self::Value(value) => Input::Value(value),
            Self::Unavailable => Input::Unavailable,
        });
    }
}

impl RateValue {
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval of 2^52 microseconds is 142 years"
    )]
    pub(super) fn from_delta(delta: OrderedNumber, elapsed: i64) -> Option<Self> {
        let denominator = u128::try_from(elapsed).ok().filter(|value| *value > 0)?;
        match delta {
            OrderedNumber::Integer(value) => {
                let numerator = u128::try_from(value).ok()?;
                Some(Self::exact(numerator, denominator))
            }
            OrderedNumber::Float(value) => {
                let rate = value / elapsed as f64;
                rate.is_finite().then_some(Self::Float(rate))
            }
        }
    }

    pub(super) const fn exact(numerator: u128, denominator: u128) -> Self {
        let divisor = greatest_common_divisor(numerator, denominator);
        Self::Exact {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        }
    }

    pub(super) fn add(self, other: Self) -> Option<Self> {
        match (self, other) {
            (
                Self::Exact {
                    numerator: left_numerator,
                    denominator: left_denominator,
                },
                Self::Exact {
                    numerator: right_numerator,
                    denominator: right_denominator,
                },
            ) => {
                let common = greatest_common_divisor(left_denominator, right_denominator);
                let left_scale = right_denominator / common;
                let right_scale = left_denominator / common;
                let exact = left_numerator
                    .checked_mul(left_scale)
                    .and_then(|left| {
                        right_numerator
                            .checked_mul(right_scale)
                            .and_then(|right| left.checked_add(right))
                    })
                    .zip(left_denominator.checked_mul(left_scale))
                    .map(|(numerator, denominator)| Self::exact(numerator, denominator));
                exact.or_else(|| Self::float_sum(self, other))
            }
            _ => Self::float_sum(self, other),
        }
    }

    pub(super) fn float_sum(self, other: Self) -> Option<Self> {
        let value = self.per_microsecond() + other.per_microsecond();
        value.is_finite().then_some(Self::Float(value))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "floating output is produced only after exact rational accumulation"
    )]
    pub(super) fn per_microsecond(self) -> f64 {
        match self {
            Self::Exact {
                numerator,
                denominator,
            } => numerator as f64 / denominator as f64,
            Self::Float(value) => value,
        }
    }

    pub(super) fn per_second(self) -> f64 {
        self.per_microsecond() * 1_000_000.0
    }
}

const fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

impl RateAggregate {
    pub(super) fn add(&mut self, input: Input, elapsed: Option<i64>) {
        if self.unavailable {
            return;
        }
        match input {
            Input::Neutral => {}
            Input::Unavailable => self.unavailable = true,
            Input::Value(delta) => {
                let Some(value) = elapsed.and_then(|elapsed| RateValue::from_delta(delta, elapsed))
                else {
                    self.unavailable = true;
                    return;
                };
                self.value = self.value.map_or(Some(value), |known| known.add(value));
                if self.value.is_none() {
                    self.unavailable = true;
                }
            }
        }
    }

    pub(super) fn metric(self) -> Option<Metric> {
        if self.unavailable {
            None
        } else {
            self.value.map(Metric::rate)
        }
    }

    pub(super) const fn value(self) -> Option<RateValue> {
        if self.unavailable { None } else { self.value }
    }

    pub(super) fn merge(&mut self, other: Self) {
        if self.unavailable || other.unavailable {
            self.unavailable = true;
            self.value = None;
            return;
        }
        if let Some(value) = other.value {
            self.value = self.value.map_or(Some(value), |known| known.add(value));
            if self.value.is_none() {
                self.unavailable = true;
            }
        }
    }
}

impl MaximumAggregate {
    pub(super) fn add(&mut self, present: bool, stored: Option<&Cell>) {
        if !present {
            self.unavailable = true;
            return;
        }
        match stored {
            Some(Cell::Null) => {}
            Some(cell) => match integer_cell(Some(cell)) {
                Some(value) => {
                    self.maximum = Some(self.maximum.map_or(value, |known| known.max(value)));
                }
                None => self.unavailable = true,
            },
            None => self.unavailable = true,
        }
    }

    pub(super) const fn exact(self) -> Option<i128> {
        if self.unavailable { None } else { self.maximum }
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.unavailable |= other.unavailable;
        if let Some(value) = other.maximum {
            self.maximum = Some(self.maximum.map_or(value, |known| known.max(value)));
        }
    }
}

impl TimestampAggregate {
    pub(super) fn add(&mut self, observation: TimestampObservation<'_>) {
        let stored = match observation {
            TimestampObservation::Unavailable => {
                self.unavailable = true;
                return;
            }
            TimestampObservation::NotApplicable => return,
            TimestampObservation::Stored(stored) => stored,
        };
        self.applicable = self.applicable.saturating_add(1);
        match stored {
            Some(Cell::Ts(value)) => {
                self.oldest = Some(self.oldest.map_or(*value, |oldest| oldest.min(*value)));
                self.latest = Some(self.latest.map_or(*value, |latest| latest.max(*value)));
            }
            Some(Cell::Null) => self.never = self.never.saturating_add(1),
            _ => self.unavailable = true,
        }
    }

    pub(super) const fn exact(self) -> bool {
        self.applicable > 0 && !self.unavailable
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.applicable = self.applicable.saturating_add(other.applicable);
        self.unavailable |= other.unavailable;
        self.oldest = match (self.oldest, other.oldest) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.latest = match (self.latest, other.latest) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        self.never = self.never.saturating_add(other.never);
    }
}

impl BoolAggregate {
    pub(super) fn add(&mut self, stored: Option<&Cell>) {
        match stored {
            Some(Cell::Bool(value)) => {
                self.known = self.known.saturating_add(1);
                self.truthy = self.truthy.saturating_add(u64::from(*value));
            }
            _ => self.unavailable = true,
        }
    }

    pub(super) fn add_scan(&mut self, input: Input) {
        match input {
            Input::Value(value) => self.add(Some(&Cell::Bool(number_is_zero(value)))),
            Input::Neutral | Input::Unavailable => self.unavailable = true,
        }
    }

    pub(super) const fn exact(self) -> bool {
        self.known > 0 && !self.unavailable
    }

    pub(super) const fn merge(&mut self, other: Self) {
        self.known = self.known.saturating_add(other.known);
        self.truthy = self.truthy.saturating_add(other.truthy);
        self.unavailable |= other.unavailable;
    }
}

impl std::fmt::Debug for RelationAggregate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelationAggregate")
            .field("key", &self.key)
            .field("source", &self.source)
            .field("sample_from", &self.from)
            .field("sample_to", &self.to)
            .finish_non_exhaustive()
    }
}

impl RelationAggregate {
    /// Start an empty reducer for one stable grouping key.
    #[must_use]
    pub fn new(key: GroupKey, source: RelationSource) -> Self {
        Self {
            key,
            count: 0,
            source,
            from: None,
            to: None,
            rates: BTreeMap::new(),
            gauges: BTreeMap::new(),
            maxima: BTreeMap::new(),
            timestamps: BTreeMap::new(),
            flags: BTreeMap::new(),
            texts: BTreeMap::new(),
            tablespace_label_timestamp: None,
            identifiers: BTreeMap::new(),
            no_scans: BoolAggregate::default(),
            state_severity: None,
        }
    }

    /// Stable grouping key accumulated by this reducer.
    #[must_use]
    pub const fn key(&self) -> &GroupKey {
        &self.key
    }

    /// Deterministic source coordinate retained for cursor tie-breaking.
    #[must_use]
    pub const fn source(&self) -> RelationSource {
        self.source
    }

    /// Earliest contributing sample boundary.
    #[must_use]
    pub const fn sample_from(&self) -> Option<i64> {
        self.from
    }

    /// Latest contributing sample boundary.
    #[must_use]
    pub const fn sample_to(&self) -> Option<i64> {
        self.to
    }

    /// Text value retained from physical rows, when present.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        self.texts.get(name).map(String::as_str)
    }

    /// Add one physical relation row to this reducer.
    ///
    /// # Errors
    ///
    /// Returns a query decoding error when a referenced dictionary value is invalid.
    #[expect(
        clippy::too_many_arguments,
        reason = "one physical object carries its validated interval and source coordinates"
    )]
    pub fn add(
        &mut self,
        kind: RelationKind,
        plan: &Plan,
        row: &Row,
        before: Option<&BTreeMap<&'static str, Cell>>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
        source: RelationSource,
    ) -> Result<(), QueryError> {
        self.count = self.count.saturating_add(1);
        self.source = self.source.min(source);
        self.to = Some(
            self.to
                .map_or(source.timestamp, |to| to.max(source.timestamp)),
        );
        if let Some(from) = elapsed.and_then(|elapsed| source.timestamp.checked_sub(elapsed)) {
            self.from = Some(self.from.map_or(from, |oldest| oldest.min(from)));
        }
        if self.key.is_tablespace()
            && let Some(label) = text_cell(row.get("tablespace"), dictionary)?
        {
            let replace = self.tablespace_label_timestamp.is_none_or(|timestamp| {
                source.timestamp > timestamp
                    || source.timestamp == timestamp
                        && self
                            .texts
                            .get("tablespace")
                            .is_none_or(|current| label.as_bytes() < current.as_bytes())
            });
            if replace {
                self.tablespace_label_timestamp = Some(source.timestamp);
                self.texts.insert("tablespace", label);
            }
        }
        match kind {
            RelationKind::Tables => self.add_table(plan, row, before, elapsed, dictionary)?,
            RelationKind::Indexes => self.add_index(plan, row, before, elapsed, dictionary)?,
        }
        Ok(())
    }

    pub(super) fn merge(&mut self, other: &Self) {
        self.count = self.count.saturating_add(other.count);
        self.source = self.source.min(other.source);
        self.from = match (self.from, other.from) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.to = match (self.to, other.to) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        for (&name, rate) in &other.rates {
            self.rates.entry(name).or_default().merge(*rate);
        }
        for (&name, gauge) in &other.gauges {
            self.gauges.entry(name).or_default().merge(*gauge);
        }
        for (&name, maximum) in &other.maxima {
            self.maxima.entry(name).or_default().merge(*maximum);
        }
        for (&name, timestamp) in &other.timestamps {
            self.timestamps.entry(name).or_default().merge(*timestamp);
        }
        for (&name, flag) in &other.flags {
            self.flags.entry(name).or_default().merge(*flag);
        }
        self.no_scans.merge(other.no_scans);
        self.state_severity = match (self.state_severity, other.state_severity) {
            (Some(left), Some(right)) => Some(left.max(right)),
            _ => None,
        };
        if let Some(timestamp) = other.tablespace_label_timestamp
            && let Some(label) = other.texts.get("tablespace")
        {
            self.update_tablespace_label(timestamp, label.clone());
        }
    }

    pub(super) fn update_tablespace_label(&mut self, timestamp: i64, label: String) {
        let replace = self.tablespace_label_timestamp.is_none_or(|known| {
            timestamp > known
                || timestamp == known
                    && self
                        .texts
                        .get("tablespace")
                        .is_none_or(|current| label.as_bytes() < current.as_bytes())
        });
        if replace {
            self.tablespace_label_timestamp = Some(timestamp);
            self.texts.insert("tablespace", label);
        }
    }

    pub(super) fn add_table(
        &mut self,
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
    ) -> Result<(), QueryError> {
        for &name in TABLE_RATES {
            let structural = TABLE_STRUCTURAL_RATES.contains(&name);
            self.rates
                .entry(name)
                .or_default()
                .add(counter_input(plan, row, before, name, structural), elapsed);
        }
        for &name in TABLE_GAUGES {
            let structural = TABLE_STRUCTURAL_GAUGES.contains(&name);
            let structural_na = structural && matches!(row.get("toast_bytes"), Some(Cell::Null));
            let mut input = gauge_input(plan, row, name, structural_na);
            if name == "reltuples" && matches!(input, Input::Value(OrderedNumber::Integer(-1))) {
                input = Input::Unavailable;
            }
            self.gauges.entry(name).or_default().add(input);
        }
        for &name in TABLE_MAXIMA {
            self.maxima
                .entry(name)
                .or_default()
                .add(plan.contract.column(name).is_some(), row.get(name));
        }
        for &name in TABLE_TIMESTAMPS {
            let structural_na = name == "toast_last_autovacuum"
                && matches!(row.get("toast_bytes"), Some(Cell::Null));
            let observation = if plan.contract.column(name).is_none() {
                TimestampObservation::Unavailable
            } else if structural_na {
                TimestampObservation::NotApplicable
            } else {
                TimestampObservation::Stored(row.get(name))
            };
            self.timestamps.entry(name).or_default().add(observation);
        }
        for name in ["relname", "tablespace"] {
            if let Some(value) = text_cell(row.get(name), dictionary)? {
                self.texts.entry(name).or_insert(value);
            }
        }
        Ok(())
    }

    pub(super) fn add_index(
        &mut self,
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
    ) -> Result<(), QueryError> {
        let scan_input = counter_input(plan, row, before, "idx_scan", false);
        for &name in INDEX_RATES {
            let input = if name == "idx_scan" {
                scan_input
            } else {
                counter_input(plan, row, before, name, false)
            };
            self.rates.entry(name).or_default().add(input, elapsed);
        }
        for &name in INDEX_GAUGES {
            self.gauges
                .entry(name)
                .or_default()
                .add(gauge_input(plan, row, name, false));
        }
        for &name in INDEX_TIMESTAMPS {
            let observation = if plan.contract.column(name).is_some() {
                TimestampObservation::Stored(row.get(name))
            } else {
                TimestampObservation::Unavailable
            };
            self.timestamps.entry(name).or_default().add(observation);
        }
        for &name in INDEX_FLAGS {
            self.flags.entry(name).or_default().add(row.get(name));
        }
        self.no_scans.add_scan(scan_input);
        let valid = bool_cell(row.get("indisvalid"));
        let ready = bool_cell(row.get("indisready"));
        let severity = match (valid, ready) {
            (Some(false), _) => Some(2),
            (Some(true), Some(false)) => Some(1),
            (Some(true), Some(true)) => Some(0),
            _ => None,
        };
        self.state_severity = match (self.state_severity, severity) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, Some(value)) if self.count == 1 => Some(value),
            _ => None,
        };
        for name in [
            "indexrelname",
            "relname",
            "tablespace",
            "amname",
            "indexdef",
        ] {
            if let Some(value) = text_cell(row.get(name), dictionary)? {
                self.texts.entry(name).or_insert(value);
            }
        }
        if let Some(value) = integer_cell(row.get("relid")) {
            self.identifiers.entry("relid").or_insert(value);
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the fixed relation contract keeps every audited reducer in one exhaustive match"
    )]
    /// Calculate one public semantic metric from the accumulated physical inputs.
    #[must_use]
    pub fn metric(&self, kind: RelationKind, group: RelationGroup, name: &str) -> Option<Metric> {
        if name == kind.count_field() {
            return Some(Metric::integer(i128::from(self.count)));
        }
        if let Some(rate) = self.rates.get(name).copied() {
            return rate.metric();
        }
        if let Some(gauge) = self.gauges.get(name).copied() {
            return gauge.value().map(Metric::number);
        }
        if let Some(maximum) = self
            .maxima
            .get(name)
            .copied()
            .and_then(MaximumAggregate::exact)
        {
            return Some(Metric::integer(maximum));
        }
        if let Some(value) = self.texts.get(name) {
            return (group == RelationGroup::Object
                || group == RelationGroup::Tablespace && name == "tablespace")
                .then(|| Metric::text(value.clone()));
        }
        if let Some(value) = self.identifiers.get(name) {
            return (group == RelationGroup::Object).then_some(Metric::integer(*value));
        }
        match (kind, name) {
            (RelationKind::Tables, "sequential_share_pct") => {
                self.ratio_neutral(&["seq_scan"], &["seq_scan", "idx_scan"], 100.0)
            }
            (RelationKind::Tables, "tuple_throughput") => self
                .rate_sum_neutral(&["seq_tup_read", "idx_tup_fetch"])
                .map(Metric::rate),
            (RelationKind::Tables, "seq_tuples_per_scan") => {
                self.ratio(&["seq_tup_read"], &["seq_scan"], 1.0)
            }
            (RelationKind::Tables, "idx_tuples_per_scan")
            | (RelationKind::Indexes, "fetches_per_scan") => {
                self.ratio(&["idx_tup_fetch"], &["idx_scan"], 1.0)
            }
            (RelationKind::Tables, "last_seq_scan_never") if group == RelationGroup::Object => {
                self.never("last_seq_scan")
            }
            (RelationKind::Tables | RelationKind::Indexes, "last_idx_scan_never")
                if group == RelationGroup::Object =>
            {
                self.never("last_idx_scan")
            }
            (RelationKind::Tables, "dml_total") => self
                .rate_sum(&["n_tup_ins", "n_tup_upd", "n_tup_del"])
                .map(Metric::rate),
            (RelationKind::Tables, "insert_share_pct") => self.ratio(
                &["n_tup_ins"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "update_share_pct") => self.ratio(
                &["n_tup_upd"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "delete_share_pct") => self.ratio(
                &["n_tup_del"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "dead_pct") => {
                self.gauge_ratio(&["n_dead_tup"], &["n_live_tup", "n_dead_tup"], 100.0)
            }
            (RelationKind::Tables, "hot_pct") => {
                self.ratio(&["n_tup_hot_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "new_page_pct") => {
                self.ratio(&["n_tup_newpage_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "displayed_storage_bytes") => self
                .gauge_sum_neutral(&["main_fork_bytes", "toast_bytes"])
                .map(Metric::number),
            (RelationKind::Tables, "toast_share_pct") => self.gauge_ratio_neutral(
                &["toast_bytes"],
                &["main_fork_bytes", "toast_bytes"],
                100.0,
            ),
            (RelationKind::Tables, "toast_dead_pct") => self.gauge_ratio(
                &["toast_n_dead_tup"],
                &["toast_n_live_tup", "toast_n_dead_tup"],
                100.0,
            ),
            (RelationKind::Tables, "heap_buffer_hit_pct") => self.ratio(
                &["heap_blks_hit"],
                &["heap_blks_read", "heap_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "index_buffer_hit_pct")
            | (RelationKind::Indexes, "buffer_hit_pct") => {
                self.ratio(&["idx_blks_hit"], &["idx_blks_read", "idx_blks_hit"], 100.0)
            }
            (RelationKind::Tables, "toast_buffer_hit_pct") => self.ratio(
                &["toast_blks_hit"],
                &["toast_blks_read", "toast_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "tidx_buffer_hit_pct") => self.ratio(
                &["tidx_blks_hit"],
                &["tidx_blks_read", "tidx_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "buffer_hit_pct") => self.ratio_neutral(
                &[
                    "heap_blks_hit",
                    "idx_blks_hit",
                    "toast_blks_hit",
                    "tidx_blks_hit",
                ],
                &[
                    "heap_blks_read",
                    "heap_blks_hit",
                    "idx_blks_read",
                    "idx_blks_hit",
                    "toast_blks_read",
                    "toast_blks_hit",
                    "tidx_blks_read",
                    "tidx_blks_hit",
                ],
                100.0,
            ),
            (RelationKind::Tables, "vacuum_mean_ms") => {
                self.ratio(&["total_vacuum_time"], &["vacuum_count"], 1.0)
            }
            (RelationKind::Tables, "autovacuum_mean_ms") => {
                self.ratio(&["total_autovacuum_time"], &["autovacuum_count"], 1.0)
            }
            (RelationKind::Tables, "analyze_mean_ms") => {
                self.ratio(&["total_analyze_time"], &["analyze_count"], 1.0)
            }
            (RelationKind::Tables, "autoanalyze_mean_ms") => {
                self.ratio(&["total_autoanalyze_time"], &["autoanalyze_count"], 1.0)
            }
            (RelationKind::Indexes, "tuples_per_scan") => {
                self.ratio(&["idx_tup_read"], &["idx_scan"], 1.0)
            }
            (RelationKind::Indexes, "no_scans") if group == RelationGroup::Object => self
                .no_scans
                .exact()
                .then_some(Metric::boolean(self.no_scans.truthy == 1)),
            (RelationKind::Indexes, "no_scan_count") => self
                .no_scans
                .exact()
                .then(|| Metric::integer(i128::from(self.no_scans.truthy))),
            (RelationKind::Indexes, "known_scan_count") => self
                .no_scans
                .exact()
                .then(|| Metric::integer(i128::from(self.no_scans.known - self.no_scans.truthy))),
            (RelationKind::Indexes, "state_severity") => self.state_severity.map(Metric::integer),
            (RelationKind::Indexes, "invalid_count") => self.flag_count("indisvalid", false),
            (RelationKind::Indexes, "unready_count") => self.flag_count("indisready", false),
            (RelationKind::Indexes, "unique_count") => self.flag_count("indisunique", true),
            (RelationKind::Indexes, "primary_count") => self.flag_count("indisprimary", true),
            (RelationKind::Indexes, "exclusion_count") => self.flag_count("indisexclusion", true),
            (RelationKind::Indexes, name) if INDEX_FLAGS.contains(&name) => self
                .flags
                .get(name)
                .copied()
                .filter(|flag| flag.exact() && flag.known == 1)
                .map(|flag| Metric::boolean(flag.truthy == 1)),
            _ => timestamp_metric(self, group, name),
        }
    }

    pub(super) fn ratio(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::rate_ratio(
            self.rate_sum(numerator)?,
            self.rate_sum(denominator)?,
            scale,
        ))
    }

    pub(super) fn ratio_neutral(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::rate_ratio(
            self.rate_sum_neutral(numerator)?,
            self.rate_sum_neutral(denominator)?,
            scale,
        ))
    }

    pub(super) fn gauge_ratio(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::ratio(
            sum_values(&self.gauges, numerator)?,
            sum_values(&self.gauges, denominator)?,
            scale,
        ))
    }

    pub(super) fn gauge_ratio_neutral(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::ratio(
            self.gauge_sum_neutral(numerator)?,
            self.gauge_sum_neutral(denominator)?,
            scale,
        ))
    }

    pub(super) fn rate_sum(&self, names: &[&str]) -> Option<RateValue> {
        sum_rates(&self.rates, names)
    }

    pub(super) fn rate_sum_neutral(&self, names: &[&str]) -> Option<RateValue> {
        sum_rates_neutral(&self.rates, names)
    }

    pub(super) fn gauge_sum_neutral(&self, names: &[&str]) -> Option<OrderedNumber> {
        sum_values_neutral(&self.gauges, names)
    }

    pub(super) fn never(&self, name: &str) -> Option<Metric> {
        let timestamp = self.timestamps.get(name).copied()?;
        timestamp
            .exact()
            .then_some(Metric::boolean(timestamp.never == 1))
    }

    pub(super) fn flag_count(&self, name: &str, truthy: bool) -> Option<Metric> {
        let flag = self.flags.get(name).copied()?;
        flag.exact().then(|| {
            let count = if truthy {
                flag.truthy
            } else {
                flag.known - flag.truthy
            };
            Metric::integer(i128::from(count))
        })
    }
}

fn timestamp_metric(
    aggregate: &RelationAggregate,
    group: RelationGroup,
    name: &str,
) -> Option<Metric> {
    if group == RelationGroup::Object {
        let timestamp = aggregate.timestamps.get(name).copied()?;
        return (timestamp.exact() && timestamp.never == 0)
            .then(|| timestamp.latest.map(Metric::timestamp))
            .flatten();
    }
    for suffix in ["_oldest", "_latest", "_never_count"] {
        let Some(base) = name.strip_suffix(suffix) else {
            continue;
        };
        let timestamp = aggregate.timestamps.get(base).copied()?;
        if !timestamp.exact() {
            return None;
        }
        return match suffix {
            "_oldest" => timestamp.oldest.map(Metric::timestamp),
            "_latest" => timestamp.latest.map(Metric::timestamp),
            "_never_count" => Some(Metric::integer(i128::from(timestamp.never))),
            _ => None,
        };
    }
    None
}

fn sum_rates(rates: &BTreeMap<&'static str, RateAggregate>, names: &[&str]) -> Option<RateValue> {
    let mut sum: Option<RateValue> = None;
    for name in names {
        let value = rates.get(name)?.value()?;
        sum = Some(match sum {
            Some(known) => known.add(value)?,
            None => value,
        });
    }
    sum
}

fn sum_rates_neutral(
    rates: &BTreeMap<&'static str, RateAggregate>,
    names: &[&str],
) -> Option<RateValue> {
    let mut sum: Option<RateValue> = None;
    for name in names {
        let rate = rates.get(name)?;
        if rate.unavailable {
            return None;
        }
        let Some(value) = rate.value else {
            continue;
        };
        sum = Some(match sum {
            Some(known) => known.add(value)?,
            None => value,
        });
    }
    sum
}

fn sum_values(
    values: &BTreeMap<&'static str, Availability>,
    names: &[&str],
) -> Option<OrderedNumber> {
    names.iter().try_fold(None, |sum, name| {
        add_ordered(sum, values.get(name)?.value()?).map(Some)
    })?
}

fn sum_values_neutral(
    values: &BTreeMap<&'static str, Availability>,
    names: &[&str],
) -> Option<OrderedNumber> {
    names.iter().try_fold(None, |sum, name| {
        let value = match values.get(name)? {
            Availability::Value(value) => *value,
            Availability::Empty => OrderedNumber::Integer(0),
            Availability::Unavailable => return None,
        };
        add_ordered(sum, value).map(Some)
    })?
}
