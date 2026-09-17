//! Type `1_118_001`: IPv6 protocol counters from `/proc/net/snmp6`.

use crate::{Section, Ts};

/// `IPv6`, `ICMPv6`, and `UDPv6` counters for one network namespace.
///
/// Every field is nullable: `/proc/net/snmp6` is absent on a kernel built
/// without IPv6, and a missing counter must not read as zero traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_118_001,
    name = "os_snmp6",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsSnmp6 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Datagrams received, including errors.
    #[column(c, unit = count)]
    pub ip6_in_receives: Option<i64>,
    /// Datagrams dropped for a bad header.
    #[column(c, unit = count)]
    pub ip6_in_hdr_errors: Option<i64>,
    /// Datagrams dropped because the destination was not local.
    #[column(c, unit = count)]
    pub ip6_in_addr_errors: Option<i64>,
    /// Datagrams dropped without a specific error.
    #[column(c, unit = count)]
    pub ip6_in_discards: Option<i64>,
    /// Datagrams delivered to an upper protocol.
    #[column(c, unit = count)]
    pub ip6_in_delivers: Option<i64>,
    /// Datagrams handed down by an upper protocol for transmission.
    #[column(c, unit = count)]
    pub ip6_out_requests: Option<i64>,
    /// Outgoing datagrams dropped without a specific error.
    #[column(c, unit = count)]
    pub ip6_out_discards: Option<i64>,
    /// Outgoing datagrams dropped for lack of a route.
    #[column(c, unit = count)]
    pub ip6_out_no_routes: Option<i64>,
    /// Fragments received that needed reassembly.
    #[column(c, unit = count)]
    pub ip6_reasm_reqds: Option<i64>,
    /// Datagrams successfully reassembled.
    #[column(c, unit = count)]
    pub ip6_reasm_oks: Option<i64>,
    /// Reassembly failures.
    #[column(c, unit = count)]
    pub ip6_reasm_fails: Option<i64>,
    /// Datagrams successfully fragmented.
    #[column(c, unit = count)]
    pub ip6_frag_oks: Option<i64>,
    /// Fragmentation failures.
    #[column(c, unit = count)]
    pub ip6_frag_fails: Option<i64>,
    /// `ICMPv6` messages received.
    #[column(c, unit = count)]
    pub icmp6_in_msgs: Option<i64>,
    /// `ICMPv6` messages received with errors.
    #[column(c, unit = count)]
    pub icmp6_in_errors: Option<i64>,
    /// `ICMPv6` messages sent.
    #[column(c, unit = count)]
    pub icmp6_out_msgs: Option<i64>,
    /// `ICMPv6` messages that could not be sent.
    #[column(c, unit = count)]
    pub icmp6_out_errors: Option<i64>,
    /// `UDPv6` datagrams delivered.
    #[column(c, unit = count)]
    pub udp6_in_datagrams: Option<i64>,
    /// `UDPv6` datagrams sent.
    #[column(c, unit = count)]
    pub udp6_out_datagrams: Option<i64>,
    /// `UDPv6` datagrams dropped for a receive error.
    #[column(c, unit = count)]
    pub udp6_in_errors: Option<i64>,
    /// `UDPv6` datagrams for a port with no listener.
    #[column(c, unit = count)]
    pub udp6_no_ports: Option<i64>,
    /// `UDPv6` datagrams dropped because the receive buffer was full.
    #[column(c, unit = count)]
    pub udp6_rcvbuf_errors: Option<i64>,
    /// `UDPv6` datagrams dropped because the send buffer was full.
    #[column(c, unit = count)]
    pub udp6_sndbuf_errors: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_snmp6.rs"]
mod tests;
