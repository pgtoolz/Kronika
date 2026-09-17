use super::{CachePolicy, Fixture, SEGMENT_ID, StatusCode, stream};
use serde_json::{Value, json};

fn neighbor(fixture: &Fixture, at: i64, direction: &str) -> Value {
    let target =
        format!("/api/snapshot/neighbor?section=pg_stat_activity&at={at}&direction={direction}");
    // Even a wildcard validator must not turn a live lookup into a cached 304.
    let prepared = fixture.prepare(&target, Some("*"));
    let metadata = prepared.meta();
    assert_eq!(metadata.status, StatusCode::OK);
    assert_eq!(metadata.cache, CachePolicy::NoStore);
    assert_eq!(metadata.cache.header(), "private,no-store");
    assert_eq!(metadata.etag, None);
    let records = stream(prepared).expect("neighbor body");
    assert_eq!(records.len(), 1);
    records.into_iter().next().expect("one neighbor record")
}

#[test]
fn an_absent_neighbor_is_rechecked_after_append_and_segment_rollover() {
    let mut fixture = Fixture::new();
    let first = SEGMENT_ID + 1_000_000;
    let second = first + 10_000_000;
    let third = first + 30_000_000;
    // The fixture records Activity 50 us after its OS metadata.
    fixture.append_postgres_health_with_interval(first, 1, 10);
    let empty = json!({"record": "snapshot_neighbor", "at": null, "segment_id": null});
    assert_eq!(neighbor(&fixture, first + 50, "next"), empty);

    fixture.append_postgres_health_with_interval(second, 2, 10);
    let second_sample = json!({
        "record": "snapshot_neighbor",
        "at": (second + 50).to_string(),
        "segment_id": SEGMENT_ID.to_string(),
    });
    assert_eq!(neighbor(&fixture, first + 50, "next"), second_sample);

    let next_segment = SEGMENT_ID + 20_000_000;
    fixture.finish_and_continue(next_segment);
    assert_eq!(neighbor(&fixture, first + 50, "next"), second_sample);
    assert_eq!(neighbor(&fixture, second + 50, "next"), empty);

    fixture.append_postgres_health_with_interval(third, 1, 10);
    assert_eq!(
        neighbor(&fixture, second + 50, "next"),
        json!({
            "record": "snapshot_neighbor",
            "at": (third + 50).to_string(),
            "segment_id": next_segment.to_string(),
        })
    );
    assert_eq!(neighbor(&fixture, third + 50, "previous"), second_sample);
}

#[test]
fn finished_navigation_is_uncached_without_changing_snapshot_validators() {
    let mut fixture = Fixture::new();
    let first = SEGMENT_ID + 1_000_000;
    let second = first + 10_000_000;
    fixture.append_postgres_health_with_interval(first, 1, 10);
    fixture.append_postgres_health_with_interval(second, 2, 10);
    fixture.finish_and_continue(SEGMENT_ID + 20_000_000);

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at={}&section=pg_stat_activity&field=pid",
        first + 50
    );
    let snapshot = fixture.prepare(&target, None);
    assert_eq!(snapshot.meta().cache, CachePolicy::Immutable);
    let etag = snapshot.meta().etag.expect("finished snapshot validator");
    let before = stream(snapshot).expect("finished snapshot rows");

    assert_eq!(
        neighbor(&fixture, first + 50, "next")["at"],
        (second + 50).to_string()
    );
    assert_eq!(neighbor(&fixture, second + 50, "next")["at"], Value::Null);
    fixture.append_postgres_health_with_interval(second + 20_000_000, 3, 10);
    assert_eq!(
        fixture.prepare(&target, Some(&etag)).meta().status,
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(
        stream(fixture.prepare(&target, None)).expect("same snapshot"),
        before
    );
}

#[tokio::test]
async fn neighbor_http_response_never_advertises_a_reusable_empty_result() {
    use http_body_util::BodyExt as _;
    use hyper::header::{CACHE_CONTROL, ETAG};

    let mut fixture = Fixture::new();
    let at = SEGMENT_ID + 1_000_000;
    fixture.append_postgres_health_with_interval(at, 1, 10);
    fixture.finish();
    let target = format!(
        "/api/snapshot/neighbor?section=pg_stat_activity&at={}&direction=next",
        at + 50
    );
    let prepared = fixture.prepare(&target, Some("*"));
    let response =
        crate::tests::stream_once(move || Ok(prepared), super::accepted("identity")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "private,no-store");
    assert!(!response.headers().contains_key(ETAG));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("neighbor body")
        .to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("neighbor JSON"),
        json!({"record": "snapshot_neighbor", "at": null, "segment_id": null})
    );
}

#[test]
fn latest_selection_uses_the_neighbor_sample_and_its_own_cache_validator() {
    let mut fixture = Fixture::new();
    let sample = SEGMENT_ID + 20_000_000;
    fixture.append_postgres_health_with_interval(sample, 1, 10);
    let newer_segment = SEGMENT_ID + 1;
    fixture.finish_and_continue(newer_segment);
    fixture.append_postgres_health_with_interval(sample - 10_000_000, 1, 10);
    fixture.append_postgres_health_with_interval(sample + 10_000_000, 1, 10);
    fixture.finish_and_continue(sample + 20_000_000);

    let found = neighbor(&fixture, sample - 2_000_000, "next");
    assert_eq!(found["at"], (sample + 50).to_string());
    assert_eq!(found["segment_id"], SEGMENT_ID.to_string());

    let target = format!(
        "/api/segments/{newer_segment}/snapshot?at={}&section=pg_stat_activity&field=pid",
        sample + 50
    );
    let anchored = fixture.prepare(&target, None);
    let old_etag = anchored.meta().etag.expect("anchor validator");
    let anchored_rows = stream(anchored).expect("anchor rows");
    let old_sample = (sample - 10_000_000 + 50).to_string();
    assert!(
        anchored_rows
            .iter()
            .any(|row| { row["record"] == "row" && row["timestamp"] == old_sample })
    );

    let target = format!("{target}&selection=latest");
    let latest = fixture.prepare(&target, Some(&old_etag));
    assert_eq!(latest.meta().status, StatusCode::OK);
    assert_eq!(latest.meta().cache, CachePolicy::Immutable);
    let latest_etag = latest.meta().etag.expect("latest validator");
    assert_ne!(latest_etag, old_etag);
    let latest_rows = stream(latest).expect("latest rows");
    let expected_sample = (sample + 50).to_string();
    assert!(
        latest_rows
            .iter()
            .any(|row| { row["record"] == "row" && row["timestamp"] == expected_sample })
    );
    assert_eq!(
        fixture.prepare(&target, Some(&latest_etag)).meta().status,
        StatusCode::NOT_MODIFIED
    );
}
