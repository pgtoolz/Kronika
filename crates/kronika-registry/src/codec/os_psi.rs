//! Type `1_107_001`: pressure stall information for one recorded OS scope.

use crate::{Section, Ts};

/// One resource's PSI counters from one host or cgroup pressure snapshot.
///
/// `full_*` fields are `None` for the `cpu` resource, which has no `full` line.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_107_001,
    name = "os_psi",
    semantics = snapshot_full,
    sort_key("resource", "ts"),
    identity("resource")
)]
pub struct OsPsi {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Resource: `0`=cpu, `1`=memory, `2`=io.
    #[column(l)]
    pub resource: u8,
    /// Fraction of time tasks stalled (some) over the last 10 s.
    #[column(g, unit = count)]
    pub some_avg10: f64,
    /// Fraction of time tasks stalled (some) over the last 60 s.
    #[column(g, unit = count)]
    pub some_avg60: f64,
    /// Fraction of time tasks stalled (some) over the last 300 s.
    #[column(g, unit = count)]
    pub some_avg300: f64,
    /// Cumulative stall time (some).
    #[column(c, unit = microseconds)]
    pub some_total: i64,
    /// Fraction of time tasks stalled (full) over the last 10 s. `None` for cpu.
    #[column(g, unit = count)]
    pub full_avg10: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 60 s. `None` for cpu.
    #[column(g, unit = count)]
    pub full_avg60: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 300 s. `None` for cpu.
    #[column(g, unit = count)]
    pub full_avg300: Option<f64>,
    /// Cumulative stall time (full), microseconds. `None` for cpu.
    #[column(c, unit = microseconds)]
    pub full_total: Option<i64>,
    /// Source scope (`0=host`, `3=container`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsPsi;
    use crate::{ColumnClass, Section, Ts, VerifiedSection, lint};

    fn cpu_row(ts: i64) -> OsPsi {
        OsPsi {
            ts: Ts(ts),
            resource: 0,
            some_avg10: 0.10,
            some_avg60: 0.05,
            some_avg300: 0.02,
            some_total: 10_000,
            full_avg10: None,
            full_avg60: None,
            full_avg300: None,
            full_total: None,
            scope: 0,
        }
    }

    fn memory_row(ts: i64) -> OsPsi {
        OsPsi {
            ts: Ts(ts),
            resource: 1,
            some_avg10: 1.50,
            some_avg60: 0.80,
            some_avg300: 0.30,
            some_total: 500_000,
            full_avg10: Some(0.20),
            full_avg60: Some(0.10),
            full_avg300: Some(0.05),
            full_total: Some(100_000),
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsPsi::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsPsi::CONTRACT;
        assert_eq!(c.type_id.get(), 1_107_001);
        assert_eq!(c.sort_key, ["resource", "ts"]);
        assert_eq!(c.identity, ["resource"]);
        assert_eq!(c.column("full_avg10").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("full_total").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("some_avg10").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("some_avg10").map(|col| col.class),
            Some(ColumnClass::Gauge)
        );
        assert_eq!(
            c.column("full_avg10").map(|col| col.class),
            Some(ColumnClass::Gauge)
        );
        assert_eq!(
            c.column("some_total").map(|col| col.class),
            Some(ColumnClass::Cumulative)
        );
        assert_eq!(c.column("scope").map(|col| col.nullable), Some(false));
    }

    #[test]
    fn encode_sorts_by_resource_then_ts() {
        let io_row = OsPsi {
            ts: Ts(1_000),
            resource: 2,
            some_avg10: 0.50,
            some_avg60: 0.25,
            some_avg300: 0.10,
            some_total: 200_000,
            full_avg10: Some(0.05),
            full_avg60: Some(0.02),
            full_avg300: Some(0.01),
            full_total: Some(20_000),
            scope: 0,
        };
        let bytes = OsPsi::encode(&[io_row, cpu_row(1_000), memory_row(1_000)]).expect("encode");
        let decoded = OsPsi::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded.iter().map(|r| r.resource).collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn nulls_survive_distinct_from_zero() {
        let bytes = OsPsi::encode(&[cpu_row(42)]).expect("encode");
        let decoded = OsPsi::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(decoded[0].full_avg10, None);
        assert_eq!(decoded[0].full_avg60, None);
        assert_eq!(decoded[0].full_avg300, None);
        assert_eq!(decoded[0].full_total, None);
        let bytes2 = OsPsi::encode(&[memory_row(43)]).expect("encode");
        let decoded2 = OsPsi::decode(VerifiedSection::for_test(bytes2.into())).expect("decode");
        assert_eq!(decoded2[0].full_avg10, Some(0.20));
        assert_eq!(decoded2[0].full_total, Some(100_000));
    }
}
