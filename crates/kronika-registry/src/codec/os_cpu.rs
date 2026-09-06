//! Type `1_102_001`: CPU time from `/proc/stat` `cpu`/`cpuN` lines.

use crate::{Section, Ts};

/// One CPU's cumulative scheduler ticks.
///
/// The aggregate `cpu` line uses `cpu_id = -1`; per-cpu lines carry their
/// index. All time fields are raw scheduler ticks — the reader converts
/// through `clock_ticks_per_sec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_102_001,
    name = "os_cpu",
    semantics = snapshot_full,
    sort_key("cpu_id", "ts"),
    identity("cpu_id")
)]
pub struct OsCpu {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// `-1` for the aggregate `cpu` line, else the CPU index.
    #[column(l)]
    pub cpu_id: i32,
    /// Ticks in user mode.
    #[column(c, unit = jiffies)]
    pub user: i64,
    /// Ticks in user mode with low priority (nice).
    #[column(c, unit = jiffies)]
    pub nice: i64,
    /// Ticks in system (kernel) mode.
    #[column(c, unit = jiffies)]
    pub system: i64,
    /// Ticks idle.
    #[column(c, unit = jiffies)]
    pub idle: i64,
    /// Ticks waiting for I/O to complete.
    #[column(c, unit = jiffies)]
    pub iowait: i64,
    /// Ticks serving hardware interrupts.
    #[column(c, unit = jiffies)]
    pub irq: i64,
    /// Ticks serving software interrupts.
    #[column(c, unit = jiffies)]
    pub softirq: i64,
    /// Ticks stolen by a hypervisor.
    #[column(c, unit = jiffies)]
    pub steal: i64,
    /// Ticks spent running a virtual CPU for a guest OS.
    #[column(c, unit = jiffies)]
    pub guest: i64,
    /// Ticks spent running a niced guest.
    #[column(c, unit = jiffies)]
    pub guest_nice: i64,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsCpu;
    use crate::{Section, Ts, contract::lint};

    fn row(ts: i64, cpu_id: i32) -> OsCpu {
        OsCpu {
            ts: Ts(ts),
            cpu_id,
            user: 1,
            nice: 2,
            system: 3,
            idle: 4,
            iowait: 5,
            irq: 6,
            softirq: 7,
            steal: 8,
            guest: 9,
            guest_nice: 10,
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsCpu::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsCpu::CONTRACT;
        assert_eq!(c.type_id.get(), 1_102_001);
        assert_eq!(c.sort_key, ["cpu_id", "ts"]);
        assert_eq!(c.identity, ["cpu_id"]);
    }

    #[test]
    fn encode_sorts_by_cpu_id_then_ts() {
        let bytes = OsCpu::encode(&[row(1_000, 3), row(1_000, -1), row(1_000, 1)]).expect("encode");
        let decoded = OsCpu::decode(kronika_registry::VerifiedSection::for_test(bytes.into()))
            .expect("decode");
        assert_eq!(
            decoded.iter().map(|r| r.cpu_id).collect::<Vec<_>>(),
            [-1, 1, 3]
        );
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[row(1_000, -1), row(1_000, 0)]);
    }
}
