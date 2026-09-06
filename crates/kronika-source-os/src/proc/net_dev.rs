//! Parse `/proc/net/dev` per-interface network counters (`1_109`).

use kronika_registry::os_netdev::OsNetdev;
use kronika_registry::{StrId, Ts};

/// Parse error for procfs lines.
pub use crate::proc::stat::ParseError;

/// One network interface's counters from a `/proc/net/dev` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetDevRow {
    /// Interface name (`:` stripped).
    pub iface: String,
    /// Bytes received.
    pub rx_bytes: i64,
    /// Packets received.
    pub rx_packets: i64,
    /// Receive errors.
    pub rx_errs: i64,
    /// Receive drops.
    pub rx_drop: i64,
    /// Receive FIFO errors.
    pub rx_fifo: i64,
    /// Receive frame errors.
    pub rx_frame: i64,
    /// Compressed packets received.
    pub rx_compressed: i64,
    /// Multicast frames received.
    pub rx_multicast: i64,
    /// Bytes transmitted.
    pub tx_bytes: i64,
    /// Packets transmitted.
    pub tx_packets: i64,
    /// Transmit errors.
    pub tx_errs: i64,
    /// Transmit drops.
    pub tx_drop: i64,
    /// Transmit FIFO errors.
    pub tx_fifo: i64,
    /// Collisions.
    pub tx_colls: i64,
    /// Carrier losses.
    pub tx_carrier: i64,
    /// Compressed packets transmitted.
    pub tx_compressed: i64,
    /// Negotiated link speed in Mbit/s, filled from sysfs by the collector.
    pub speed_mbit: Option<i64>,
    /// `0` unknown, `1` half, `2` full, filled from sysfs by the collector.
    pub duplex: u8,
}

fn parse_i64(s: &str, pos: usize) -> Result<i64, ParseError> {
    s.parse::<i64>()
        .map_err(|e| ParseError(format!("net/dev field {pos}: {e}")))
}

/// Parse every data line in `/proc/net/dev` content.
///
/// Header lines (containing `|`) are skipped. Lines where the interface
/// column and first counter are glued together (e.g. `eth0:1000`) are handled
/// by splitting on the first `:`. Rows with fewer than 16 counters are
/// silently skipped. A non-numeric counter is a [`ParseError`].
///
/// # Errors
///
/// Returns [`ParseError`] when an integer field cannot be parsed.
pub fn parse(content: &str) -> Result<Vec<NetDevRow>, ParseError> {
    let mut rows = Vec::new();
    for line in content.lines() {
        if line.contains('|') {
            continue;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Split interface name from the rest; the `:` may be glued to the
        // first counter (for example, `eth0:1000 ...`).
        let Some((iface_raw, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface_raw.trim().to_owned();

        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 16 {
            continue;
        }

        let rx_bytes = parse_i64(fields[0], 0)?;
        let rx_packets = parse_i64(fields[1], 1)?;
        let rx_errs = parse_i64(fields[2], 2)?;
        let rx_drop = parse_i64(fields[3], 3)?;
        let rx_fifo = parse_i64(fields[4], 4)?;
        let rx_frame = parse_i64(fields[5], 5)?;
        let rx_compressed = parse_i64(fields[6], 6)?;
        let rx_multicast = parse_i64(fields[7], 7)?;
        let tx_bytes = parse_i64(fields[8], 8)?;
        let tx_packets = parse_i64(fields[9], 9)?;
        let tx_errs = parse_i64(fields[10], 10)?;
        let tx_drop = parse_i64(fields[11], 11)?;
        let tx_fifo = parse_i64(fields[12], 12)?;
        let tx_colls = parse_i64(fields[13], 13)?;
        let tx_carrier = parse_i64(fields[14], 14)?;
        let tx_compressed = parse_i64(fields[15], 15)?;

        rows.push(NetDevRow {
            iface,
            rx_bytes,
            rx_packets,
            rx_errs,
            rx_drop,
            rx_fifo,
            rx_frame,
            rx_compressed,
            rx_multicast,
            tx_bytes,
            tx_packets,
            tx_errs,
            tx_drop,
            tx_fifo,
            tx_colls,
            tx_carrier,
            tx_compressed,
            speed_mbit: None,
            duplex: 0,
        });
    }
    Ok(rows)
}

impl NetDevRow {
    /// Registry row for `1_109_001` with the given scope, timestamp, and
    /// pre-resolved interface string-dictionary id.
    #[must_use]
    pub const fn to_section(&self, scope: u8, ts: i64, iface_id: StrId) -> OsNetdev {
        OsNetdev {
            ts: Ts(ts),
            iface: iface_id,
            rx_bytes: self.rx_bytes,
            rx_packets: self.rx_packets,
            rx_errs: self.rx_errs,
            rx_drop: self.rx_drop,
            rx_fifo: self.rx_fifo,
            rx_frame: self.rx_frame,
            rx_compressed: self.rx_compressed,
            rx_multicast: self.rx_multicast,
            tx_bytes: self.tx_bytes,
            tx_packets: self.tx_packets,
            tx_errs: self.tx_errs,
            tx_drop: self.tx_drop,
            tx_fifo: self.tx_fifo,
            tx_colls: self.tx_colls,
            tx_carrier: self.tx_carrier,
            tx_compressed: self.tx_compressed,
            speed_mbit: self.speed_mbit,
            duplex: self.duplex,
            scope,
        }
    }
}

#[cfg(test)]
mod tests {
    use kronika_registry::StrId;

    use super::parse;

    #[test]
    fn parses_all_sixteen_columns_and_strips_iface_colon() {
        let c = "\
Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n\
  eth0:1000 10 1 2 3 4 5 6 2000 20 7 8 9 10 11 12\n";
        let rows = parse(c).unwrap();
        assert_eq!(rows.len(), 2);
        let eth = rows.iter().find(|r| r.iface == "eth0").unwrap();
        assert_eq!(eth.rx_bytes, 1000);
        assert_eq!(eth.rx_multicast, 6);
        assert_eq!(eth.tx_bytes, 2000);
        assert_eq!(eth.tx_compressed, 12);
    }

    #[test]
    fn includes_loopback_and_skips_short_lines() {
        let c = "\
Inter-|   Receive\n\
    lo: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n\
  bad: 1 2 3\n";
        let rows = parse(c).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].iface, "lo");
    }

    #[test]
    fn garbled_counter_is_an_error() {
        let c = "    lo: notanumber 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n";
        assert!(parse(c).is_err());
    }

    #[test]
    fn to_section_carries_every_field_and_scope() {
        let c = "\
Inter-|header\n\
    lo: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n";
        let row = &parse(c).unwrap()[0];
        let section = row.to_section(2, 9_999, StrId(7));
        assert_eq!(section.ts.0, 9_999);
        assert_eq!(section.iface, StrId(7));
        assert_eq!(section.rx_bytes, 100);
        assert_eq!(section.tx_bytes, 200);
        assert_eq!(section.scope, 2);
    }
}
