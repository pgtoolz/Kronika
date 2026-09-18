use super::{
    BATCH_LOGICAL_BYTES, BATCH_ROWS, Batch, BatchError, PendingBatch, QueryStats,
    batch_limit_reached, deliver_batch, extend_fetch_deadline,
};
use std::time::Duration;

#[test]
fn batch_limits_cover_both_rows_and_payload() {
    assert!(!batch_limit_reached(
        BATCH_ROWS - 1,
        BATCH_LOGICAL_BYTES - 1
    ));
    assert!(batch_limit_reached(BATCH_ROWS, 1));
    assert!(batch_limit_reached(1, BATCH_LOGICAL_BYTES));
}

#[test]
fn batching_preserves_order_across_a_row_boundary() {
    let mut pending = PendingBatch::new();
    let mut first = None;
    for row in 0..=BATCH_ROWS {
        if let Some(batch) = pending.push(row, 1) {
            assert!(first.replace(batch).is_none(), "only one full batch");
        }
    }
    let first = first.expect("the row bound emits a batch");
    let last = pending.finish().expect("one row remains");
    assert_eq!(first.rows, (0..BATCH_ROWS).collect::<Vec<_>>());
    assert_eq!(first.logical_bytes, BATCH_ROWS);
    assert_eq!(last.rows, vec![BATCH_ROWS]);
    assert_eq!(last.logical_bytes, 1);
}

#[test]
fn one_oversized_row_is_delivered_as_its_own_batch() {
    let mut pending = PendingBatch::new();
    let batch = pending
        .push("large", BATCH_LOGICAL_BYTES + 1)
        .expect("the payload bound emits immediately");
    assert_eq!(batch.rows, ["large"]);
    assert_eq!(batch.logical_bytes, BATCH_LOGICAL_BYTES + 1);
    assert!(pending.finish().is_none());
}

#[test]
fn payload_boundary_row_is_delivered_before_another_row_is_fetched() {
    let mut pending = PendingBatch::new();
    assert!(pending.push("first", BATCH_LOGICAL_BYTES - 1).is_none());
    let batch = pending
        .push("boundary", 2)
        .expect("the boundary row completes the current batch");
    assert_eq!(batch.rows, ["first", "boundary"]);
    assert_eq!(batch.logical_bytes, BATCH_LOGICAL_BYTES + 1);
    assert!(pending.finish().is_none());
}

#[test]
fn decoded_payload_counts_toward_telemetry_and_the_batch_limit() {
    let mut stats = QueryStats::default();
    let bytes = stats.add_decoded_payload(8, BATCH_LOGICAL_BYTES);
    let mut pending = PendingBatch::new();
    let batch = pending
        .push("locks", bytes)
        .expect("the decoded payload reaches the byte target");

    assert_eq!(batch.logical_bytes, BATCH_LOGICAL_BYTES + 8);
    assert_eq!(
        stats.application_payload_from_postgres_bytes,
        u64::try_from(BATCH_LOGICAL_BYTES).expect("the target fits u64")
    );
}

#[test]
fn a_failed_sink_attempt_counts_without_claiming_written_bytes() {
    let mut stats = QueryStats::default();
    let result = deliver_batch(
        Batch {
            rows: vec![1],
            logical_bytes: 8,
        },
        &mut stats,
        &mut |_batch| {
            std::thread::sleep(Duration::from_millis(1));
            Err("journal full")
        },
    );
    assert!(matches!(result, Err(BatchError::Sink("journal full"))));
    assert_eq!(stats.batches, 1);
    assert_eq!(
        stats.fetch_elapsed(Duration::from_millis(1)),
        Duration::ZERO
    );
    assert_eq!(stats.encode_elapsed, Duration::ZERO);
    assert_eq!(stats.append_elapsed, Duration::ZERO);
    assert_eq!(stats.encoded_bytes, 0);
    assert_eq!(stats.wal_bytes_appended, 0);
}

#[test]
fn synchronous_sink_time_extends_the_fetch_deadline() {
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let before = deadline;
    extend_fetch_deadline(&mut deadline, Duration::from_secs(7));
    assert_eq!(deadline.duration_since(before), Duration::from_secs(7));
}
