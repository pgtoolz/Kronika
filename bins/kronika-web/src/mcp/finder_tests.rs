use std::io;

use kronika_query::QueryError;
use kronika_reader::ReaderError;
use serde_json::{Map, Value, json};

use super::test_config;
use crate::api::ApiError;
use crate::config::Config;
use crate::tests::artifacts::{Fixture, NamedIndexSnapshot};

fn arguments(value: &Value) -> Map<String, Value> {
    value.as_object().expect("arguments object").clone()
}

fn structured(result: rmcp::model::CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false));
    result.structured_content.expect("structured content")
}

fn assert_no_storage_coordinates(value: &Value) {
    match value {
        Value::Object(object) => {
            for field in [
                "detail_locator",
                "type_id",
                "segment_id",
                "row_ordinal",
                "row_key",
            ] {
                assert!(
                    !object.contains_key(field),
                    "public MCP value exposed {field}: {value:#?}",
                );
            }
            for child in object.values() {
                assert_no_storage_coordinates(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_no_storage_coordinates(child);
            }
        }
        _ => {}
    }
}

fn stale(message: &'static str) -> QueryError {
    QueryError::Unreadable(Box::new(ReaderError::Io(io::Error::new(
        io::ErrorKind::Interrupted,
        message,
    ))))
}

#[test]
fn native_snapshot_queries_replay_only_one_source_change() {
    let fixture = Fixture::new();
    let config = test_config(fixture.root().to_path_buf());

    let mut attempts = 0;
    let value = super::run_snapshot_query(&config, |_context| {
        attempts += 1;
        if attempts == 1 {
            Err(stale("first active generation"))
        } else {
            Ok(42)
        }
    })
    .expect("second capture succeeds");
    assert_eq!(value, 42);
    assert_eq!(attempts, 2);

    let mut repeated = 0;
    let error = super::run_snapshot_query(&config, |_context| {
        repeated += 1;
        Err::<(), _>(stale(if repeated == 1 {
            "first active generation"
        } else {
            "second active generation"
        }))
    })
    .expect_err("the second source change is returned");
    assert_eq!(repeated, 2);
    assert!(error.to_string().contains("second active generation"));

    let mut refused = 0;
    let error = super::run_snapshot_query(&config, |_context| {
        refused += 1;
        Err::<(), _>(QueryError::BadFilter("filter".to_owned()))
    })
    .expect_err("semantic refusal is returned");
    assert_eq!(refused, 1);
    assert!(matches!(error, ApiError::BadFilter(_)));
}

fn detail(config: &Config, row: &Value) -> Value {
    assert_no_storage_coordinates(row);
    let detail_ref = row["detail_ref"]
        .as_str()
        .filter(|detail_ref| !detail_ref.is_empty())
        .expect("opaque detail_ref");
    let result = super::row_detail::call(
        config,
        arguments(&json!({"detail_ref": detail_ref})),
        &|| false,
    );
    let detail = structured(result);
    assert_no_storage_coordinates(&detail);
    detail
}

#[test]
fn process_time_selects_rows_without_exposing_internal_sample_metadata() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (300, 101, 60, "alpha")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let latest = structured(super::processes::call(
        &config,
        arguments(&json!({ "limit": 10 })),
        &|| false,
    ));
    assert_eq!(latest["rows"].as_array().map(Vec::len), Some(1));
    assert!(latest.get("as_of").is_none());

    let selected = structured(super::processes::call(
        &config,
        arguments(&json!({
            "at": 400,
            "filters": [{"field": "pid", "op": "eq", "value": 999}],
            "limit": 10
        })),
        &|| false,
    ));
    assert_eq!(selected["rows"], json!([]));

    let edge = structured(super::processes::call(
        &config,
        arguments(&json!({ "at": 20_000_300, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(edge["rows"].as_array().expect("rows").len(), 1);

    let outside = structured(super::processes::call(
        &config,
        arguments(&json!({ "at": 20_000_301, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(outside["rows"], json!([]));
}

#[test]
fn relation_and_recorded_postgresql_cadences_bound_current_samples() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(200, 1, 11, 10, "db", "public", "orders")]);
    let indexes: [NamedIndexSnapshot<'_>; 1] = [(
        200,
        1,
        21,
        10,
        "db",
        "public",
        "orders",
        "orders_pkey",
        "CREATE UNIQUE INDEX orders_pkey ON orders USING btree (id)",
    )];
    fixture.append_named_index_snapshots(&indexes);
    fixture.append_postgres_health_with_interval(100, 1, 60);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    for call in [
        super::postgresql::call_tables,
        super::postgresql::call_indexes,
    ] {
        let result = structured(call(
            &config,
            arguments(&json!({
                "at": 750_000_200,
                "group": "object",
                "limit": 10
            })),
            &|| false,
        ));
        assert_eq!(result["rows"].as_array().expect("rows").len(), 1);
    }

    let old_table = structured(super::postgresql::call_tables(
        &config,
        arguments(&json!({
            "at": 750_000_201,
            "group": "object",
            "limit": 10
        })),
        &|| false,
    ));
    assert_eq!(old_table["rows"], json!([]));

    let recorded_cadence = structured(super::postgresql::call_activity(
        &config,
        arguments(&json!({ "at": 150_000_150, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(recorded_cadence["rows"].as_array().expect("rows").len(), 2);

    let outside_recorded_cadence = structured(super::postgresql::call_activity(
        &config,
        arguments(&json!({ "at": 150_000_151, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(outside_recorded_cadence["rows"], json!([]));
}

#[test]
fn active_metadata_and_default_postgresql_cadences_bound_samples() {
    let mut active = Fixture::new();
    active.append_postgres_health_with_interval(100, 1, 60);
    let config = test_config(active.root().to_path_buf());
    let edge = structured(super::postgresql::call_activity(
        &config,
        arguments(&json!({ "at": 150_000_150, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(edge["rows"].as_array().expect("rows").len(), 2);

    let mut fallback = Fixture::new();
    fallback.append_postgres_health_with_interval(100, 1, 0);
    fallback.finish();
    let config = test_config(fallback.root().to_path_buf());
    let edge = structured(super::postgresql::call_activity(
        &config,
        arguments(&json!({ "at": 75_000_150, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(edge["rows"].as_array().map(Vec::len), Some(2));
    let outside = structured(super::postgresql::call_activity(
        &config,
        arguments(&json!({ "at": 75_000_151, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(outside["rows"], json!([]));
}

#[test]
fn in_combines_plain_and_relation_filter_values() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.append_named_table_snapshots(&[
        (100, 1, 11, 0, "db", "public", "alpha"),
        (100, 1, 12, 0, "db", "public", "beta"),
    ]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let scalar = structured(super::processes::call(
        &config,
        arguments(&json!({
            "filters": [{"field": "pid", "op": "eq", "value": 101}],
            "limit": 10
        })),
        &|| false,
    ));
    assert_eq!(scalar["rows"].as_array().expect("rows").len(), 1);

    let any = structured(super::processes::call(
        &config,
        arguments(&json!({
            "filters": [{
                "field": "pid",
                "op": "in",
                "values": [101, "101", 901, 902, 903, 904, 905, 906]
            }],
            "limit": 10
        })),
        &|| false,
    ));
    assert_eq!(any["rows"].as_array().expect("rows").len(), 1);

    let and_or = structured(super::processes::call(
        &config,
        arguments(&json!({
            "filters": [
                {"field": "command", "op": "in", "values": ["ALPHA", "beta"]},
                {"field": "pid", "op": "in", "values": [102, 999]}
            ],
            "limit": 10
        })),
        &|| false,
    ));
    let rows = and_or["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["pid"], 102);

    let scalar = structured(super::postgresql::call_tables(
        &config,
        arguments(&json!({
            "group": "object",
            "filters": [{"field": "table_name", "op": "eq", "value": "alpha"}],
            "limit": 10
        })),
        &|| false,
    ));
    assert_eq!(scalar["rows"].as_array().expect("rows").len(), 1);

    let any = structured(super::postgresql::call_tables(
        &config,
        arguments(&json!({
            "group": "object",
            "filters": [{
                "field": "table_name",
                "op": "in",
                "values": ["alpha", "missing-1", "missing-2", "missing-3",
                           "missing-4", "missing-5", "missing-6", "missing-7"]
            }],
            "limit": 10
        })),
        &|| false,
    ));
    assert_eq!(any["rows"].as_array().expect("rows").len(), 1);
}

#[test]
fn finder_results_truncate_without_a_continuation_contract() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let result = super::processes::call(&config, arguments(&json!({ "limit": 1 })), &|| false);
    assert_eq!(result.is_error, Some(false));
    let content: Value = serde_json::from_str(&result.content[0].as_text().expect("JSON").text)
        .expect("content JSON");
    let output = result.structured_content.expect("structured content");
    assert_eq!(content, output);
    assert_eq!(output["truncated"], true);
    assert_eq!(output["rows"].as_array().expect("rows").len(), 1);
    for obsolete in ["has_more", "next_from", "next_cursor", "cursor"] {
        assert!(output.get(obsolete).is_none(), "obsolete field {obsolete}");
    }
}

#[test]
fn a_predecessor_before_the_current_window_still_feeds_rates() {
    const CURRENT: i64 = 20_000_100;
    const INTERMEDIATE_SEGMENT: i64 = 1_709_164_800_000_500;
    const CURRENT_SEGMENT: i64 = 1_709_164_800_001_000;
    let mut fixture = Fixture::new();
    fixture.append_process_counter_rows(&[(100, Some(100))]);
    fixture.append_named_table_snapshots(&[(100, 1, 11, 10, "db", "public", "orders")]);
    fixture.finish_and_continue(INTERMEDIATE_SEGMENT);
    fixture.append_log_error(10_000_100);
    fixture.finish_and_continue(CURRENT_SEGMENT);
    fixture.append_process_counter_rows(&[(CURRENT, Some(200))]);
    fixture.append_named_table_snapshots(&[(750_000_100, 1, 11, 40, "db", "public", "orders")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let process = structured(super::processes::call(
        &config,
        arguments(&json!({ "at": CURRENT + 20_000_000, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(process["rows"][0]["read_bytes"], 5.0);
    let process_detail = detail(&config, &process["rows"][0]);
    assert_eq!(process_detail["at"], CURRENT.to_string());

    let table = structured(super::postgresql::call_tables(
        &config,
        arguments(&json!({
            "at": 1_500_000_100_i64,
            "group": "object",
            "limit": 10
        })),
        &|| false,
    ));
    assert_eq!(table["rows"][0]["seq_scan"], 0.04);
}

#[test]
fn finder_chooses_the_latest_actual_sample_across_overlapping_segments() {
    const SECOND_SEGMENT: i64 = 1_709_164_800_001_000;
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(300, 101, 60, "later")]);
    fixture.finish_and_continue(SECOND_SEGMENT);
    fixture.append_process_gauge_rows(&[(200, 101, 40, "earlier")]);
    fixture.append_log_error(400);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let result = structured(super::processes::call(
        &config,
        arguments(&json!({ "at": 300, "limit": 10 })),
        &|| false,
    ));
    assert_eq!(result["rows"][0]["comm"], "later");
    let detail = detail(&config, &result["rows"][0]);
    assert_eq!(detail["at"], "300");
    assert_eq!(detail["comm"], "later");
}

#[test]
fn missing_rollback_and_real_zero_counter_rates_remain_distinct() {
    for (rows, expected) in [
        (vec![(200, Some(100))], Value::Null),
        (vec![(100, Some(100)), (200, Some(50))], Value::Null),
        (vec![(100, Some(100)), (200, Some(100))], json!(0.0)),
    ] {
        let mut fixture = Fixture::new();
        fixture.append_process_counter_rows(&rows);
        fixture.finish();
        let config = test_config(fixture.root().to_path_buf());
        let result = structured(super::processes::call(
            &config,
            arguments(&json!({ "at": 200, "limit": 10 })),
            &|| false,
        ));
        assert_eq!(result["rows"][0]["read_bytes"], expected);
    }
}

#[test]
fn cancellation_is_an_error_instead_of_an_empty_success() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());
    let result = super::processes::call(&config, arguments(&json!({ "limit": 10 })), &|| true);
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.expect("structured error")["message"],
        "request cancelled"
    );
}

#[test]
fn every_plain_postgresql_finder_accepts_an_explicit_point() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());
    for call in [
        super::postgresql::call_locks,
        super::postgresql::call_vacuum,
        super::postgresql::call_databases,
        super::postgresql::call_statements,
        super::postgresql::call_plans,
    ] {
        let result = structured(call(
            &config,
            arguments(&json!({ "at": 100, "limit": 1 })),
            &|| false,
        ));
        assert_eq!(result["rows"], json!([]));
        assert!(result.get("as_of").is_none());
    }
}

#[test]
fn omitted_at_uses_the_global_store_bound_and_drops_an_old_vacuum() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_vacuum_rows(&[(100, 42, "scanning heap", 10, 5, 0)]);
    fixture.append_log_error(75_000_101);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let result = structured(super::postgresql::call_vacuum(
        &config,
        arguments(&json!({ "limit": 10 })),
        &|| false,
    ));

    assert_eq!(result, json!({"rows": [], "truncated": false}));
}

#[test]
fn an_unknown_sort_is_rejected_even_when_the_surface_has_no_sample() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());
    let result = super::postgresql::call_vacuum(
        &config,
        arguments(&json!({
            "at": 100,
            "sort": {"field": "made_up", "direction": "asc"},
            "limit": 1
        })),
        &|| false,
    );
    assert_eq!(result.is_error, Some(true));
    let message = &result.content[0].as_text().expect("error text").text;
    assert!(message.contains("no such sort field"), "{message}");
    assert!(!message.contains("made_up"), "{message}");
}
