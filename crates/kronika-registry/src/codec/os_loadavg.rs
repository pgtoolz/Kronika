//! Type `1_105_001`: system load averages from `/proc/loadavg`.

use crate::{Section, Ts};

/// System load averages from `/proc/loadavg`.
///
/// One row per snapshot; `running`/`total` are the process counts from the
/// `running/total` token.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_105_001,
    name = "os_loadavg",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsLoadavg {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// 1-minute load average.
    #[column(g, unit = none)]
    pub load1: f64,
    /// 5-minute load average.
    #[column(g, unit = none)]
    pub load5: f64,
    /// 15-minute load average.
    #[column(g, unit = none)]
    pub load15: f64,
    /// Runnable processes at collection time.
    #[column(g, unit = count)]
    pub running: i32,
    /// Total threads/processes at collection time.
    #[column(g, unit = count)]
    pub total: i32,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsLoadavg;
    use crate::{Section, Ts, contract::lint};

    fn row(ts: i64) -> OsLoadavg {
        OsLoadavg {
            ts: Ts(ts),
            load1: 0.15,
            load5: 0.10,
            load15: 0.05,
            running: 2,
            total: 345,
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsLoadavg::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsLoadavg::CONTRACT;
        assert_eq!(c.type_id.get(), 1_105_001);
        assert_eq!(c.sort_key, ["ts"]);
    }

    #[test]
    fn encode_sorts_by_ts() {
        let bytes = OsLoadavg::encode(&[row(2_000), row(1_000), row(3_000)]).expect("encode");
        let decoded = OsLoadavg::decode(kronika_registry::VerifiedSection::for_test(bytes.into()))
            .expect("decode");
        assert_eq!(
            decoded.iter().map(|r| r.ts.0).collect::<Vec<_>>(),
            [1_000, 2_000, 3_000]
        );
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[row(1_000), row(2_000)]);
    }
}
