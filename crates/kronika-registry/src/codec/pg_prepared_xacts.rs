//! Type `1_010_001`: per-database aggregate over `pg_prepared_xacts`.
//!
//! `pg_prepared_xacts` lists two-phase-commit transactions (`PREPARE
//! TRANSACTION`) awaiting resolution — cluster-wide, one row per transaction,
//! each tagged with its database. This section aggregates them per database: how
//! many are prepared, how long the oldest has waited, and the highest XID age. A
//! forgotten 2PC pins the owning database's xmin horizon and blocks vacuum
//! there, so the database is the actionable label. No rows when nothing is
//! prepared (the default, since `max_prepared_transactions` is 0).

use crate::{Section, StrId, Ts};

/// Type `1_010_001`: per-database summary of `pg_prepared_xacts`.
///
/// One row per database that holds prepared transactions. `prepared_count` is
/// how many await commit or rollback; `max_age_us` is the oldest wall-clock age
/// in microseconds, and `max_xid_age_tx` is the highest transaction-id age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_010_001,
    name = "pg_prepared_xacts",
    semantics = snapshot_full,
    sort_key("datname", "ts"),
    identity("datname")
)]
pub struct PgPreparedXacts {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Database holding these prepared transactions.
    #[column(l)]
    pub datname: StrId,
    /// Prepared transactions in this database awaiting commit or rollback.
    #[column(g, unit = count)]
    pub prepared_count: i64,
    /// Wall-clock age of the oldest prepared transaction in this database,
    /// microseconds.
    #[column(g, unit = microseconds)]
    pub max_age_us: i64,
    /// Highest transaction-id age among prepared transactions in this database.
    #[column(g, unit = count)]
    pub max_xid_age_tx: i64,
}

#[cfg(test)]
mod tests {
    use super::PgPreparedXacts;
    use crate::{Section, StrId, Ts, VerifiedSection};

    fn row(ts: i64, datname: u64, count: i64, age_us: i64, xid_age_tx: i64) -> PgPreparedXacts {
        PgPreparedXacts {
            ts: Ts(ts),
            datname: StrId(datname),
            prepared_count: count,
            max_age_us: age_us,
            max_xid_age_tx: xid_age_tx,
        }
    }

    #[test]
    fn contract_shape_matches_the_source() {
        let c = PgPreparedXacts::CONTRACT;
        assert_eq!(c.type_id.get(), 1_010_001);
        assert_eq!(c.columns.len(), 5);
        assert_eq!(c.sort_key, ["datname", "ts"]);
        assert_eq!(c.identity, ["datname"]);
        assert_eq!(c.column("datname").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("prepared_count").map(|col| col.nullable),
            Some(false)
        );
        assert_eq!(c.column("max_age_us").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("max_xid_age_tx").map(|col| col.nullable),
            Some(false)
        );
    }

    #[test]
    fn roundtrip_preserves_per_database_rows() {
        crate::assert_roundtrips(&[
            row(1_000_000, 1, 3, 4_200_500_000, 120),
            row(1_000_000, 2, 1, 60_000_000, 8),
        ]);
    }

    #[test]
    fn encode_sorts_by_datname() {
        let bytes = PgPreparedXacts::encode(&[row(1_000, 9, 1, 10, 3), row(1_000, 2, 1, 20, 4)])
            .expect("encode");
        let decoded =
            PgPreparedXacts::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded.iter().map(|r| r.datname.0).collect::<Vec<_>>(),
            [2, 9]
        );
    }
}
