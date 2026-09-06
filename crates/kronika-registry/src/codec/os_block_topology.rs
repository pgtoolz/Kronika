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
mod tests {
    use super::OsBlockTopology;
    use crate::{Section, Ts, contract::lint};

    #[test]
    fn contract_is_an_exact_edge_identity() {
        assert_eq!(lint(&[OsBlockTopology::CONTRACT]), Ok(()));
        assert_eq!(
            OsBlockTopology::CONTRACT.identity,
            ["major", "minor", "parent_major", "parent_minor"]
        );
        crate::assert_roundtrips(&[OsBlockTopology {
            ts: Ts(10),
            major: 252,
            minor: 0,
            parent_major: 259,
            parent_minor: 4,
            scope: 0,
        }]);
    }
}
