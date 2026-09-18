//! Type `1_109_001`: per-interface network counters from `/proc/net/dev`.

use crate::{Section, StrId, Ts};

/// Per-interface network I/O counters from one `/proc/net/dev` line.
///
/// All 16 counter columns are cumulative; none are gauges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_109_001,
    name = "os_netdev",
    semantics = snapshot_full,
    sort_key("iface", "ts"),
    identity("iface")
)]
pub struct OsNetdev {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Interface name (e.g. `eth0`, `lo`), as a string dictionary reference.
    #[column(l)]
    pub iface: StrId,
    /// Bytes received.
    #[column(c, unit = bytes)]
    pub rx_bytes: i64,
    /// Packets received.
    #[column(c, unit = count)]
    pub rx_packets: i64,
    /// Receive errors.
    #[column(c, unit = count)]
    pub rx_errs: i64,
    /// Receive drops.
    #[column(c, unit = count)]
    pub rx_drop: i64,
    /// Receive FIFO errors.
    #[column(c, unit = count)]
    pub rx_fifo: i64,
    /// Receive frame errors.
    #[column(c, unit = count)]
    pub rx_frame: i64,
    /// Compressed packets received.
    #[column(c, unit = count)]
    pub rx_compressed: i64,
    /// Multicast frames received.
    #[column(c, unit = count)]
    pub rx_multicast: i64,
    /// Bytes transmitted.
    #[column(c, unit = bytes)]
    pub tx_bytes: i64,
    /// Packets transmitted.
    #[column(c, unit = count)]
    pub tx_packets: i64,
    /// Transmit errors.
    #[column(c, unit = count)]
    pub tx_errs: i64,
    /// Transmit drops.
    #[column(c, unit = count)]
    pub tx_drop: i64,
    /// Transmit FIFO errors.
    #[column(c, unit = count)]
    pub tx_fifo: i64,
    /// Collisions.
    #[column(c, unit = count)]
    pub tx_colls: i64,
    /// Carrier losses.
    #[column(c, unit = count)]
    pub tx_carrier: i64,
    /// Compressed packets transmitted.
    #[column(c, unit = count)]
    pub tx_compressed: i64,
    /// Negotiated link speed in Mbit/s from sysfs; `None` for a virtual or
    /// down interface where the kernel reports none.
    #[column(g, unit = count)]
    pub speed_mbit: Option<i64>,
    /// `0` unknown, `1` half, `2` full, from sysfs `duplex`.
    #[column(l)]
    pub duplex: u8,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_netdev.rs"]
mod tests;
