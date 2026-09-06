//! Parse `/proc/diskstats` per-device I/O counters (`1_108`).

use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::{StrId, Ts};

/// Parse error for procfs lines.
pub use crate::proc::stat::ParseError;

/// One block device's I/O counters from a `/proc/diskstats` line.
///
/// Sector counts are raw 512-byte units, not converted to bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskstatsRow {
    /// Major device number.
    pub major: i32,
    /// Minor device number.
    pub minor: i32,
    /// Device name (e.g. `sda`, `nvme0n1`).
    pub device: String,
    /// Reads completed successfully.
    pub reads: i64,
    /// Reads merged before submitting to device.
    pub r_merged: i64,
    /// Sectors read (512-byte units).
    pub read_sectors: i64,
    /// Time spent reading, milliseconds.
    pub read_time_ms: i64,
    /// Writes completed successfully.
    pub writes: i64,
    /// Writes merged before submitting to device.
    pub w_merged: i64,
    /// Sectors written (512-byte units).
    pub write_sectors: i64,
    /// Time spent writing, milliseconds.
    pub write_time_ms: i64,
    /// I/O operations currently in progress (gauge, not a monotonic counter).
    pub io_in_progress: i64,
    /// Time spent doing I/O, milliseconds.
    pub io_time_ms: i64,
    /// Weighted time spent doing I/O, milliseconds.
    pub io_weighted_time_ms: i64,
    /// Discard operations completed (kernel >= 4.18; `None` on older kernels).
    pub discards: Option<i64>,
    /// Discards merged (kernel >= 4.18; `None` on older kernels).
    pub d_merged: Option<i64>,
    /// Sectors discarded (kernel >= 4.18; `None` on older kernels).
    pub discard_sectors: Option<i64>,
    /// Time spent discarding, milliseconds (kernel >= 4.18; `None` on older kernels).
    pub discard_time_ms: Option<i64>,
    /// Flush requests completed (kernel >= 5.5; `None` on older kernels).
    pub flushes: Option<i64>,
    /// Time spent flushing, milliseconds (kernel >= 5.5; `None` on older kernels).
    pub flush_time_ms: Option<i64>,
}

/// Major numbers of block devices that store nothing of their own: `1` is a
/// RAM disk and `7` is a loop device. A host with snap packages carries a loop
/// device per package, each backed by a squashfs file that already lives on a
/// real device, so their counters describe the same I/O a second time.
const PSEUDO_DEVICE_MAJORS: &[i32] = &[1, 7];

/// Whether `/proc/diskstats` rows of this major number describe real storage.
#[must_use]
pub fn is_pseudo_device(major: i32) -> bool {
    PSEUDO_DEVICE_MAJORS.contains(&major)
}

fn parse_i32(s: &str, pos: usize) -> Result<i32, ParseError> {
    s.parse::<i32>()
        .map_err(|e| ParseError(format!("diskstats field {pos}: {e}")))
}

fn parse_i64(s: &str, pos: usize) -> Result<i64, ParseError> {
    s.parse::<i64>()
        .map_err(|e| ParseError(format!("diskstats field {pos}: {e}")))
}

/// Parse every line in `/proc/diskstats` content.
///
/// Lines with fewer than 14 whitespace-separated fields are silently skipped
/// (partition entries on older kernels). A line with at least 14 fields but a
/// non-numeric value is a [`ParseError`].
///
/// # Errors
///
/// Returns [`ParseError`] when an integer field cannot be parsed.
pub fn parse(content: &str) -> Result<Vec<DiskstatsRow>, ParseError> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 14 {
            continue;
        }
        if is_pseudo_device(parse_i32(fields[0], 0)?) {
            continue;
        }

        let major = parse_i32(fields[0], 0)?;
        let minor = parse_i32(fields[1], 1)?;
        let device = fields[2].to_owned();
        let reads = parse_i64(fields[3], 3)?;
        let r_merged = parse_i64(fields[4], 4)?;
        let read_sectors = parse_i64(fields[5], 5)?;
        let read_time_ms = parse_i64(fields[6], 6)?;
        let writes = parse_i64(fields[7], 7)?;
        let w_merged = parse_i64(fields[8], 8)?;
        let write_sectors = parse_i64(fields[9], 9)?;
        let write_time_ms = parse_i64(fields[10], 10)?;
        let io_in_progress = parse_i64(fields[11], 11)?;
        let io_time_ms = parse_i64(fields[12], 12)?;
        let io_weighted_time_ms = parse_i64(fields[13], 13)?;

        // Discard counters: fields 15-18 (indices 14-17), kernel >= 4.18.
        let (discards, d_merged, discard_sectors, discard_time_ms) = if fields.len() >= 18 {
            (
                Some(parse_i64(fields[14], 14)?),
                Some(parse_i64(fields[15], 15)?),
                Some(parse_i64(fields[16], 16)?),
                Some(parse_i64(fields[17], 17)?),
            )
        } else {
            (None, None, None, None)
        };

        // Flush counters: fields 19-20 (indices 18-19), kernel >= 5.5.
        let (flushes, flush_time_ms) = if fields.len() >= 20 {
            (
                Some(parse_i64(fields[18], 18)?),
                Some(parse_i64(fields[19], 19)?),
            )
        } else {
            (None, None)
        };

        rows.push(DiskstatsRow {
            major,
            minor,
            device,
            reads,
            r_merged,
            read_sectors,
            read_time_ms,
            writes,
            w_merged,
            write_sectors,
            write_time_ms,
            io_in_progress,
            io_time_ms,
            io_weighted_time_ms,
            discards,
            d_merged,
            discard_sectors,
            discard_time_ms,
            flushes,
            flush_time_ms,
        });
    }
    Ok(rows)
}

impl DiskstatsRow {
    /// Registry row for `1_108_001` with the given scope, timestamp, and
    /// pre-resolved device string-dictionary id.
    #[must_use]
    pub const fn to_section(&self, scope: u8, ts: i64, device_id: StrId) -> OsDiskstats {
        OsDiskstats {
            ts: Ts(ts),
            major: self.major,
            minor: self.minor,
            device: device_id,
            reads: self.reads,
            r_merged: self.r_merged,
            read_sectors: self.read_sectors,
            read_time_ms: self.read_time_ms,
            writes: self.writes,
            w_merged: self.w_merged,
            write_sectors: self.write_sectors,
            write_time_ms: self.write_time_ms,
            io_in_progress: self.io_in_progress,
            io_time_ms: self.io_time_ms,
            io_weighted_time_ms: self.io_weighted_time_ms,
            discards: self.discards,
            d_merged: self.d_merged,
            discard_sectors: self.discard_sectors,
            discard_time_ms: self.discard_time_ms,
            flushes: self.flushes,
            flush_time_ms: self.flush_time_ms,
            scope,
        }
    }
}

#[cfg(test)]
mod tests {
    use kronika_registry::StrId;

    use super::{is_pseudo_device, parse};

    #[test]
    fn skips_loop_and_ram_devices() {
        let content = "\
   7       0 loop0 10 0 80 4 0 0 0 0 0 4 4\n\
   1       0 ram0 1 0 8 0 0 0 0 0 0 0 0\n\
   8       0 sda 100 2 3000 40 200 5 6000 70 1 800 900\n";
        let rows = parse(content).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].device, "sda");
        assert!(is_pseudo_device(7) && is_pseudo_device(1) && !is_pseudo_device(8));
    }

    #[test]
    fn parses_modern_and_legacy_lines() {
        // modern: 20 fields (with discard+flush); legacy: 14 fields
        let c = "\
   8       0 sda 100 2 3000 40 200 5 6000 70 1 800 900 10 11 12 13 14 15\n\
 259       0 nvme0n1 1 0 8 2 3 0 24 4 0 6 6\n";
        let rows = parse(c).unwrap();
        assert_eq!(rows.len(), 2);
        let sda = &rows[0];
        assert_eq!((sda.major, sda.minor, sda.device.as_str()), (8, 0, "sda"));
        assert_eq!(sda.reads, 100);
        assert_eq!(sda.read_sectors, 3000);
        assert_eq!(sda.io_in_progress, 1);
        assert_eq!(sda.io_weighted_time_ms, 900);
        assert_eq!(sda.discards, Some(10));
        assert_eq!(sda.flushes, Some(14));
        let nvme = &rows[1];
        assert_eq!(nvme.reads, 1);
        assert_eq!(nvme.discards, None); // legacy 14-field line
    }

    #[test]
    fn short_line_is_silently_skipped() {
        // A line with fewer than 14 fields is skipped, not an error.
        let c = "8 0 sda 100 2 3000 40 200 5 6000\n\
                 8 1 sda1 10 0 80 5 20 0 160 3 0 40 45\n";
        let rows = parse(c).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].device, "sda1");
    }

    #[test]
    fn garbled_integer_field_is_an_error() {
        let c = "8 0 sda notanumber 2 3000 40 200 5 6000 70 1 800 900\n";
        assert!(parse(c).is_err());
    }

    #[test]
    fn to_section_carries_every_floor_field_and_scope() {
        let c = "8 0 sda 100 2 3000 40 200 5 6000 70 1 800 900\n";
        let row = &parse(c).unwrap()[0];
        let section = row.to_section(3, 9_999, StrId(5));
        assert_eq!(section.ts.0, 9_999);
        assert_eq!(section.major, 8);
        assert_eq!(section.minor, 0);
        assert_eq!(section.device, StrId(5));
        assert_eq!(section.reads, 100);
        assert_eq!(section.read_sectors, 3000);
        assert_eq!(section.io_in_progress, 1);
        assert_eq!(section.io_weighted_time_ms, 900);
        assert_eq!(section.discards, None);
        assert_eq!(section.flushes, None);
        assert_eq!(section.scope, 3);
    }
}
