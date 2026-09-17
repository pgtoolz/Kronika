use super::OsSnmp6;
use crate::{Section, Ts, contract::lint};

fn row(ts: i64, present: bool) -> OsSnmp6 {
    let v = |n: i64| present.then_some(n);
    OsSnmp6 {
        ts: Ts(ts),
        ip6_in_receives: v(1),
        ip6_in_hdr_errors: v(2),
        ip6_in_addr_errors: v(3),
        ip6_in_discards: v(4),
        ip6_in_delivers: v(5),
        ip6_out_requests: v(6),
        ip6_out_discards: v(7),
        ip6_out_no_routes: v(8),
        ip6_reasm_reqds: v(9),
        ip6_reasm_oks: v(10),
        ip6_reasm_fails: v(11),
        ip6_frag_oks: v(12),
        ip6_frag_fails: v(13),
        icmp6_in_msgs: v(14),
        icmp6_in_errors: v(15),
        icmp6_out_msgs: v(16),
        icmp6_out_errors: v(17),
        udp6_in_datagrams: v(18),
        udp6_out_datagrams: v(19),
        udp6_in_errors: v(20),
        udp6_no_ports: v(21),
        udp6_rcvbuf_errors: v(22),
        udp6_sndbuf_errors: v(23),
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsSnmp6::CONTRACT]), Ok(()));
}

#[test]
fn roundtrip_keeps_a_kernel_without_ipv6_null() {
    crate::assert_roundtrips(&[row(1, true), row(2, false)]);
}
