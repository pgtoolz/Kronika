use crate::{RelationGroup, exact_product::compare_products};
use serde_json::{Value, json};
use std::cmp::Ordering;

use super::*;

fn key(datid: u32) -> GroupKey {
    GroupKey(GroupKeyValue::Index {
        datid,
        datname: format!("db{datid}"),
        schemaname: "public".to_owned(),
        relid: 10 + datid,
        relname: format!("table{datid}"),
        indexrelid: 20 + datid,
        indexrelname: format!("index{datid}"),
    })
}

fn table_key(datid: u32) -> GroupKey {
    GroupKey(GroupKeyValue::Table {
        datid,
        datname: format!("db{datid}"),
        schemaname: "public".to_owned(),
        relid: 10 + datid,
        relname: format!("table{datid}"),
    })
}

const fn source(context_index: usize, ordinal: u64) -> RelationSource {
    RelationSource::new(7, context_index, ordinal, 1_014_004, 42)
}

fn aggregate() -> RelationAggregate {
    RelationAggregate::new(key(1), source(2, 3))
}

fn add_rate(rate: &mut RateAggregate, delta: i128) {
    rate.add(Input::Value(OrderedNumber::Integer(delta)), Some(1_000_000));
}

fn metric_number(metric: Option<Metric>) -> Value {
    metric.expect("available metric").json()
}

#[test]
fn exact_metric_comparison_preserves_large_ratios() {
    assert_eq!(
        compare_products(&[100, 1_000_000], &[1_048_575, 1_000_000]),
        Ordering::Less
    );
    assert_eq!(
        compare_products(&[u128::MAX, u128::MAX], &[u128::MAX, u128::MAX - 1]),
        Ordering::Greater
    );
    let larger = Metric::rate(RateValue::exact(u128::MAX, u128::MAX - 1));
    let smaller = Metric::rate(RateValue::exact(u128::MAX - 1, u128::MAX));
    assert_eq!(larger.compare(&smaller), Some(Ordering::Greater));
    assert_eq!(smaller.compare(&larger), Some(Ordering::Less));
    assert_eq!(larger.compare(&larger), Some(Ordering::Equal));
}

#[test]
fn relation_contract_rejects_physical_and_cross_section_fields() {
    let sections = vec![TABLES.to_owned()];
    assert!(matches!(
        output_fields(
            &sections,
            RelationGroup::Database,
            &["relid".to_owned()]
        ),
        Err(QueryError::NoSuchColumn(name)) if name == "relid"
    ));
    assert!(matches!(
        output_fields(
            &[TABLES.to_owned(), INDEXES.to_owned()],
            RelationGroup::Database,
            &[]
        ),
        Err(QueryError::BadFilter(name)) if name == "group"
    ));
}

#[test]
fn object_keys_are_minimal_and_display_identity_is_a_value() {
    let table_key = table_key(7).json(RelationKind::Tables, RelationGroup::Object);
    assert_eq!(
        table_key
            .as_object()
            .expect("table key")
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["datid", "datname", "relid", "relname", "schemaname"]
    );
    let index_key = key(7).json(RelationKind::Indexes, RelationGroup::Object);
    assert_eq!(
        index_key
            .as_object()
            .expect("index key")
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        [
            "datid",
            "datname",
            "indexrelid",
            "indexrelname",
            "relid",
            "relname",
            "schemaname"
        ]
    );
    let output = output_fields(
        &[INDEXES.to_owned()],
        RelationGroup::Object,
        &[
            "datid".to_owned(),
            "indexrelid".to_owned(),
            "indexrelname".to_owned(),
            "relid".to_owned(),
            "relname".to_owned(),
        ],
    )
    .expect("valid identity fields");
    assert!(output.is_empty(), "identity values are emitted in the key");

    assert!(matches!(
        output_fields(
            &[INDEXES.to_owned()],
            RelationGroup::Object,
            &["indexdef".to_owned()]
        ),
        Err(QueryError::NoSuchColumn(name)) if name == "indexdef"
    ));
}

#[test]
fn database_schema_and_object_keys_keep_database_scope() {
    let first = key(1);
    let second = key(2);
    assert_eq!(
        first.metric("schemaname").expect("schema metric").json(),
        second.metric("schemaname").expect("schema metric").json()
    );
    assert_ne!(
        first, second,
        "the same schema name in two databases is distinct"
    );

    assert_eq!(
        first
            .json(RelationKind::Tables, RelationGroup::Database)
            .as_object()
            .expect("database key")
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["datid", "datname"]
    );
    assert_eq!(
        first
            .json(RelationKind::Tables, RelationGroup::Schema)
            .as_object()
            .expect("schema key")
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["datid", "datname", "schemaname"]
    );
    assert_ne!(
        first.json(RelationKind::Tables, RelationGroup::Object),
        second.json(RelationKind::Tables, RelationGroup::Object)
    );
}

#[test]
fn aggregate_contract_has_explicit_counts_and_timestamp_semantics() {
    let table = RelationKind::Tables.fields(RelationGroup::Schema);
    assert_eq!(
        table
            .iter()
            .find(|field| field.name() == "table_count")
            .and_then(|field| field.unit()),
        Some("count")
    );
    assert!(
        table
            .iter()
            .any(|field| field.name() == "last_vacuum_oldest")
    );
    assert!(
        table
            .iter()
            .any(|field| field.name() == "last_vacuum_never_count")
    );
    assert!(
        table
            .iter()
            .any(|field| field.name() == "toast_last_autovacuum_latest")
    );
    assert!(!table.iter().any(|field| field.name() == "tablespace"));
    assert_eq!(
        RelationKind::Tables.fields(RelationGroup::Tablespace)[0].name(),
        "tablespace"
    );

    let index = RelationKind::Indexes.fields(RelationGroup::Database);
    for name in [
        "index_count",
        "last_idx_scan_never_count",
        "no_scan_count",
        "known_scan_count",
        "invalid_count",
        "unready_count",
        "unique_count",
        "primary_count",
        "exclusion_count",
    ] {
        assert_eq!(
            index
                .iter()
                .find(|field| field.name() == name)
                .and_then(|field| field.unit()),
            Some("count"),
            "{name}"
        );
    }
    for name in [
        "idx_scan",
        "idx_tup_read",
        "idx_tup_fetch",
        "idx_blks_read",
        "idx_blks_hit",
    ] {
        assert_eq!(
            index
                .iter()
                .find(|field| field.name() == name)
                .and_then(|field| field.unit()),
            Some("per_second"),
            "{name}"
        );
    }
    assert!(!index.iter().any(|field| field.name() == "indisvalid"));
    assert_eq!(
        RelationKind::Indexes.fields(RelationGroup::Tablespace)[0].name(),
        "tablespace"
    );
}

#[test]
fn unavailable_values_are_explicit_nulls() {
    let metrics = BTreeMap::from([
        ("available".to_owned(), Some(Metric::integer(7))),
        ("unavailable".to_owned(), None),
    ]);
    let values = relation_values(&metrics);
    assert_eq!(
        values.get("available"),
        Some(&Value::String("7".to_owned()))
    );
    assert_eq!(values.get("unavailable"), Some(&Value::Null));
}

#[test]
fn additive_rates_and_ratios_sum_each_objects_own_interval() {
    let mut aggregate = aggregate();
    let mut sequential = RateAggregate::default();
    sequential.add(Input::Value(OrderedNumber::Integer(10)), Some(10_000_000));
    sequential.add(Input::Value(OrderedNumber::Integer(20)), Some(5_000_000));
    aggregate.rates.insert("seq_scan", sequential);
    let mut indexed = RateAggregate::default();
    indexed.add(Input::Value(OrderedNumber::Integer(10)), Some(10_000_000));
    indexed.add(Input::Value(OrderedNumber::Integer(5)), Some(5_000_000));
    aggregate.rates.insert("idx_scan", indexed);
    assert_eq!(
        metric_number(sequential.metric()),
        json!(5.0),
        "10/10s + 20/5s is five scans per second"
    );
    assert_eq!(
        metric_number(aggregate.metric(
            RelationKind::Tables,
            RelationGroup::Database,
            "sequential_share_pct",
        )),
        json!(500.0 / 7.0),
        "the percentage is recomputed from the summed 5/s and 2/s operands"
    );
    let exact = sequential.metric().expect("exact rate");
    assert_eq!(
        exact.compare(&Metric::rate(RateValue::exact(4, 1_000_000))),
        Some(Ordering::Greater)
    );
}

#[test]
fn table_cuts_recompute_from_exact_operands() {
    let mut aggregate = aggregate();
    for (name, delta) in [
        ("seq_scan", 2),
        ("idx_scan", 6),
        ("seq_tup_read", 20),
        ("idx_tup_fetch", 60),
        ("n_tup_ins", 1),
        ("n_tup_upd", 3),
        ("n_tup_del", 6),
        ("n_tup_hot_upd", 2),
        ("n_tup_newpage_upd", 1),
        ("heap_blks_read", 1),
        ("heap_blks_hit", 9),
        ("idx_blks_read", 2),
        ("idx_blks_hit", 8),
    ] {
        let mut rate = RateAggregate::default();
        add_rate(&mut rate, delta);
        aggregate.rates.insert(name, rate);
    }
    for name in [
        "toast_blks_read",
        "toast_blks_hit",
        "tidx_blks_read",
        "tidx_blks_hit",
    ] {
        aggregate.rates.insert(name, RateAggregate::default());
    }
    for (name, value) in [
        ("main_fork_bytes", 800),
        ("toast_bytes", 200),
        ("toast_n_live_tup", 90),
        ("toast_n_dead_tup", 10),
    ] {
        aggregate
            .gauges
            .insert(name, Availability::Value(OrderedNumber::Integer(value)));
    }

    let table = |name: &'static str| {
        aggregate
            .metric(RelationKind::Tables, RelationGroup::Database, name)
            .expect("available table cut")
            .json()
    };
    assert_eq!(table("tuple_throughput"), json!(80.0));
    assert_eq!(table("dml_total"), json!(10.0));
    assert_eq!(table("insert_share_pct"), json!(10.0));
    assert_eq!(table("update_share_pct"), json!(30.0));
    assert_eq!(table("delete_share_pct"), json!(60.0));
    assert_eq!(table("displayed_storage_bytes"), json!("1000"));
    assert_eq!(table("toast_share_pct"), json!(20.0));
    assert_eq!(table("toast_dead_pct"), json!(10.0));
    let assert_close = |name: &'static str, expected: f64| {
        let value = table(name);
        let actual = value.as_f64().expect("numeric table cut");
        assert!((actual - expected).abs() < 1e-12, "{name}: {actual}");
    };
    assert_close("heap_buffer_hit_pct", 90.0);
    assert_close("index_buffer_hit_pct", 80.0);
    assert_close("buffer_hit_pct", 85.0);
    assert!(
        aggregate
            .metric(
                RelationKind::Tables,
                RelationGroup::Database,
                "toast_buffer_hit_pct"
            )
            .is_none()
    );
}

#[test]
fn size_metrics_use_the_authoritative_reducer_and_exact_boundaries() {
    let mut table = aggregate();
    table.gauges.insert(
        "main_fork_bytes",
        Availability::Value(OrderedNumber::Integer(80_000_000)),
    );
    table.gauges.insert(
        "toast_bytes",
        Availability::Value(OrderedNumber::Integer(25_000_000)),
    );
    let size = table
        .metric(
            RelationKind::Tables,
            RelationGroup::Object,
            "displayed_storage_bytes",
        )
        .expect("table size");
    assert_eq!(
        size.compare(&Metric::integer(100_000_000)),
        Some(Ordering::Greater)
    );

    table.gauges.insert(
        "toast_bytes",
        Availability::Value(OrderedNumber::Integer(20_000_000)),
    );
    let size = table
        .metric(
            RelationKind::Tables,
            RelationGroup::Object,
            "displayed_storage_bytes",
        )
        .expect("table size");
    assert_eq!(
        size.compare(&Metric::integer(100_000_000)),
        Some(Ordering::Equal)
    );
    assert_eq!(
        size.compare(&Metric::integer(99_999_999)),
        Some(Ordering::Greater)
    );
    assert_eq!(
        size.compare(&Metric::integer(100_000_001)),
        Some(Ordering::Less)
    );

    table
        .gauges
        .insert("toast_bytes", Availability::Unavailable);
    assert!(
        table
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "displayed_storage_bytes",
            )
            .is_none()
    );

    let mut index = aggregate();
    index.gauges.insert(
        "main_fork_bytes",
        Availability::Value(OrderedNumber::Integer(100_000_000)),
    );
    let size = index
        .metric(
            RelationKind::Indexes,
            RelationGroup::Object,
            "main_fork_bytes",
        )
        .expect("index size");
    assert_eq!(
        size.compare(&Metric::integer(100_000_000)),
        Some(Ordering::Equal)
    );
}

#[test]
fn structural_absence_is_zero_only_inside_valid_sums() {
    let mut stored = aggregate();
    stored.gauges.insert(
        "main_fork_bytes",
        Availability::Value(OrderedNumber::Integer(800)),
    );
    stored.gauges.insert("toast_bytes", Availability::Empty);
    assert_eq!(
        metric_number(stored.metric(
            RelationKind::Tables,
            RelationGroup::Object,
            "displayed_storage_bytes"
        )),
        json!("800")
    );
    assert_eq!(
        metric_number(stored.metric(
            RelationKind::Tables,
            RelationGroup::Object,
            "toast_share_pct"
        )),
        json!(0.0)
    );

    for name in ["heap_blks_read", "heap_blks_hit"] {
        let mut rate = RateAggregate::default();
        add_rate(&mut rate, 0);
        stored.rates.insert(name, rate);
    }
    for name in [
        "idx_blks_read",
        "idx_blks_hit",
        "toast_blks_read",
        "toast_blks_hit",
        "tidx_blks_read",
        "tidx_blks_hit",
    ] {
        stored.rates.insert(name, RateAggregate::default());
    }
    let ratio = stored
        .metric(
            RelationKind::Tables,
            RelationGroup::Object,
            "buffer_hit_pct",
        )
        .expect("exact zero-access ratio");
    assert_eq!(ratio.json(), Value::Null);
    assert!(ratio.compare(&ratio).is_none());

    let mut scans = aggregate();
    let mut sequential = RateAggregate::default();
    add_rate(&mut sequential, 4);
    scans.rates.insert("seq_scan", sequential);
    scans.rates.insert("idx_scan", RateAggregate::default());
    assert_eq!(
        metric_number(scans.metric(
            RelationKind::Tables,
            RelationGroup::Object,
            "sequential_share_pct"
        )),
        json!(100.0)
    );

    stored
        .gauges
        .insert("toast_bytes", Availability::Unavailable);
    assert!(
        stored
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "displayed_storage_bytes"
            )
            .is_none()
    );
}

#[test]
fn zero_denominators_are_unavailable_for_table_cuts() {
    let mut aggregate = aggregate();
    for name in [
        "seq_scan",
        "seq_tup_read",
        "idx_scan",
        "idx_tup_fetch",
        "n_tup_ins",
        "n_tup_upd",
        "n_tup_del",
        "n_tup_hot_upd",
        "n_tup_newpage_upd",
        "heap_blks_read",
        "heap_blks_hit",
    ] {
        let mut rate = RateAggregate::default();
        add_rate(&mut rate, 0);
        aggregate.rates.insert(name, rate);
    }
    for name in [
        "n_live_tup",
        "n_dead_tup",
        "main_fork_bytes",
        "toast_bytes",
        "toast_n_live_tup",
        "toast_n_dead_tup",
    ] {
        aggregate
            .gauges
            .insert(name, Availability::Value(OrderedNumber::Integer(0)));
    }

    assert_eq!(
        metric_number(aggregate.metric(
            RelationKind::Tables,
            RelationGroup::Object,
            "tuple_throughput"
        )),
        json!(0.0)
    );
    assert_eq!(
        metric_number(aggregate.metric(RelationKind::Tables, RelationGroup::Object, "dml_total")),
        json!(0.0)
    );
    for name in [
        "sequential_share_pct",
        "seq_tuples_per_scan",
        "idx_tuples_per_scan",
        "insert_share_pct",
        "update_share_pct",
        "delete_share_pct",
        "hot_pct",
        "new_page_pct",
        "dead_pct",
        "toast_share_pct",
        "toast_dead_pct",
        "heap_buffer_hit_pct",
    ] {
        let metric = aggregate
            .metric(RelationKind::Tables, RelationGroup::Object, name)
            .unwrap_or_else(|| panic!("exact zero-denominator metric {name}"));
        assert_eq!(metric.json(), Value::Null, "{name}");
        assert!(metric.compare(&metric).is_none(), "{name}");
    }
}

#[test]
fn last_scan_never_is_distinct_from_layout_absence() {
    let mut aggregate = aggregate();
    let mut never = TimestampAggregate::default();
    never.add(TimestampObservation::Stored(Some(&Cell::Null)));
    aggregate.timestamps.insert("last_seq_scan", never);
    assert_eq!(
        aggregate
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "last_seq_scan_never",
            )
            .expect("never metric")
            .json(),
        json!(true)
    );

    let mut seen = TimestampAggregate::default();
    seen.add(TimestampObservation::Stored(Some(&Cell::Ts(30))));
    aggregate.timestamps.insert("last_idx_scan", seen);
    for kind in [RelationKind::Tables, RelationKind::Indexes] {
        assert_eq!(
            aggregate
                .metric(kind, RelationGroup::Object, "last_idx_scan_never")
                .expect("seen metric")
                .json(),
            json!(false)
        );
    }

    let mut absent = TimestampAggregate::default();
    absent.add(TimestampObservation::Unavailable);
    aggregate.timestamps.insert("last_seq_scan", absent);
    assert!(
        aggregate
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "last_seq_scan_never"
            )
            .is_none()
    );
}

#[test]
fn reset_and_layout_unknown_poison_but_structural_null_is_neutral() {
    let mut sum = Availability::default();
    sum.add(Input::Neutral);
    assert!(sum.value().is_none(), "all structural N/A stays null");
    sum.add(Input::Value(OrderedNumber::Integer(8)));
    assert!(matches!(sum.value(), Some(OrderedNumber::Integer(8))));
    sum.add(Input::Unavailable);
    assert!(
        sum.value().is_none(),
        "reset/layout absence poisons the sum"
    );

    let mut rate = RateAggregate::default();
    rate.add(Input::Neutral, None);
    assert!(rate.metric().is_none(), "all structural N/A stays null");
    add_rate(&mut rate, 8);
    rate.add(Input::Unavailable, Some(1_000_000));
    assert!(
        rate.metric().is_none(),
        "one reset poisons an aggregate rate"
    );

    let mut missing_elapsed = RateAggregate::default();
    missing_elapsed.add(Input::Value(OrderedNumber::Integer(1)), None);
    assert!(missing_elapsed.metric().is_none());
}

#[test]
fn ages_take_the_maximum_ignore_partition_na_and_reject_layout_absence() {
    let mut maximum = MaximumAggregate::default();
    maximum.add(true, Some(&Cell::Null));
    maximum.add(true, Some(&Cell::I64(41)));
    maximum.add(true, Some(&Cell::I64(17)));
    assert_eq!(maximum.exact(), Some(41));

    let mut all_na = MaximumAggregate::default();
    all_na.add(true, Some(&Cell::Null));
    assert_eq!(all_na.exact(), None);

    maximum.add(false, None);
    assert_eq!(maximum.exact(), None, "layout absence poisons the maximum");
}

#[test]
fn timestamps_keep_oldest_latest_and_explicit_never() {
    let mut timestamp = TimestampAggregate::default();
    timestamp.add(TimestampObservation::Stored(Some(&Cell::Ts(30))));
    timestamp.add(TimestampObservation::Stored(Some(&Cell::Null)));
    timestamp.add(TimestampObservation::Stored(Some(&Cell::Ts(10))));
    assert!(timestamp.exact());
    assert_eq!(timestamp.oldest, Some(10));
    assert_eq!(timestamp.latest, Some(30));
    assert_eq!(timestamp.never, 1);

    let mut toast = TimestampAggregate::default();
    toast.add(TimestampObservation::NotApplicable);
    assert!(!toast.exact(), "all no-TOAST rows stay unavailable");
    toast.add(TimestampObservation::Stored(Some(&Cell::Null)));
    assert!(toast.exact());
    assert_eq!(toast.never, 1);

    let mut absent = TimestampAggregate::default();
    absent.add(TimestampObservation::Unavailable);
    assert!(!absent.exact());
}

#[test]
fn index_flags_and_each_objects_scan_delta_are_counted() {
    let mut scans = BoolAggregate::default();
    scans.add_scan(Input::Value(OrderedNumber::Integer(0)));
    scans.add_scan(Input::Value(OrderedNumber::Integer(5)));
    scans.add_scan(Input::Value(OrderedNumber::Integer(0)));
    assert!(scans.exact());
    assert_eq!((scans.known, scans.truthy), (3, 2));

    let mut aggregate = aggregate();
    let mut valid = BoolAggregate::default();
    valid.add(Some(&Cell::Bool(true)));
    valid.add(Some(&Cell::Bool(false)));
    aggregate.flags.insert("indisvalid", valid);
    assert_eq!(
        aggregate
            .flag_count("indisvalid", false)
            .expect("invalid count")
            .json(),
        json!("1")
    );

    aggregate.state_severity = Some(2);
    assert_eq!(
        aggregate
            .metric(
                RelationKind::Indexes,
                RelationGroup::Database,
                "state_severity",
            )
            .expect("state severity")
            .json(),
        json!("2")
    );

    scans.add_scan(Input::Unavailable);
    assert!(!scans.exact(), "unknown scan delta poisons the count");
}

#[test]
fn reltuples_unknown_poisons_the_sum_and_zero_remains_exact() {
    let mut aggregate = Availability::default();
    aggregate.add(Input::Value(OrderedNumber::Integer(0)));
    aggregate.add(Input::Value(OrderedNumber::Integer(12)));
    assert!(matches!(
        aggregate.value(),
        Some(OrderedNumber::Integer(12))
    ));
    aggregate.add(Input::Unavailable);
    assert!(aggregate.value().is_none());
}
