//! Type `1_114_001`: per-IRQ interrupt counts from `/proc/interrupts`.

use crate::{Section, StrId, Ts};

/// One hardware or synthetic interrupt line, summed across CPUs.
///
/// The per-CPU breakdown is deliberately not stored: it multiplies the row
/// count by the CPU count for a number that only matters when chasing IRQ
/// affinity, and the aggregate is what an operator reads first. `device` is
/// the trailing free-text description; it is absent for the synthetic lines
/// (`NMI`, `LOC`, `RES`, ...) that carry no device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_114_001,
    name = "os_interrupts",
    semantics = snapshot_full,
    sort_key("irq", "ts"),
    identity("irq")
)]
pub struct OsInterrupts {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// IRQ name as printed in the first column: a number or a symbolic name.
    #[column(l)]
    pub irq: StrId,
    /// Trailing device description; `None` for lines that carry none.
    #[column(l)]
    pub device: Option<StrId>,
    /// Interrupts on this line since boot, summed across CPUs.
    #[column(c, unit = count)]
    pub count: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_interrupts.rs"]
mod tests;
