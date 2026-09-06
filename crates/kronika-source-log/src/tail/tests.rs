use std::io::Write as _;

use super::{Continues, MAX_LINE_BYTES, Position, Record, Tail, TailBatch};
use crate::postgres::PgLog;

/// Every line opens its own record.
fn never(_open: &[String], _line: &str, _raw_quotes_odd: bool) -> bool {
    false
}

/// A tab continues the line before it, as `PgBouncer` and wrapped `stderr`
/// messages are written.
fn tabbed(_open: &[String], line: &str, _raw_quotes_odd: bool) -> bool {
    line.starts_with('\t')
}

/// CSV remains open while its raw quote count is odd.
const fn quoted(_open: &[String], _line: &str, raw_quotes_odd: bool) -> bool {
    raw_quotes_odd
}

fn write(path: &std::path::Path, text: &str) {
    std::fs::write(path, text).expect("write the log");
}

fn append(path: &std::path::Path, text: &str) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open the log");
    file.write_all(text.as_bytes()).expect("append to the log");
}

fn read(tail: &mut Tail, continues: Continues) -> TailBatch {
    tail.read_batch(continues, 1024).expect("read the log")
}

fn acknowledge(tail: &mut Tail, batch: &TailBatch) {
    if batch.needs_ack {
        tail.acknowledge().expect("acknowledge the batch");
    }
}

fn texts(records: &[Record]) -> Vec<String> {
    records.iter().map(Record::joined).collect()
}

#[test]
fn reads_only_what_arrived_since_the_last_acknowledged_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "first\n");
    let mut tail = Tail::new(path.clone(), Position::default());

    assert!(read(&mut tail, never).records.is_empty());
    append(&path, "second\n");
    let first = read(&mut tail, never);
    assert_eq!(texts(&first.records), ["first"]);
    acknowledge(&mut tail, &first);

    append(&path, "third\n");
    let second = read(&mut tail, never);
    assert_eq!(texts(&second.records), ["second"]);
    acknowledge(&mut tail, &second);

    let third = read(&mut tail, never);
    assert_eq!(texts(&third.records), ["third"]);
    acknowledge(&mut tail, &third);
    assert!(read(&mut tail, never).records.is_empty());
}

#[test]
fn a_line_without_its_newline_waits_for_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "half");
    let mut tail = Tail::new(path.clone(), Position::default());

    assert!(read(&mut tail, never).records.is_empty());
    assert_eq!(tail.position().offset, 0);
    append(&path, " and half\nnext\n");
    let completed = read(&mut tail, never);
    assert_eq!(texts(&completed.records), ["half and half"]);
    acknowledge(&mut tail, &completed);
}

#[test]
fn a_truncated_file_resets_volatile_state_and_is_read_from_its_start() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "a long line that is later cut away\nnext\n");
    let mut tail = Tail::new(path.clone(), Position::default());
    let before = read(&mut tail, never);
    acknowledge(&mut tail, &before);

    write(&path, "after\nsentinel\n");

    let after = read(&mut tail, never);
    assert_eq!(texts(&after.records), ["after"]);
}

#[test]
fn a_rotated_file_resets_volatile_state_and_is_read_from_its_start() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "before rotation\nnext\n");
    let mut tail = Tail::new(path.clone(), Position::default());
    let before = read(&mut tail, never);
    acknowledge(&mut tail, &before);

    std::fs::rename(&path, dir.path().join("postgresql.log.1")).expect("rotate");
    write(&path, "after rotation\nsentinel\n");

    let after = read(&mut tail, never);
    assert_eq!(texts(&after.records), ["after rotation"]);
}

#[test]
fn an_over_long_line_keeps_its_prefix_and_the_next_one_survives() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    let long = "x".repeat(MAX_LINE_BYTES + 4096);
    write(&path, &format!("{long}\nshort\nsentinel\n"));
    let mut tail = Tail::new(path, Position::default());

    let batch = read(&mut tail, never);

    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.records[0].first().len(), MAX_LINE_BYTES);
    assert!(batch.records[0].truncated());
    assert_eq!(batch.records[1].first(), "short");
}

#[test]
fn a_long_unfinished_line_keeps_scanning_without_rereading_or_emitting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    let long = "x".repeat(MAX_LINE_BYTES + 32_768);
    write(&path, &long);
    let mut tail = Tail::new(path.clone(), Position::default());

    for expected_scan in [32_768_u64, 65_536, 98_304] {
        let batch = tail
            .read_batch_with_limit(never, 8, 32_768)
            .expect("scan unfinished line");
        assert!(batch.records.is_empty());
        assert!(!batch.needs_ack);
        assert_eq!(tail.position().offset, 0);
        assert_eq!(tail.scan_offset, expected_scan);
    }

    append(&path, "\nsuccessor\nsentinel\n");
    let completed = read(&mut tail, never);
    assert_eq!(completed.records[0].first().len(), MAX_LINE_BYTES);
    assert!(completed.records[0].truncated());
    assert_eq!(completed.records[1].first(), "successor");
}

#[test]
fn a_restart_mid_discard_rescans_from_the_unacknowledged_line_start() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    let long = "x".repeat(MAX_LINE_BYTES + 8192);
    write(&path, &long);
    let mut first = Tail::new(path.clone(), Position::default());

    let partial = first
        .read_batch_with_limit(never, 8, MAX_LINE_BYTES + 4096)
        .expect("scan into discarded suffix");
    assert!(partial.records.is_empty());
    assert_eq!(first.position().offset, 0);

    append(&path, "\nsuccessor\nsentinel\n");
    let mut restarted = Tail::new(path, first.position());
    let batch = read(&mut restarted, never);

    assert_eq!(batch.records[0].first().len(), MAX_LINE_BYTES);
    assert!(batch.records[0].truncated());
    assert_eq!(batch.records[1].first(), "successor");
}

#[test]
fn a_line_that_is_not_utf8_does_not_hide_the_next_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    std::fs::write(&path, b"bad \xff\xfe line\ngood line\nsentinel\n").expect("write");
    let mut tail = Tail::new(path, Position::default());

    let batch = read(&mut tail, never);
    assert_eq!(texts(&batch.records), ["bad ", "good line"]);
    assert!(batch.records[0].truncated());
}

#[test]
fn continuations_join_the_line_they_belong_to() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    write(&path, "opening\n\tcontinued\n\tagain\nnext\n");
    let mut tail = Tail::new(path, Position::default());

    let first = read(&mut tail, tabbed);
    assert_eq!(texts(&first.records), ["opening\n\tcontinued\n\tagain"]);
    acknowledge(&mut tail, &first);
}

#[test]
fn an_unfinished_continuation_does_not_flush_the_preceding_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    write(&path, "opening\n");
    let mut tail = Tail::new(path.clone(), Position::default());

    assert!(read(&mut tail, tabbed).records.is_empty());
    append(&path, "\tpart");
    assert!(read(&mut tail, tabbed).records.is_empty());
    assert_eq!(tail.position().offset, 0);

    append(&path, "ial\nnext\n");
    let completed = read(&mut tail, tabbed);
    assert_eq!(texts(&completed.records), ["opening\n\tpartial"]);
}

#[test]
fn a_newline_complete_record_is_emitted_on_the_next_idle_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    write(&path, "alone\n");
    let mut tail = Tail::new(path, Position::default());

    assert!(read(&mut tail, tabbed).records.is_empty());
    let idle = read(&mut tail, tabbed);
    assert_eq!(texts(&idle.records), ["alone"]);
    assert!(idle.needs_ack);
    acknowledge(&mut tail, &idle);
    assert_eq!(tail.position().offset, "alone\n".len() as u64);
}

#[test]
fn text_records_can_skip_csv_quote_tracking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pgbouncer.log");
    write(&path, "a \"quote\nb plain\nsentinel\n");
    let mut tail = Tail::new(path, Position::default());

    let batch = tail
        .read_batch_without_quote_tracking(never, 8)
        .expect("read text records");

    assert_eq!(texts(&batch.records), ["a \"quote", "b plain"]);
}

#[test]
fn an_open_csv_quote_is_not_flushed_by_an_idle_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.csv");
    write(&path, "\"open\n");
    let mut tail = Tail::new(path.clone(), Position::default());

    assert!(read(&mut tail, quoted).records.is_empty());
    assert!(read(&mut tail, quoted).records.is_empty());
    assert_eq!(tail.position().offset, 0);

    append(&path, "close\"\nnext\n");
    let completed = read(&mut tail, quoted);
    assert_eq!(texts(&completed.records), ["\"open\nclose\""]);
}

#[test]
fn committed_position_changes_only_after_acknowledgement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "first\nsecond\n");
    let mut tail = Tail::new(path, Position::default());

    let batch = read(&mut tail, never);
    assert_eq!(texts(&batch.records), ["first"]);
    assert!(batch.needs_ack);
    assert_eq!(tail.position().offset, 0);
    assert!(tail.read_batch(never, 8).is_err());

    let committed = tail.acknowledge().expect("acknowledge");
    assert_eq!(committed.offset, "first\n".len() as u64);
    assert_eq!(tail.position(), committed);
}

#[test]
fn retry_rescans_the_same_unacknowledged_batch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "first\nsecond\n");
    let mut tail = Tail::new(path, Position::default());

    let first_attempt = read(&mut tail, never);
    assert_eq!(texts(&first_attempt.records), ["first"]);
    assert_eq!(tail.position().offset, 0);

    tail.retry();
    let second_attempt = read(&mut tail, never);
    assert_eq!(texts(&second_attempt.records), ["first"]);
    assert_eq!(tail.position().offset, 0);
}

#[test]
fn record_cap_stages_the_next_line_without_advancing_the_candidate_past_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql.log");
    write(&path, "one\ntwo\nthree\n");
    let mut tail = Tail::new(path, Position::default());

    let one = tail.read_batch(never, 1).expect("first batch");
    assert_eq!(texts(&one.records), ["one"]);
    assert_eq!(tail.acknowledge().expect("ack one").offset, 4);

    let two = tail.read_batch(never, 1).expect("second batch");
    assert_eq!(texts(&two.records), ["two"]);
    assert_eq!(tail.acknowledge().expect("ack two").offset, 8);

    assert!(
        tail.read_batch(never, 1)
            .expect("stage last line")
            .records
            .is_empty()
    );
    append(tail.path(), "four\n");
    let three = tail.read_batch(never, 1).expect("flush last line");
    assert_eq!(texts(&three.records), ["three"]);
}

#[test]
fn csv_quote_closing_beyond_the_line_prefix_does_not_poison_the_successor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql-csvlog.csv");
    let oversized = format!("\"{}\"", "x".repeat(MAX_LINE_BYTES + 128));
    let valid = valid_csv("valid successor");
    write(&path, &format!("{oversized}\n{valid}\nignored\n"));
    let mut log = PgLog::new(path, Position::default(), None);

    let batch = log.read_batch(0, 16).expect("read csvlog");

    assert_eq!(batch.events.errors.len(), 1);
    assert_eq!(batch.events.errors[0].sample, "valid successor");
}

#[test]
fn truncated_multiline_csv_resynchronizes_at_its_raw_quote_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("postgresql-csvlog.csv");
    let oversized_open = format!("\"{}", "x".repeat(MAX_LINE_BYTES + 128));
    let valid = valid_csv("after multiline");
    write(
        &path,
        &format!("{oversized_open}\nstill discarded\"\n{valid}\nignored\n"),
    );
    let mut log = PgLog::new(path, Position::default(), None);

    let batch = log.read_batch(0, 16).expect("read csvlog");

    assert_eq!(batch.events.errors.len(), 1);
    assert_eq!(batch.events.errors[0].sample, "after multiline");
}

fn valid_csv(message: &str) -> String {
    format!(
        "2026-08-07 12:34:56.789 MSK,alice,shop,12345,10.0.0.1:53124,session,1,SELECT,2026-08-07 12:34:00.000 MSK,3/15,0,ERROR,42P01,\"{message}\",,,,,,select 1,0,,psql"
    )
}
