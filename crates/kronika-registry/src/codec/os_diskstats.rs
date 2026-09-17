//! Type `1_108_001`: per-device I/O counters from `/proc/diskstats`.

use crate::{Section, StrId, Ts};

/// Per-device block I/O counters from one `/proc/diskstats` line.
///
/// Sector fields carry raw 512-byte units as reported by the kernel.
/// `io_in_progress` is a gauge; all other counter fields are cumulative.
/// Discard and flush counters are `None` on kernels older than 4.18 / 5.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_108_001,
    name = "os_diskstats",
    semantics = snapshot_full,
    sort_key("major", "minor", "ts"),
    identity("major", "minor")
)]
pub struct OsDiskstats {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Major device number.
    #[column(l)]
    pub major: i32,
    /// Minor device number.
    #[column(l)]
    pub minor: i32,
    /// Device name (e.g. `sda`, `nvme0n1`), as a string dictionary reference.
    #[column(l)]
    pub device: StrId,
    /// Reads completed successfully.
    #[column(c, unit = count)]
    pub reads: i64,
    /// Reads merged before submitting to the device.
    #[column(c, unit = count)]
    pub r_merged: i64,
    /// Sectors read (512-byte units).
    #[column(c, unit = sectors)]
    pub read_sectors: i64,
    /// Time spent reading.
    #[column(c, unit = milliseconds)]
    pub read_time_ms: i64,
    /// Writes completed successfully.
    #[column(c, unit = count)]
    pub writes: i64,
    /// Writes merged before submitting to the device.
    #[column(c, unit = count)]
    pub w_merged: i64,
    /// Sectors written (512-byte units).
    #[column(c, unit = sectors)]
    pub write_sectors: i64,
    /// Time spent writing.
    #[column(c, unit = milliseconds)]
    pub write_time_ms: i64,
    /// I/O operations currently in progress (instantaneous, not monotonic).
    #[column(g, unit = count)]
    pub io_in_progress: i64,
    /// Total time spent doing I/O.
    #[column(c, unit = milliseconds)]
    pub io_time_ms: i64,
    /// Weighted time spent doing I/O.
    #[column(c, unit = milliseconds)]
    pub io_weighted_time_ms: i64,
    /// Discard operations completed (kernel >= 4.18; `None` on older kernels).
    #[column(c, unit = count)]
    pub discards: Option<i64>,
    /// Discards merged (kernel >= 4.18; `None` on older kernels).
    #[column(c, unit = count)]
    pub d_merged: Option<i64>,
    /// Sectors discarded (kernel >= 4.18; `None` on older kernels).
    #[column(c, unit = sectors)]
    pub discard_sectors: Option<i64>,
    /// Time spent discarding, milliseconds (kernel >= 4.18; `None` on older kernels).
    #[column(c, unit = milliseconds)]
    pub discard_time_ms: Option<i64>,
    /// Flush requests completed (kernel >= 5.5; `None` on older kernels).
    #[column(c, unit = count)]
    pub flushes: Option<i64>,
    /// Time spent flushing, milliseconds (kernel >= 5.5; `None` on older kernels).
    #[column(c, unit = milliseconds)]
    pub flush_time_ms: Option<i64>,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_diskstats.rs"]
mod tests;
