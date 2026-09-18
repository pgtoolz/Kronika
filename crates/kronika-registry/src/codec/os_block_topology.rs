//! Type `1_123_001`: exact Linux block-device edges from sysfs.

use crate::{Section, Ts};

/// One exact sysfs edge from a block device to the device directly beneath it:
/// a partition to its whole device, or a layered dm/LVM/MD device to one of the
/// devices it lists in `slaves/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_123_001,
    name = "os_block_topology",
    semantics = on_change,
    sort_key("major", "minor", "parent_major", "parent_minor", "ts"),
    identity("major", "minor", "parent_major", "parent_minor")
)]
pub struct OsBlockTopology {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Upper device major number.
    #[column(l)]
    pub major: i32,
    /// Upper device minor number.
    #[column(l)]
    pub minor: i32,
    /// Exact major number of the device beneath it.
    #[column(l)]
    pub parent_major: i32,
    /// Exact minor number of the device beneath it.
    #[column(l)]
    pub parent_minor: i32,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_block_topology.rs"]
mod tests;
