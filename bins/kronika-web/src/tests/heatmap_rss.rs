use serde_json::{Value, json};

use super::{Fixture, query_response, raw_ndjson_records};

fn rss_records(fixture: &Fixture, columns: usize, top: usize, group: bool) -> Vec<Value> {
    let group = if group { "&group=comm" } else { "" };
    let target = format!(
        "/api/heatmap?from=100&to=999&section=os_process&field=rmem_kb&columns={columns}&top={top}{group}"
    );
    let (_meta, bytes) =
        query_response(fixture, &target, usize::MAX, usize::MAX).expect("RSS heatmap");
    raw_ndjson_records(&bytes)
}

fn append_rss_snapshots(fixture: &mut Fixture) {
    fixture.append_process_gauge_rows(&[
        (50, 201, 900, "steady"),
        (100, 101, 90, "postgres"),
        (200, 102, 90, "postgres"),
        (900, 103, 90, "postgres"),
        (100, 201, 100, "steady"),
        (200, 201, 100, "steady"),
        (900, 201, 100, "steady"),
        (1_000, 201, 900, "steady"),
        (100, 301, 0, "zero"),
        (200, 301, 0, "zero"),
        (900, 301, 0, "zero"),
        (100, 401, 210, "spike"),
    ]);
}

#[test]
fn rss_means_rank_recorded_snapshot_contributions_independently_of_columns() {
    for finished in [false, true] {
        let mut fixture = Fixture::new();
        append_rss_snapshots(&mut fixture);
        if finished {
            fixture.finish();
        }

        for columns in [1, 60] {
            for group in [false, true] {
                let records = rss_records(&fixture, columns, 1, group);
                assert_eq!(records[0]["summary"], "mean");
                assert_eq!(records[0]["class"], "gauge");
                assert_eq!(records[0]["entity_count"], if group { 4 } else { 6 });
                assert_eq!(records[0]["others_count"], if group { 3 } else { 5 });
                assert_eq!(records[1]["total"], 100.0);
                assert_eq!(
                    records[1]["identity"],
                    if group {
                        json!(["steady"])
                    } else {
                        json!([201])
                    }
                );
                if !group {
                    assert!(
                        records[1]["labels"]
                            .as_array()
                            .expect("process labels")
                            .contains(&json!("steady"))
                    );
                }
                assert_eq!(records[2]["band"], "totals");
                assert_eq!(records[2]["total"], 260.0);
                assert_eq!(records[3]["band"], "others");
                assert_eq!(records[3]["total"], 160.0);
            }
        }

        let grouped = rss_records(&fixture, 60, 10, true);
        let groups = &grouped[1..5];
        assert_eq!(
            groups
                .iter()
                .map(|row| row["identity"].clone())
                .collect::<Vec<_>>(),
            vec![
                json!(["steady"]),
                json!(["postgres"]),
                json!(["spike"]),
                json!(["zero"])
            ]
        );
        assert_eq!(groups[1]["members"], 3);
        assert_eq!(
            groups
                .iter()
                .map(|row| row["total"].clone())
                .collect::<Vec<_>>(),
            vec![json!(100.0), json!(90.0), json!(70.0), json!(0.0)]
        );
        assert!(grouped[6]["total"].is_null());

        let entities = rss_records(&fixture, 60, 10, false);
        assert_eq!(
            entities[1..7]
                .iter()
                .map(|row| row["total"].clone())
                .collect::<Vec<_>>(),
            vec![
                json!(100.0),
                json!(70.0),
                json!(30.0),
                json!(30.0),
                json!(30.0),
                json!(0.0)
            ]
        );
        assert!(entities[8]["total"].is_null());
    }
}

#[test]
fn rss_mean_keeps_zero_and_no_recorded_snapshots_distinct() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 0, "zero")]);
    fixture.finish();
    for group in [false, true] {
        let records = rss_records(&fixture, 60, 10, group);
        assert_eq!(records[1]["total"], 0.0);
        assert_eq!(records[2]["total"], 0.0);
        assert!(records[3]["total"].is_null());

        let group = if group { "&group=comm" } else { "" };
        let target = format!(
            "/api/heatmap?from=200&to=999&section=os_process&field=rmem_kb&columns=60&top=10{group}"
        );
        let (_meta, bytes) =
            query_response(&fixture, &target, usize::MAX, usize::MAX).expect("empty RSS heatmap");
        let records = raw_ndjson_records(&bytes);
        assert_eq!(records[0]["summary"], "mean");
        assert_eq!(records[0]["entity_count"], 0);
        assert_eq!(records.len(), 3);
        assert!(records[1]["total"].is_null());
        assert!(records[2]["total"].is_null());
    }
}

#[test]
fn rss_mean_preserves_the_existing_duplicate_identity_refusal() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 10, "first"), (100, 101, 20, "second")]);
    fixture.finish();
    let target = "/api/heatmap?from=100&to=999&section=os_process&field=rmem_kb&columns=60&top=10";
    let error = query_response(&fixture, target, usize::MAX, usize::MAX)
        .expect_err("duplicate PID at a recorded timestamp");
    assert!(error.to_string().contains("non-unique identity"));
}

#[test]
fn other_process_gauges_keep_maximum_summaries() {
    let mut fixture = Fixture::new();
    append_rss_snapshots(&mut fixture);
    fixture.finish();
    for fields in ["field=num_threads", "field=rmem_kb&field=vmem_kb"] {
        let target =
            format!("/api/heatmap?from=100&to=999&section=os_process&{fields}&columns=60&top=10");
        let (_meta, bytes) = query_response(&fixture, &target, usize::MAX, usize::MAX)
            .expect("other process gauge heatmap");
        let records = raw_ndjson_records(&bytes);
        assert_eq!(records[0]["summary"], "max");
    }
}
