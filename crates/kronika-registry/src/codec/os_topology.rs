//! Type `1_113_001`: CPU topology from `/proc/cpuinfo` and sysfs.

use crate::{Section, StrId, Ts};

/// One logical CPU's topology facts from `/proc/cpuinfo` and sysfs.
///
/// Emitted `on_change`; one row per logical CPU per collection segment.
/// `mhz_max` is `None` when sysfs does not expose a max-frequency value.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_113_001,
    name = "os_topology",
    semantics = on_change,
    sort_key("cpu_id", "ts"),
    identity("cpu_id")
)]
pub struct OsTopology {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Logical CPU index (`processor` field in `/proc/cpuinfo`).
    #[column(l)]
    pub cpu_id: i32,
    /// CPU model string, as a string dictionary reference.
    #[column(l)]
    pub model_name: StrId,
    /// Maximum clock frequency in MHz from sysfs; `None` when unavailable.
    #[column(l)]
    pub mhz_max: Option<f64>,
    /// Physical core within the socket (`core id`); `-1` when absent.
    #[column(l)]
    pub core_id: i32,
    /// Physical socket (`physical id`); `-1` when absent.
    #[column(l)]
    pub socket_id: i32,
    /// NUMA node this CPU belongs to; `-1` when sysfs exposes no node.
    #[column(l)]
    pub numa_node: i32,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_topology.rs"]
mod tests;
