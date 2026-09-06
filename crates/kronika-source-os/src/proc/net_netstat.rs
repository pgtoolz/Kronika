//! Parse `/proc/net/netstat` extended TCP counters (`1_111`).

use std::collections::HashMap;

use kronika_registry::Ts;
use kronika_registry::os_netstat::OsNetstat;

/// Parse error for procfs lines.
pub use crate::proc::stat::ParseError;

/// Extended TCP counters from `/proc/net/netstat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetNetstatRow {
    /// TCP listen queue overflows since boot.
    pub listen_overflows: i64,
    /// Connections dropped while listening since boot.
    pub listen_drops: i64,
    /// TCP timeout events since boot.
    pub tcp_timeouts: i64,
    /// TCP fast retransmissions since boot.
    pub tcp_fast_retrans: i64,
    /// TCP slow-start retransmissions since boot.
    pub tcp_slow_start_retrans: i64,
    /// Packets placed in the out-of-order queue since boot.
    pub tcp_ofo_queue: i64,
    /// SYN retransmissions since boot.
    pub tcp_syn_retrans: i64,
    /// Retransmissions later found unnecessary.
    pub tcp_lost_retransmit: i64,
    /// Connections reset after a retransmission timeout.
    pub tcp_abort_on_timeout: i64,
    /// Connections reset with data still queued at close.
    pub tcp_abort_on_close: i64,
    /// Connections reset under socket memory pressure.
    pub tcp_abort_on_memory: i64,
    /// Connections reset by unread data after shutdown.
    pub tcp_abort_on_data: i64,
    /// Resets that could not be sent for lack of memory.
    pub tcp_abort_failed: i64,
    /// Times the TCP stack entered memory pressure.
    pub tcp_memory_pressures: i64,
    /// Packets dropped because the socket backlog was full.
    pub tcp_backlog_drop: i64,
    /// Out-of-order packets dropped for lack of memory.
    pub tcp_ofo_drop: i64,
    /// Packets pruned from the receive queue.
    pub tcp_rcv_pruned: i64,
    /// Times receive-queue pruning was invoked.
    pub tcp_prune_called: i64,
    /// Acknowledgements deferred by the delayed-ACK timer.
    pub delayed_acks: i64,
    /// Sockets that left `TIME_WAIT` normally.
    pub time_wait: i64,
    /// Payload octets received.
    pub ip_in_octets: i64,
    /// Payload octets sent.
    pub ip_out_octets: i64,
}

/// Parse `/proc/net/netstat` content into a singleton [`NetNetstatRow`].
///
/// The file uses two-line group pairs sharing the same `<Group>:` prefix: the
/// first line names the columns, the second holds the values. Only the
/// `TcpExt:` group is consumed; all other groups are silently ignored. Values
/// are matched by column name, not position. Missing keys default to `0`.
///
/// # Errors
///
/// Returns [`ParseError`] when a value token is not a valid `i64`.
pub fn parse(content: &str) -> Result<NetNetstatRow, ParseError> {
    let mut header_lines: HashMap<&str, &str> = HashMap::new();
    let mut value_lines: HashMap<&str, &str> = HashMap::new();

    for line in content.lines() {
        let Some((prefix, rest)) = line.split_once(':') else {
            continue;
        };
        let prefix = prefix.trim();
        if header_lines.contains_key(prefix) {
            value_lines.insert(prefix, rest);
        } else {
            header_lines.insert(prefix, rest);
        }
    }

    let group = |name: &str| -> HashMap<&str, &str> {
        let headers = header_lines.get(name).copied().unwrap_or("");
        let values = value_lines.get(name).copied().unwrap_or("");
        headers
            .split_whitespace()
            .zip(values.split_whitespace())
            .collect()
    };
    let tcp_ext = group("TcpExt");
    let ip_ext = group("IpExt");

    let read = |map: &HashMap<&str, &str>, group: &str, key: &str| -> Result<i64, ParseError> {
        map.get(key).map_or(Ok(0), |v| {
            v.parse::<i64>()
                .map_err(|e| ParseError(format!("{group}/{key}: {e}")))
        })
    };
    let get = |key: &str| read(&tcp_ext, "TcpExt", key);

    Ok(NetNetstatRow {
        listen_overflows: get("ListenOverflows")?,
        listen_drops: get("ListenDrops")?,
        tcp_timeouts: get("TCPTimeouts")?,
        tcp_fast_retrans: get("TCPFastRetrans")?,
        tcp_slow_start_retrans: get("TCPSlowStartRetrans")?,
        tcp_ofo_queue: get("TCPOFOQueue")?,
        tcp_syn_retrans: get("TCPSynRetrans")?,
        tcp_lost_retransmit: get("TCPLostRetransmit")?,
        tcp_abort_on_timeout: get("TCPAbortOnTimeout")?,
        tcp_abort_on_close: get("TCPAbortOnClose")?,
        tcp_abort_on_memory: get("TCPAbortOnMemory")?,
        tcp_abort_on_data: get("TCPAbortOnData")?,
        tcp_abort_failed: get("TCPAbortFailed")?,
        tcp_memory_pressures: get("TCPMemoryPressures")?,
        tcp_backlog_drop: get("TCPBacklogDrop")?,
        tcp_ofo_drop: get("TCPOFODrop")?,
        tcp_rcv_pruned: get("RcvPruned")?,
        tcp_prune_called: get("PruneCalled")?,
        delayed_acks: get("DelayedACKs")?,
        time_wait: get("TW")?,
        ip_in_octets: read(&ip_ext, "IpExt", "InOctets")?,
        ip_out_octets: read(&ip_ext, "IpExt", "OutOctets")?,
    })
}

impl NetNetstatRow {
    /// Registry row for `1_111_001` with the given scope and timestamp.
    #[must_use]
    pub const fn to_section(self, scope: u8, ts: i64) -> OsNetstat {
        OsNetstat {
            ts: Ts(ts),
            listen_overflows: self.listen_overflows,
            listen_drops: self.listen_drops,
            tcp_timeouts: self.tcp_timeouts,
            tcp_fast_retrans: self.tcp_fast_retrans,
            tcp_slow_start_retrans: self.tcp_slow_start_retrans,
            tcp_ofo_queue: self.tcp_ofo_queue,
            tcp_syn_retrans: self.tcp_syn_retrans,
            tcp_lost_retransmit: self.tcp_lost_retransmit,
            tcp_abort_on_timeout: self.tcp_abort_on_timeout,
            tcp_abort_on_close: self.tcp_abort_on_close,
            tcp_abort_on_memory: self.tcp_abort_on_memory,
            tcp_abort_on_data: self.tcp_abort_on_data,
            tcp_abort_failed: self.tcp_abort_failed,
            tcp_memory_pressures: self.tcp_memory_pressures,
            tcp_backlog_drop: self.tcp_backlog_drop,
            tcp_ofo_drop: self.tcp_ofo_drop,
            tcp_rcv_pruned: self.tcp_rcv_pruned,
            tcp_prune_called: self.tcp_prune_called,
            delayed_acks: self.delayed_acks,
            time_wait: self.time_wait,
            ip_in_octets: self.ip_in_octets,
            ip_out_octets: self.ip_out_octets,
            scope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn matches_by_name_not_position() {
        // Fields shuffled; IpExt block must be silently ignored.
        let c = "\
TcpExt: SyncookiesSent SyncookiesRecv ListenOverflows ListenDrops TCPTimeouts TCPFastRetrans TCPSlowStartRetrans TCPOFOQueue TCPSynRetrans\n\
TcpExt: 0 0 10 20 30 40 50 60 70\n\
IpExt: InNoRoutes InTruncatedPkts\n\
IpExt: 1 2\n";
        let r = parse(c).unwrap();
        assert_eq!(r.listen_overflows, 10);
        assert_eq!(r.listen_drops, 20);
        assert_eq!(r.tcp_timeouts, 30);
        assert_eq!(r.tcp_fast_retrans, 40);
        assert_eq!(r.tcp_slow_start_retrans, 50);
        assert_eq!(r.tcp_ofo_queue, 60);
        assert_eq!(r.tcp_syn_retrans, 70);
    }

    #[test]
    fn missing_tcpext_group_yields_zeros() {
        let c = "\
IpExt: InNoRoutes InTruncatedPkts\n\
IpExt: 1 2\n";
        let r = parse(c).unwrap();
        assert_eq!(r.listen_overflows, 0);
        assert_eq!(r.listen_drops, 0);
        assert_eq!(r.tcp_timeouts, 0);
        assert_eq!(r.tcp_fast_retrans, 0);
        assert_eq!(r.tcp_slow_start_retrans, 0);
        assert_eq!(r.tcp_ofo_queue, 0);
        assert_eq!(r.tcp_syn_retrans, 0);
    }

    #[test]
    fn missing_key_within_tcpext_yields_zero() {
        let c = "\
TcpExt: ListenOverflows TCPTimeouts\n\
TcpExt: 11 22\n";
        let r = parse(c).unwrap();
        assert_eq!(r.listen_overflows, 11);
        assert_eq!(r.tcp_timeouts, 22);
        assert_eq!(r.listen_drops, 0);
        assert_eq!(r.tcp_fast_retrans, 0);
        assert_eq!(r.tcp_slow_start_retrans, 0);
        assert_eq!(r.tcp_ofo_queue, 0);
        assert_eq!(r.tcp_syn_retrans, 0);
    }

    #[test]
    fn garbled_value_is_an_error() {
        let c = "\
TcpExt: ListenOverflows\n\
TcpExt: notanumber\n";
        assert!(parse(c).is_err());
    }

    #[test]
    fn to_section_carries_all_fields_and_scope() {
        let c = "\
TcpExt: SyncookiesSent SyncookiesRecv ListenOverflows ListenDrops TCPTimeouts TCPFastRetrans TCPSlowStartRetrans TCPOFOQueue TCPSynRetrans\n\
TcpExt: 0 0 10 20 30 40 50 60 70\n\
IpExt: InNoRoutes InTruncatedPkts\n\
IpExt: 1 2\n";
        let section = parse(c).unwrap().to_section(2, 8_888);
        assert_eq!(section.ts.0, 8_888);
        assert_eq!(section.scope, 2);
        assert_eq!(section.listen_overflows, 10);
        assert_eq!(section.listen_drops, 20);
        assert_eq!(section.tcp_timeouts, 30);
        assert_eq!(section.tcp_fast_retrans, 40);
        assert_eq!(section.tcp_slow_start_retrans, 50);
        assert_eq!(section.tcp_ofo_queue, 60);
        assert_eq!(section.tcp_syn_retrans, 70);
    }
}
