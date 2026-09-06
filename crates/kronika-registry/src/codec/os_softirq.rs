//! Type `1_115_001`: per-type software interrupt counts from `/proc/softirqs`.

use crate::{Section, StrId, Ts};

/// One softirq vector (`TIMER`, `NET_RX`, `BLOCK`, ...), summed across CPUs.
///
/// The vector set is fixed by the kernel, so this section stays around ten
/// rows per snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_115_001,
    name = "os_softirq",
    semantics = snapshot_full,
    sort_key("vector", "ts"),
    identity("vector")
)]
pub struct OsSoftirq {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Softirq vector name as printed by the kernel.
    #[column(l)]
    pub vector: StrId,
    /// Softirqs of this vector since boot, summed across CPUs.
    #[column(c, unit = count)]
    pub count: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsSoftirq;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsSoftirq::CONTRACT]), Ok(()));
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[
            OsSoftirq {
                ts: Ts(1),
                vector: StrId(1),
                count: 10,
                scope: 0,
            },
            OsSoftirq {
                ts: Ts(1),
                vector: StrId(2),
                count: 20,
                scope: 0,
            },
        ]);
    }
}
