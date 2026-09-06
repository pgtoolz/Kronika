use super::{
    PgLogAutovacuum, PgLogCheckpoints, PgLogErrors, PgLogLifecycle, PgLogLockWaits,
    PgLogSlowQueries, PgLogTempFiles,
};
use crate::{Section, StrId, Ts, VerifiedSection, lint};

const TS: i64 = 1_780_000_000_000_000;

fn error(ts: i64, severity: u8, pattern: u64) -> PgLogErrors {
    PgLogErrors {
        ts: Ts(ts),
        system_identifier: Some(7_000_000_000_000_000_001),
        source_file: StrId(9),
        severity,
        category: 9,
        sqlstate: Some(StrId(1)),
        pattern: StrId(pattern),
        count: 2,
        sample: StrId(3),
        detail: None,
        hint: None,
        context: None,
        statement: Some(StrId(4)),
        database: Some(StrId(5)),
        username: Some(StrId(6)),
    }
}

fn checkpoint(ts: i64, phase: u8) -> PgLogCheckpoints {
    PgLogCheckpoints {
        ts: Ts(ts),
        system_identifier: Some(7_000_000_000_000_000_001),
        source_file: StrId(9),
        phase,
        reason: Some(StrId(1)),
        seconds_apart: None,
        buffers_written: Some(3),
        write_ms: Some(201.0),
        sync_ms: Some(11.0),
        total_ms: Some(230.0),
        distance_kb: Some(512),
        estimate_kb: Some(1024),
        wal_added: Some(1),
        wal_removed: Some(2),
        wal_recycled: Some(3),
        sync_files: Some(4),
        longest_sync_ms: Some(8.0),
        average_sync_ms: Some(6.0),
    }
}

fn autovacuum(ts: i64, kind: u8) -> PgLogAutovacuum {
    PgLogAutovacuum {
        ts: Ts(ts),
        system_identifier: Some(7_000_000_000_000_000_001),
        source_file: StrId(9),
        kind,
        relation: Some(StrId(1)),
        index_scans: Some(1),
        pages_removed: Some(0),
        pages_remaining: Some(45),
        tuples_removed: Some(100),
        tuples_remaining: Some(200),
        tuples_dead_not_removable: Some(7),
        elapsed_ms: Some(50.0),
        buffer_hits: Some(90),
        buffer_misses: Some(3),
        buffer_dirtied: Some(5),
        avg_read_rate_mbs: Some(1.25),
        avg_write_rate_mbs: Some(2.5),
        cpu_user_ms: Some(10.0),
        cpu_system_ms: Some(20.0),
        wal_records: Some(12),
        wal_fpi: Some(3),
        wal_bytes: Some(4567),
    }
}

fn slow_query(ts: i64, pattern: u64) -> PgLogSlowQueries {
    PgLogSlowQueries {
        ts: Ts(ts),
        system_identifier: Some(7_000_000_000_000_000_001),
        source_file: StrId(9),
        pattern: StrId(pattern),
        sample: StrId(2),
        count: 4,
        max_duration_ms: 1234.567,
        total_duration_ms: 4321.0,
    }
}

fn lock_wait(ts: i64, kind: u8, pid: i32) -> PgLogLockWaits {
    PgLogLockWaits {
        ts: Ts(ts),
        system_identifier: Some(7_000_000_000_000_000_001),
        source_file: StrId(9),
        kind,
        pid: Some(pid),
        lock_mode: Some(StrId(1)),
        lock_target: Some(StrId(2)),
        duration_ms: Some(1000.123),
        holding_pids: Some(StrId(5)),
        wait_queue: Some(StrId(6)),
        detail: Some(StrId(3)),
        context: None,
        statement: Some(StrId(4)),
    }
}

fn lifecycle(ts: i64, kind: u8) -> PgLogLifecycle {
    PgLogLifecycle {
        ts: Ts(ts),
        system_identifier: Some(7_000_000_000_000_000_001),
        source_file: StrId(9),
        kind,
        pid: Some(4242),
        signal: Some(9),
        shutdown_mode: None,
        message: StrId(1),
        query_detail: Some(StrId(2)),
    }
}

fn temp_file(ts: i64, size_bytes: i64) -> PgLogTempFiles {
    PgLogTempFiles {
        ts: Ts(ts),
        system_identifier: Some(7_000_000_000_000_000_001),
        source_file: StrId(9),
        path: Some(StrId(1)),
        size_bytes,
        statement: Some(StrId(2)),
    }
}

#[test]
fn contracts_pass_the_linter() {
    assert_eq!(
        lint(&[
            PgLogErrors::CONTRACT,
            PgLogCheckpoints::CONTRACT,
            PgLogAutovacuum::CONTRACT,
            PgLogSlowQueries::CONTRACT,
            PgLogLockWaits::CONTRACT,
            PgLogLifecycle::CONTRACT,
            PgLogTempFiles::CONTRACT,
        ]),
        Ok(())
    );
}

#[test]
fn contract_shapes() {
    let errors = PgLogErrors::CONTRACT;
    assert_eq!(errors.type_id.get(), 2_001_001);
    assert_eq!(errors.columns.len(), 15);
    assert_eq!(errors.sort_key, ["severity", "category", "pattern", "ts"]);
    assert_eq!(
        errors.column("pattern").map(|column| column.nullable),
        Some(false)
    );
    assert_eq!(
        errors.column("database").map(|column| column.nullable),
        Some(true)
    );

    assert_eq!(PgLogCheckpoints::CONTRACT.columns.len(), 18);
    assert_eq!(PgLogAutovacuum::CONTRACT.columns.len(), 22);
    assert_eq!(PgLogSlowQueries::CONTRACT.columns.len(), 8);
    assert_eq!(PgLogLockWaits::CONTRACT.columns.len(), 13);
    assert_eq!(PgLogLifecycle::CONTRACT.columns.len(), 9);
    assert_eq!(PgLogTempFiles::CONTRACT.columns.len(), 6);
}

#[test]
fn every_section_survives_a_roundtrip() {
    crate::assert_roundtrips(&[error(TS, 0, 10), error(TS, 1, 11)]);
    crate::assert_roundtrips(&[checkpoint(TS, 0), checkpoint(TS + 1, 1)]);
    crate::assert_roundtrips(&[autovacuum(TS, 0), autovacuum(TS + 1, 1)]);
    crate::assert_roundtrips(&[slow_query(TS, 10), slow_query(TS + 1, 11)]);
    crate::assert_roundtrips(&[lock_wait(TS, 0, 1), lock_wait(TS, 1, 2)]);
    crate::assert_roundtrips(&[lifecycle(TS, 0), lifecycle(TS + 1, 1)]);
    crate::assert_roundtrips(&[temp_file(TS, 1_048_576), temp_file(TS + 1, 2_097_152)]);
}

#[test]
fn a_record_shape_that_printed_no_numbers_keeps_them_missing() {
    let bare = PgLogCheckpoints {
        reason: None,
        buffers_written: None,
        write_ms: None,
        sync_ms: None,
        total_ms: None,
        distance_kb: None,
        estimate_kb: None,
        wal_added: None,
        wal_removed: None,
        wal_recycled: None,
        sync_files: None,
        longest_sync_ms: None,
        average_sync_ms: None,
        seconds_apart: Some(9),
        ..checkpoint(TS, 2)
    };

    let bytes = PgLogCheckpoints::encode(&[bare]).expect("encode");
    let decoded =
        PgLogCheckpoints::decode(VerifiedSection::for_test(bytes.into())).expect("decode");

    assert_eq!(decoded[0], bare);
    assert_eq!(decoded[0].buffers_written, None);
    assert_eq!(decoded[0].seconds_apart, Some(9));
}

#[test]
fn errors_sort_by_severity_before_time() {
    let rows = [error(TS + 10, 1, 11), error(TS, 0, 10), error(TS, 0, 9)];

    let bytes = PgLogErrors::encode(&rows).expect("encode");
    let decoded = PgLogErrors::decode(VerifiedSection::for_test(bytes.into())).expect("decode");

    assert_eq!(
        decoded
            .iter()
            .map(|row| (row.severity, row.pattern.0))
            .collect::<Vec<_>>(),
        [(0, 9), (0, 10), (1, 11)]
    );
}
