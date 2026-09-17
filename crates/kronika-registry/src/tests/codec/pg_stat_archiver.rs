use super::PgStatArchiver;
use crate::{Section, StrId, Ts};

fn row(ts: i64, with_archive: bool) -> PgStatArchiver {
    PgStatArchiver {
        ts: Ts(ts),
        archived_count: 100,
        last_archived_wal: with_archive.then_some(StrId(1)),
        last_archived_time: with_archive.then(|| Ts(ts - 1000)),
        failed_count: 2,
        last_failed_wal: None,
        last_failed_time: None,
        stats_reset: Some(Ts(ts - 100_000)),
    }
}

#[test]
fn contract_shape_matches_the_registry() {
    let c = PgStatArchiver::CONTRACT;
    assert_eq!(c.type_id.get(), 1_008_001);
    assert_eq!(c.columns.len(), 8);
    assert_eq!(c.sort_key, ["ts"]);
    assert_eq!(
        c.column("last_archived_wal").map(|col| col.nullable),
        Some(true)
    );
    assert_eq!(
        c.column("archived_count").map(|col| col.nullable),
        Some(false)
    );
}

#[test]
fn roundtrip_preserves_values_and_nulls() {
    crate::assert_roundtrips(&[row(5, false), row(1_000, true)]);
}
