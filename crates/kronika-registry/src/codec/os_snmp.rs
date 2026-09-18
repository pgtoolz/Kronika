//! Type `1_110_001`: global TCP/UDP counters from `/proc/net/snmp`.

use crate::{Section, Ts};

/// Global TCP and UDP counters from the `/proc/net/snmp` singleton.
///
/// Collected once per snapshot. `tcp_curr_estab` is a gauge (current count);
/// all other counter fields are cumulative since boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_110_001,
    name = "os_snmp",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsSnmp {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Active TCP connections opened since boot.
    #[column(c, unit = count)]
    pub tcp_active_opens: i64,
    /// Passive TCP connections opened since boot.
    #[column(c, unit = count)]
    pub tcp_passive_opens: i64,
    /// TCP connection attempts that failed since boot.
    #[column(c, unit = count)]
    pub tcp_attempt_fails: i64,
    /// TCP connections reset from ESTABLISHED state since boot.
    #[column(c, unit = count)]
    pub tcp_estab_resets: i64,
    /// TCP segments received since boot.
    #[column(c, unit = count)]
    pub tcp_in_segs: i64,
    /// TCP segments sent since boot.
    #[column(c, unit = count)]
    pub tcp_out_segs: i64,
    /// TCP segments retransmitted since boot.
    #[column(c, unit = count)]
    pub tcp_retrans_segs: i64,
    /// TCP segments received with errors since boot.
    #[column(c, unit = count)]
    pub tcp_in_errs: i64,
    /// TCP resets sent since boot.
    #[column(c, unit = count)]
    pub tcp_out_rsts: i64,
    /// TCP connections currently in ESTABLISHED or CLOSE-WAIT state.
    #[column(g, unit = count)]
    pub tcp_curr_estab: i64,
    /// UDP datagrams received since boot.
    #[column(c, unit = count)]
    pub udp_in_datagrams: i64,
    /// UDP datagrams sent since boot.
    #[column(c, unit = count)]
    pub udp_out_datagrams: i64,
    /// UDP receive errors since boot.
    #[column(c, unit = count)]
    pub udp_in_errors: i64,
    /// UDP datagrams received to a port with no listener.
    #[column(c, unit = count)]
    pub udp_no_ports: i64,
    /// IPv4 datagrams received, including errors.
    #[column(c, unit = count)]
    pub ip_in_receives: Option<i64>,
    /// IPv4 datagrams dropped for a bad header.
    #[column(c, unit = count)]
    pub ip_in_hdr_errors: Option<i64>,
    /// IPv4 datagrams dropped because the destination was not local.
    #[column(c, unit = count)]
    pub ip_in_addr_errors: Option<i64>,
    /// IPv4 datagrams forwarded to another host.
    #[column(c, unit = count)]
    pub ip_forw_datagrams: Option<i64>,
    /// IPv4 datagrams for an unsupported upper protocol.
    #[column(c, unit = count)]
    pub ip_in_unknown_protos: Option<i64>,
    /// Incoming IPv4 datagrams dropped without a specific error.
    #[column(c, unit = count)]
    pub ip_in_discards: Option<i64>,
    /// IPv4 datagrams delivered to an upper protocol.
    #[column(c, unit = count)]
    pub ip_in_delivers: Option<i64>,
    /// IPv4 datagrams handed down for transmission.
    #[column(c, unit = count)]
    pub ip_out_requests: Option<i64>,
    /// Outgoing IPv4 datagrams dropped without a specific error.
    #[column(c, unit = count)]
    pub ip_out_discards: Option<i64>,
    /// Outgoing IPv4 datagrams dropped for lack of a route.
    #[column(c, unit = count)]
    pub ip_out_no_routes: Option<i64>,
    /// IPv4 fragments received that needed reassembly.
    #[column(c, unit = count)]
    pub ip_reasm_reqds: Option<i64>,
    /// IPv4 datagrams successfully reassembled.
    #[column(c, unit = count)]
    pub ip_reasm_oks: Option<i64>,
    /// IPv4 reassembly failures.
    #[column(c, unit = count)]
    pub ip_reasm_fails: Option<i64>,
    /// IPv4 datagrams successfully fragmented.
    #[column(c, unit = count)]
    pub ip_frag_oks: Option<i64>,
    /// IPv4 fragmentation failures.
    #[column(c, unit = count)]
    pub ip_frag_fails: Option<i64>,
    /// IPv4 fragments generated.
    #[column(c, unit = count)]
    pub ip_frag_creates: Option<i64>,
    /// ICMP messages received.
    #[column(c, unit = count)]
    pub icmp_in_msgs: Option<i64>,
    /// ICMP messages received with errors.
    #[column(c, unit = count)]
    pub icmp_in_errors: Option<i64>,
    /// ICMP messages sent.
    #[column(c, unit = count)]
    pub icmp_out_msgs: Option<i64>,
    /// ICMP messages that could not be sent.
    #[column(c, unit = count)]
    pub icmp_out_errors: Option<i64>,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_snmp.rs"]
mod tests;
