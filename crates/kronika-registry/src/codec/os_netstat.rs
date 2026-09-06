//! Type `1_111_001`: extended TCP counters from `/proc/net/netstat`.

use crate::{Section, Ts};

/// Extended TCP counters from the `/proc/net/netstat` singleton.
///
/// All fields are cumulative counters since boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_111_001,
    name = "os_netstat",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsNetstat {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// TCP listen queue overflows since boot.
    #[column(c, unit = count)]
    pub listen_overflows: i64,
    /// Connections dropped while listening since boot.
    #[column(c, unit = count)]
    pub listen_drops: i64,
    /// TCP timeout events since boot.
    #[column(c, unit = count)]
    pub tcp_timeouts: i64,
    /// TCP fast retransmissions since boot.
    #[column(c, unit = count)]
    pub tcp_fast_retrans: i64,
    /// TCP slow-start retransmissions since boot.
    #[column(c, unit = count)]
    pub tcp_slow_start_retrans: i64,
    /// Packets placed in the out-of-order queue since boot.
    #[column(c, unit = count)]
    pub tcp_ofo_queue: i64,
    /// SYN retransmissions since boot.
    #[column(c, unit = count)]
    pub tcp_syn_retrans: i64,
    /// Retransmissions the sender later found unnecessary.
    #[column(c, unit = count)]
    pub tcp_lost_retransmit: i64,
    /// Connections reset after a retransmission timeout.
    #[column(c, unit = count)]
    pub tcp_abort_on_timeout: i64,
    /// Connections reset because data was still queued at close.
    #[column(c, unit = count)]
    pub tcp_abort_on_close: i64,
    /// Connections reset under socket memory pressure.
    #[column(c, unit = count)]
    pub tcp_abort_on_memory: i64,
    /// Connections reset because unread data arrived after shutdown.
    #[column(c, unit = count)]
    pub tcp_abort_on_data: i64,
    /// Resets that could not be sent for lack of memory.
    #[column(c, unit = count)]
    pub tcp_abort_failed: i64,
    /// Times the TCP stack entered memory pressure.
    #[column(c, unit = count)]
    pub tcp_memory_pressures: i64,
    /// Packets dropped because the socket backlog was full.
    #[column(c, unit = count)]
    pub tcp_backlog_drop: i64,
    /// Out-of-order packets dropped for lack of memory.
    #[column(c, unit = count)]
    pub tcp_ofo_drop: i64,
    /// Packets pruned from the receive queue under memory pressure.
    #[column(c, unit = count)]
    pub tcp_rcv_pruned: i64,
    /// Times receive-queue pruning was invoked.
    #[column(c, unit = count)]
    pub tcp_prune_called: i64,
    /// Acknowledgements deferred by the delayed-ACK timer.
    #[column(c, unit = count)]
    pub delayed_acks: i64,
    /// Sockets that left `TIME_WAIT` normally.
    #[column(c, unit = count)]
    pub time_wait: i64,
    /// Payload octets received (`IpExt: InOctets`).
    #[column(c, unit = count)]
    pub ip_in_octets: i64,
    /// Payload octets sent (`IpExt: OutOctets`).
    #[column(c, unit = count)]
    pub ip_out_octets: i64,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsNetstat;
    use crate::{Section, Ts, VerifiedSection, contract::lint};

    fn row(ts: i64) -> OsNetstat {
        OsNetstat {
            ts: Ts(ts),
            listen_overflows: 10,
            listen_drops: 20,
            tcp_timeouts: 30,
            tcp_fast_retrans: 40,
            tcp_slow_start_retrans: 50,
            tcp_ofo_queue: 60,
            tcp_syn_retrans: 70,
            tcp_lost_retransmit: 80,
            tcp_abort_on_timeout: 90,
            tcp_abort_on_close: 100,
            tcp_abort_on_memory: 110,
            tcp_abort_on_data: 120,
            tcp_abort_failed: 130,
            tcp_memory_pressures: 140,
            tcp_backlog_drop: 150,
            tcp_ofo_drop: 160,
            tcp_rcv_pruned: 170,
            tcp_prune_called: 180,
            delayed_acks: 190,
            time_wait: 200,
            ip_in_octets: 210,
            ip_out_octets: 220,
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsNetstat::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsNetstat::CONTRACT;
        assert_eq!(c.type_id.get(), 1_111_001);
        assert_eq!(c.sort_key, ["ts"]);
    }

    #[test]
    fn encode_sorts_by_ts() {
        let bytes = OsNetstat::encode(&[row(2_000), row(1_000), row(3_000)]).expect("encode");
        let decoded = OsNetstat::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded.iter().map(|r| r.ts.0).collect::<Vec<_>>(),
            [1_000, 2_000, 3_000]
        );
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[row(1_000), row(2_000)]);
    }

    #[test]
    fn all_seven_counters_survive_encode_decode() {
        let bytes = OsNetstat::encode(&[row(5)]).expect("encode");
        let decoded = OsNetstat::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        let r = &decoded[0];
        assert_eq!(r.listen_overflows, 10);
        assert_eq!(r.listen_drops, 20);
        assert_eq!(r.tcp_timeouts, 30);
        assert_eq!(r.tcp_fast_retrans, 40);
        assert_eq!(r.tcp_slow_start_retrans, 50);
        assert_eq!(r.tcp_ofo_queue, 60);
        assert_eq!(r.tcp_syn_retrans, 70);
    }
}
