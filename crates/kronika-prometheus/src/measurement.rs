//! Conversion of one fetched result set into exposable samples.
//!
//! Mirrors the pgwatch Prometheus sink row rules:
//!
//! * `dbname` label first, `tag_<name>` columns override it and each other by
//!   insertion order (later columns win within one row);
//! * NULL and empty-string cells are dropped — including tag cells, whose
//!   label then stays absent rather than empty;
//! * `epoch_ns` sets the fetch timestamp and is never a value; all rows of
//!   one fetch share the first row's `epoch_ns`;
//! * bool columns give 0/1, non-finite floats are exposed as rendered by
//!   the text format, textual and dropped-type columns are never values
//!   (only tags);
//! * the second row with an identical family-and-label-set is dropped and
//!   counted as an error.

use crate::catalog::{Gauges, NOT_EXPOSED_METRICS};

use crate::expose::Sample;
use crate::typing::{Cell, ColumnKind, parse_value};

/// One result column: name plus kind resolved by the executor from the type OID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Column name as returned by the server.
    pub name: String,
    /// Type classification.
    pub kind: ColumnKind,
}

/// One metric fetch: columns and text-protocol rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    /// Column list in server order.
    pub columns: Vec<Column>,
    /// Rows of text cells aligned with [`QueryResult::columns`].
    pub rows: Vec<Vec<Cell>>,
}

/// Converted output of one metric fetch for one database.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleSet {
    /// Exposition timestamp of every sample in this set, epoch milliseconds.
    pub timestamp_ms: i64,
    /// Samples in row order.
    pub samples: Vec<Sample>,
    /// Rows dropped for duplicate identity or invalid names.
    pub errors: usize,
}

/// Special metric name whose value column is exposed without a suffix.
pub const INSTANCE_UP_METRIC: &str = "instance_up";

/// Builds the `pgwatch_instance_up` sample for a connection check.
///
/// This is the only catalog metric not produced by SQL: the engine connects
/// and runs its ping; a failed SQL fetch of another metric never zeroes it.
#[must_use]
pub fn instance_up_sample_set(dbname: &str, up: bool, timestamp_ms: i64) -> SampleSet {
    SampleSet {
        timestamp_ms,
        samples: vec![Sample {
            family: format!("pgwatch_{INSTANCE_UP_METRIC}"),
            help: INSTANCE_UP_METRIC.to_owned(),
            is_gauge: true,
            labels: vec![("dbname".to_owned(), dbname.to_owned())],
            value: if up { 1.0 } else { 0.0 },
        }],
        errors: 0,
    }
}

/// Converts a fetched [`QueryResult`] into a [`SampleSet`].
///
/// `metric_name` is the catalog name (gauge lists and the `instance_up`
/// special case key on it), `exposed_name` the storage-name-resolved name
/// used in families and HELP. `fallback_ms` is the query completion time used
/// when no row carries a usable `epoch_ns`.
#[must_use]
pub fn to_sample_set(
    result: &QueryResult,
    metric_name: &str,
    exposed_name: &str,
    gauges: &Gauges,
    dbname: &str,
    fallback_ms: i64,
) -> SampleSet {
    if NOT_EXPOSED_METRICS.contains(&metric_name) {
        return SampleSet {
            timestamp_ms: fallback_ms,
            samples: Vec::new(),
            errors: 0,
        };
    }

    // Upstream stamps ALL rows with the FIRST row's epoch_ns; when it is
    // absent or not an int64, the fetch time is used — later rows are never
    // scanned (Measurements.GetEpoch, types.go).
    let timestamp_ms = result
        .rows
        .first()
        .and_then(|row| epoch_ms(result, row))
        .unwrap_or(fallback_ms);

    let mut samples = Vec::new();
    let mut errors = 0_usize;
    let mut seen: std::collections::HashSet<(String, Vec<(String, String)>)> =
        std::collections::HashSet::new();

    for row in &result.rows {
        // dbname first: a tag_dbname column overrides it (upstream writes
        // the map in this order), later tag columns override earlier ones.
        let mut labels: Vec<(String, String)> = vec![("dbname".to_owned(), dbname.to_owned())];
        let mut fields: Vec<(String, f64)> = Vec::new();
        let mut row_invalid = false;

        for (col, cell) in result.columns.iter().zip(row) {
            if col.name == "epoch_ns" {
                continue;
            }
            let Some(text) = cell else { continue };
            if text.is_empty() {
                continue;
            }
            if let Some(tag) = col.name.strip_prefix("tag_") {
                if !is_valid_label_name(tag) {
                    // the row could not be encoded; upstream drops it at
                    // exposition time
                    row_invalid = true;
                    continue;
                }
                if let Some(slot) = labels.iter_mut().find(|(k, _)| k == tag) {
                    slot.1.clone_from(text);
                } else {
                    labels.push((tag.to_owned(), text.clone()));
                }
                continue;
            }
            if let Some(value) = parse_value(col.kind, text) {
                // Upstream decodes into a map, so duplicate column names
                // keep the LAST value with no error (types.go ScanRow).
                if let Some(slot) = fields.iter_mut().find(|(n, _)| n == &col.name) {
                    slot.1 = value;
                } else {
                    fields.push((col.name.clone(), value));
                }
            }
        }
        if row_invalid {
            errors += 1;
            continue;
        }

        labels.sort_by(|a, b| a.0.cmp(&b.0));

        for (field, value) in fields {
            let family = if metric_name == INSTANCE_UP_METRIC {
                format!("pgwatch_{metric_name}")
            } else if is_valid_metric_suffix(&format!("pgwatch_{exposed_name}_{field}")) {
                format!("pgwatch_{exposed_name}_{field}")
            } else {
                errors += 1;
                continue;
            };
            let identity = (family.clone(), labels.clone());
            if !seen.insert(identity) {
                errors += 1;
                continue;
            }
            samples.push(Sample {
                family,
                help: exposed_name.to_owned(),
                is_gauge: metric_name == INSTANCE_UP_METRIC || gauges.contains(&field),
                labels: labels.clone(),
                value,
            });
        }
    }

    SampleSet {
        timestamp_ms,
        samples,
        errors,
    }
}

/// `epoch_ns` cell of a row as milliseconds, when present and parseable.
fn epoch_ms(result: &QueryResult, row: &[Cell]) -> Option<i64> {
    let idx = result.columns.iter().position(|c| c.name == "epoch_ns")?;
    let ns = row.get(idx)?.as_deref()?.parse::<i64>().ok()?;
    Some(ns.div_euclid(1_000_000))
}

fn is_valid_metric_suffix(name: &str) -> bool {
    // Prometheus metric names: [a-zA-Z_:][a-zA-Z0-9_:]*. The pgwatch_ prefix
    // is always valid; this only guards appended metric/column names.
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn is_valid_label_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expose::expose_samples;

    fn col(name: &str, kind: ColumnKind) -> Column {
        Column {
            name: name.to_owned(),
            kind,
        }
    }

    fn row(cells: &[Option<&str>]) -> Vec<Cell> {
        cells.iter().map(|c| c.map(str::to_owned)).collect()
    }

    fn convert(result: &QueryResult, metric: &str, gauges: &Gauges, dbname: &str) -> SampleSet {
        to_sample_set(result, metric, metric, gauges, dbname, 1_700_000_000_000)
    }

    fn columns() -> Vec<Column> {
        vec![
            col("epoch_ns", ColumnKind::Int),
            col("tag_schema", ColumnKind::Text),
            col("size_b", ColumnKind::Int),
            col("ratio", ColumnKind::Float),
            col("active", ColumnKind::Bool),
            col("note", ColumnKind::Text),
        ]
    }

    fn full_row() -> Vec<Cell> {
        row(&[
            Some("1700000000000000000"),
            Some("public"),
            Some("1024"),
            Some("1.5"),
            Some("t"),
            Some("hello"),
        ])
    }

    #[test]
    fn row_becomes_samples_with_dbname_and_tags() {
        let result = QueryResult {
            columns: columns(),
            rows: vec![full_row()],
        };
        let set = convert(&result, "db_size", &Gauges::All, "host1_appdb");
        assert_eq!(set.errors, 0);
        assert_eq!(set.timestamp_ms, 1_700_000_000_000);
        // text column `note` without tag_ prefix is dropped; epoch consumed
        let families: Vec<&str> = set.samples.iter().map(|s| s.family.as_str()).collect();
        assert_eq!(
            families,
            [
                "pgwatch_db_size_size_b",
                "pgwatch_db_size_ratio",
                "pgwatch_db_size_active"
            ]
        );
        for s in &set.samples {
            assert_eq!(
                s.labels,
                vec![
                    ("dbname".to_owned(), "host1_appdb".to_owned()),
                    ("schema".to_owned(), "public".to_owned())
                ]
            );
            assert_eq!(s.help, "db_size");
            assert!(s.is_gauge);
        }
        let size = set
            .samples
            .iter()
            .find(|s| s.family == "pgwatch_db_size_size_b")
            .unwrap();
        assert!((size.value - 1024.0).abs() < f64::EPSILON, "size_b");
        let ratio = set
            .samples
            .iter()
            .find(|s| s.family == "pgwatch_db_size_ratio")
            .unwrap();
        assert!((ratio.value - 1.5).abs() < f64::EPSILON, "ratio");
        let active = set
            .samples
            .iter()
            .find(|s| s.family == "pgwatch_db_size_active")
            .unwrap();
        assert!((active.value - 1.0).abs() < f64::EPSILON, "active");
    }

    #[test]
    fn counter_by_default_gauge_when_listed() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("x", ColumnKind::Int),
                col("y", ColumnKind::Int),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("1"), Some("2")])],
        };
        let set = convert(
            &result,
            "db_stats",
            &Gauges::Columns(vec!["y".to_owned()]),
            "db",
        );
        assert!(
            !set.samples
                .iter()
                .find(|s| s.family == "pgwatch_db_stats_x")
                .unwrap()
                .is_gauge
        );
        assert!(
            set.samples
                .iter()
                .find(|s| s.family == "pgwatch_db_stats_y")
                .unwrap()
                .is_gauge
        );
    }

    #[test]
    fn bool_false_is_zero() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("active", ColumnKind::Bool),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("f")])],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert!(
            (set.samples[0].value - 0.0).abs() < f64::EPSILON,
            "false is zero"
        );
    }

    #[test]
    fn null_and_empty_cells_drop_including_tags() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("tag_state", ColumnKind::Text),
                col("v", ColumnKind::Int),
                col("w", ColumnKind::Int),
            ],
            rows: vec![row(&[
                Some("1700000000000000000"),
                None,
                Some(""),
                Some("3"),
            ])],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        // NULL tag and empty-string value are gone; empty tag leaves the
        // label absent, not an empty label
        assert_eq!(set.samples.len(), 1);
        assert_eq!(
            set.samples[0].labels,
            vec![("dbname".to_owned(), "db".to_owned())]
        );
        assert_eq!(set.samples[0].family, "pgwatch_m_w");
    }

    #[test]
    fn missing_or_bad_epoch_falls_back_to_completion_time() {
        let no_epoch = QueryResult {
            columns: vec![col("v", ColumnKind::Int)],
            rows: vec![row(&[Some("1")])],
        };
        let set = to_sample_set(
            &no_epoch,
            "m",
            "m",
            &Gauges::Columns(vec![]),
            "db",
            1_234_000,
        );
        assert_eq!(set.timestamp_ms, 1_234_000);

        let null_epoch = QueryResult {
            columns: vec![col("epoch_ns", ColumnKind::Int), col("v", ColumnKind::Int)],
            rows: vec![row(&[None, Some("1")])],
        };
        let set = to_sample_set(
            &null_epoch,
            "m",
            "m",
            &Gauges::Columns(vec![]),
            "db",
            1_234_000,
        );
        assert_eq!(set.timestamp_ms, 1_234_000);
    }

    #[test]
    fn later_row_epoch_is_never_scanned() {
        // upstream GetEpoch reads strictly the first row: a NULL epoch there
        // falls back to fetch time even when a later row carries one
        let result = QueryResult {
            columns: vec![col("epoch_ns", ColumnKind::Int), col("v", ColumnKind::Int)],
            rows: vec![
                row(&[None, Some("1")]),
                row(&[Some("1800000000000000000"), Some("2")]),
            ],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert_eq!(
            set.timestamp_ms, 1_700_000_000_000,
            "fallback, not the later epoch"
        );
    }

    #[test]
    fn duplicate_column_names_keep_the_last_value() {
        // upstream decodes rows into a map: the last column wins, no error
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("v", ColumnKind::Int),
                col("v", ColumnKind::Int),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("1"), Some("2")])],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert_eq!(set.errors, 0, "duplicate names are not errors");
        assert_eq!(set.samples.len(), 1);
        assert!(
            (set.samples[0].value - 2.0).abs() < f64::EPSILON,
            "last value wins"
        );
    }

    #[test]
    fn first_row_epoch_wins_for_all_rows() {
        let result = QueryResult {
            columns: vec![col("epoch_ns", ColumnKind::Int), col("v", ColumnKind::Int)],
            rows: vec![
                row(&[Some("1700000000000000000"), Some("1")]),
                row(&[Some("1800000000000000000"), Some("2")]),
            ],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert_eq!(set.timestamp_ms, 1_700_000_000_000);
    }

    #[test]
    fn duplicate_identity_dropped_and_counted() {
        let result = QueryResult {
            columns: vec![col("epoch_ns", ColumnKind::Int), col("v", ColumnKind::Int)],
            rows: vec![
                row(&[Some("1700000000000000000"), Some("1")]),
                row(&[Some("1700000000000000000"), Some("2")]),
            ],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert_eq!(set.samples.len(), 1, "first row wins");
        assert!(
            (set.samples[0].value - 1.0).abs() < f64::EPSILON,
            "first value"
        );
        assert_eq!(set.errors, 1, "duplicate counted");
    }

    #[test]
    fn distinct_labels_are_not_duplicates() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("tag_n", ColumnKind::Text),
                col("v", ColumnKind::Int),
            ],
            rows: vec![
                row(&[Some("1700000000000000000"), Some("a"), Some("1")]),
                row(&[Some("1700000000000000000"), Some("b"), Some("2")]),
            ],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert_eq!(set.samples.len(), 2);
        assert_eq!(set.errors, 0);
    }

    #[test]
    fn tag_overrides_dbname_and_later_tags_win() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("tag_dbname", ColumnKind::Text),
                col("tag_dbname", ColumnKind::Text),
                col("v", ColumnKind::Int),
            ],
            rows: vec![row(&[
                Some("1700000000000000000"),
                Some("mine"),
                Some("yours"),
                Some("1"),
            ])],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "discovery_name");
        assert_eq!(
            set.samples[0].labels,
            vec![("dbname".to_owned(), "yours".to_owned())]
        );
    }

    #[test]
    fn instance_up_family_has_no_column_suffix() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("is_up", ColumnKind::Int),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("1")])],
        };
        let set = convert(&result, "instance_up", &Gauges::Columns(vec![]), "db");
        assert_eq!(set.samples.len(), 1);
        assert_eq!(set.samples[0].family, "pgwatch_instance_up");
        assert!(set.samples[0].is_gauge);

        // and the synthesized ping form matches it
        let ping = instance_up_sample_set("db", false, 42);
        assert_eq!(ping.samples[0].family, "pgwatch_instance_up");
        assert!(
            (ping.samples[0].value - 0.0).abs() < f64::EPSILON,
            "ping down"
        );
        assert_eq!(
            ping.samples[0].labels,
            vec![("dbname".to_owned(), "db".to_owned())]
        );
        assert_eq!(ping.timestamp_ms, 42);
    }

    #[test]
    fn storage_name_replaces_families_and_help() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("calls", ColumnKind::Int),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("5")])],
        };
        let set = to_sample_set(
            &result,
            "stat_statements_no_query_text",
            "stat_statements",
            &Gauges::Columns(vec![]),
            "db",
            0,
        );
        assert_eq!(set.samples[0].family, "pgwatch_stat_statements_calls");
        assert_eq!(set.samples[0].help, "stat_statements");
    }

    #[test]
    fn non_finite_exposed_and_malformed_drop() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("a", ColumnKind::Float),
                col("b", ColumnKind::Int),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("NaN"), Some("xx")])],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        // upstream v5.3.0 exposes NaN; only the malformed int drops
        assert_eq!(set.samples.len(), 1);
        assert!(set.samples[0].value.is_nan());
    }

    #[test]
    fn not_exposed_metrics_yield_nothing() {
        let result = QueryResult {
            columns: vec![col("epoch_ns", ColumnKind::Int), col("v", ColumnKind::Int)],
            rows: vec![row(&[Some("1700000000000000000"), Some("1")])],
        };
        for metric in NOT_EXPOSED_METRICS {
            let set = convert(&result, metric, &Gauges::Columns(vec![]), "db");
            assert!(set.samples.is_empty(), "{metric}");
        }
    }

    #[test]
    fn invalid_names_drop_with_error() {
        // a quoted column alias with spaces cannot form a valid family
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("bad name", ColumnKind::Int),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("1")])],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert!(set.samples.is_empty());
        assert_eq!(set.errors, 1);

        // tag with an invalid label name drops the sample entirely
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("tag_b@d", ColumnKind::Text),
                col("v", ColumnKind::Int),
            ],
            rows: vec![row(&[Some("1700000000000000000"), Some("x"), Some("1")])],
        };
        let set = convert(&result, "m", &Gauges::Columns(vec![]), "db");
        assert!(set.samples.is_empty());
        assert_eq!(set.errors, 1);
    }

    #[test]
    fn empty_result_is_no_samples_no_errors() {
        let result = QueryResult {
            columns: columns(),
            rows: Vec::new(),
        };
        let set = convert(&result, "db_size", &Gauges::All, "db");
        assert_eq!(set.samples.len(), 0);
        assert_eq!(set.errors, 0);
        assert_eq!(set.timestamp_ms, 1_700_000_000_000);
    }

    #[test]
    fn converted_output_exposes_clean_text() {
        let result = QueryResult {
            columns: vec![
                col("epoch_ns", ColumnKind::Int),
                col("tag_path", ColumnKind::Text),
                col("v", ColumnKind::Int),
            ],
            rows: vec![row(&[
                Some("1700000000123456789"),
                Some("a\"b\\c\nd"),
                Some("7"),
            ])],
        };
        let set = convert(&result, "wal", &Gauges::All, "h_db");
        let text = expose_samples(std::iter::once(&set));
        assert!(text.contains("# HELP pgwatch_wal_v wal\n"), "{text}");
        assert!(text.contains("# TYPE pgwatch_wal_v gauge\n"), "{text}");
        assert!(
            text.contains(
                "pgwatch_wal_v{dbname=\"h_db\",path=\"a\\\"b\\\\c\\nd\"} 7 1700000000123\n"
            ),
            "{text}"
        );
    }
}
