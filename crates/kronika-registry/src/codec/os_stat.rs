//! Type `1_103_001`: misc counters from `/proc/stat`.

use crate::{Section, Ts};

/// Miscellaneous kernel counters from the `/proc/stat` singleton lines.
///
/// Collected once per snapshot; `btime` is unix microseconds from the
/// `secs * 1_000_000` conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_103_001,
    name = "os_stat",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsStat {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Context switches since boot.
    #[column(c, unit = count)]
    pub ctxt: i64,
    /// Processes forked since boot.
    #[column(c, unit = count)]
    pub processes: i64,
    /// Processes in runnable state at collection time.
    #[column(g, unit = count)]
    pub procs_running: i64,
    /// Processes blocked waiting for I/O at collection time.
    #[column(g, unit = count)]
    pub procs_blocked: i64,
    /// Kernel boot time, unix microseconds.
    #[column(g, unit = microseconds)]
    pub btime: Ts,
    /// Hardware interrupts serviced since boot, all lines summed.
    #[column(c, unit = count)]
    pub intr_total: Option<i64>,
    /// Software interrupts serviced since boot, all vectors summed.
    #[column(c, unit = count)]
    pub softirq_total: Option<i64>,
    /// Seconds since boot (`/proc/uptime` field 1).
    #[column(g, unit = microseconds)]
    pub uptime_us: Option<i64>,
    /// Cumulative idle time of all cores (`/proc/uptime` field 2),
    /// microseconds.
    #[column(g, unit = microseconds)]
    pub idle_us: Option<i64>,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsStat;
    use crate::{Section, Ts, contract::lint};

    fn row(ts: i64) -> OsStat {
        OsStat {
            ts: Ts(ts),
            ctxt: 1_234_567,
            processes: 42,
            procs_running: 3,
            procs_blocked: 1,
            btime: Ts(1_700_000_000_000_000),
            intr_total: Some(9_000_000),
            softirq_total: Some(8_000_000),
            uptime_us: Some(3_600_000_000),
            idle_us: Some(28_000_000_000),
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsStat::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsStat::CONTRACT;
        assert_eq!(c.type_id.get(), 1_103_001);
        assert_eq!(c.sort_key, ["ts"]);
    }

    #[test]
    fn encode_sorts_by_ts() {
        let bytes = OsStat::encode(&[row(2_000), row(1_000), row(3_000)]).expect("encode");
        let decoded = OsStat::decode(kronika_registry::VerifiedSection::for_test(bytes.into()))
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
