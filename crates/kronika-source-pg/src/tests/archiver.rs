use super::{ArchiverRow, to_archiver};
use crate::tests::intern as fake_intern;

#[derive(Clone, Copy)]
enum WalSamples {
    Both,
    Neither,
    FailedOnly,
}

fn sample_row(samples: WalSamples) -> ArchiverRow {
    let (archived, failed) = match samples {
        WalSamples::Both => (true, true),
        WalSamples::Neither => (false, false),
        WalSamples::FailedOnly => (false, true),
    };
    ArchiverRow {
        ts: 2_000,
        archived_count: 100,
        last_archived_wal: archived.then(|| "000000010000000000000005".to_owned()),
        last_archived_time: archived.then_some(1_500),
        failed_count: 2,
        last_failed_wal: failed.then(|| "000000010000000000000006".to_owned()),
        last_failed_time: failed.then_some(1_750),
        stats_reset: Some(1_000),
    }
}

#[test]
fn interns_wal_names_and_maps_times() {
    let r = to_archiver(&sample_row(WalSamples::Both), fake_intern).expect("intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.archived_count, 100);
    assert_eq!(
        r.last_archived_wal,
        Some(fake_intern(b"000000010000000000000005").unwrap())
    );
    assert_eq!(r.last_archived_time.map(|t| t.0), Some(1_500));
    assert_eq!(
        r.last_failed_wal,
        Some(fake_intern(b"000000010000000000000006").unwrap())
    );
    assert_eq!(r.last_failed_time.map(|t| t.0), Some(1_750));
}

#[test]
fn handles_null_wal_names() {
    let r = to_archiver(&sample_row(WalSamples::Neither), fake_intern).expect("intern");
    assert_eq!(r.last_archived_wal, None);
    assert_eq!(r.last_archived_time, None);
    assert_eq!(r.last_failed_wal, None);
    assert_eq!(r.last_failed_time, None);
}

#[test]
fn intern_failure_propagates() {
    assert_eq!(
        to_archiver(&sample_row(WalSamples::FailedOnly), |_| Err("full")),
        Err("full")
    );
}
