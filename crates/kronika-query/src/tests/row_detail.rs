use serde_json::{Value, json};

use super::{counter_delta, normalize_detail_text};
use kronika_reader::Cell;

#[test]
fn present_detail_text_has_one_stable_shape() {
    let mut activity = json!({
        "query": "select 'привет'",
        "state": "active",
    })
    .as_object()
    .expect("activity")
    .clone();
    normalize_detail_text("pg_stat_activity", &mut activity).expect("normalize activity");
    assert_eq!(
        activity["query"],
        json!({
            "stored_text": "select 'привет'",
            "full_len": "21",
            "truncated": false,
            "sha256": null,
        })
    );
    assert_eq!(activity["state"], "active");

    let mut error = json!({
        "sample": {
            "representation": "text",
            "stored_text": "stored prefix",
            "full_len": "9000",
            "truncated": true,
            "sha256": "abc123",
        },
        "detail": null,
        "pattern": "duplicate key value violates constraint ?",
    })
    .as_object()
    .expect("error")
    .clone();
    normalize_detail_text("pg_log_errors", &mut error).expect("normalize error");
    assert_eq!(
        error["sample"],
        json!({
            "stored_text": "stored prefix",
            "full_len": "9000",
            "truncated": true,
            "sha256": "abc123",
        })
    );
    assert_eq!(error["detail"], Value::Null);
    assert!(error["sample"].get("representation").is_none());
}

#[test]
fn counter_deltas_reject_resets_and_non_finite_values() {
    assert_eq!(counter_delta(&Cell::U64(15), &Cell::U64(5)), Some(10.0));
    assert_eq!(counter_delta(&Cell::U64(5), &Cell::U64(15)), None);
    assert_eq!(counter_delta(&Cell::F64(2.5), &Cell::F64(1.0)), Some(1.5));
    assert_eq!(counter_delta(&Cell::F64(f64::NAN), &Cell::F64(1.0)), None);
}
