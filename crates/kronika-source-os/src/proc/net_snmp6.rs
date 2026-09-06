//! Parse `/proc/net/snmp6` IPv6 protocol counters (`1_118`).

use kronika_registry::Ts;
use kronika_registry::os_snmp6::OsSnmp6;

/// Build the `1_118_001` row from `/proc/net/snmp6` content.
///
/// The file is a flat `Name value` list, one counter per line. Absent counters
/// stay null: a kernel built without IPv6 has no file at all, and a zero would
/// read as "no IPv6 traffic" rather than "not measured".
///
/// # Errors
///
/// This parser has no failure mode: an unparsable value leaves that one column
/// null and the rest of the file is still used.
#[must_use]
pub fn parse(content: &str, ts: i64, scope: u8) -> OsSnmp6 {
    let mut row = OsSnmp6 {
        ts: Ts(ts),
        ip6_in_receives: None,
        ip6_in_hdr_errors: None,
        ip6_in_addr_errors: None,
        ip6_in_discards: None,
        ip6_in_delivers: None,
        ip6_out_requests: None,
        ip6_out_discards: None,
        ip6_out_no_routes: None,
        ip6_reasm_reqds: None,
        ip6_reasm_oks: None,
        ip6_reasm_fails: None,
        ip6_frag_oks: None,
        ip6_frag_fails: None,
        icmp6_in_msgs: None,
        icmp6_in_errors: None,
        icmp6_out_msgs: None,
        icmp6_out_errors: None,
        udp6_in_datagrams: None,
        udp6_out_datagrams: None,
        udp6_in_errors: None,
        udp6_no_ports: None,
        udp6_rcvbuf_errors: None,
        udp6_sndbuf_errors: None,
        scope,
    };

    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(value)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<i64>() else {
            continue;
        };
        let slot = match name {
            "Ip6InReceives" => &mut row.ip6_in_receives,
            "Ip6InHdrErrors" => &mut row.ip6_in_hdr_errors,
            "Ip6InAddrErrors" => &mut row.ip6_in_addr_errors,
            "Ip6InDiscards" => &mut row.ip6_in_discards,
            "Ip6InDelivers" => &mut row.ip6_in_delivers,
            "Ip6OutRequests" => &mut row.ip6_out_requests,
            "Ip6OutDiscards" => &mut row.ip6_out_discards,
            "Ip6OutNoRoutes" => &mut row.ip6_out_no_routes,
            "Ip6ReasmReqds" => &mut row.ip6_reasm_reqds,
            "Ip6ReasmOKs" => &mut row.ip6_reasm_oks,
            "Ip6ReasmFails" => &mut row.ip6_reasm_fails,
            "Ip6FragOKs" => &mut row.ip6_frag_oks,
            "Ip6FragFails" => &mut row.ip6_frag_fails,
            "Icmp6InMsgs" => &mut row.icmp6_in_msgs,
            "Icmp6InErrors" => &mut row.icmp6_in_errors,
            "Icmp6OutMsgs" => &mut row.icmp6_out_msgs,
            "Icmp6OutErrors" => &mut row.icmp6_out_errors,
            "Udp6InDatagrams" => &mut row.udp6_in_datagrams,
            "Udp6OutDatagrams" => &mut row.udp6_out_datagrams,
            "Udp6InErrors" => &mut row.udp6_in_errors,
            "Udp6NoPorts" => &mut row.udp6_no_ports,
            "Udp6RcvbufErrors" => &mut row.udp6_rcvbuf_errors,
            "Udp6SndbufErrors" => &mut row.udp6_sndbuf_errors,
            _ => continue,
        };
        *slot = Some(value);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::parse;

    const SAMPLE: &str = "\
Ip6InReceives                   \t1000
Ip6InHdrErrors                  \t0
Ip6InDelivers                   \t990
Ip6OutRequests                  \t980
Icmp6InMsgs                     \t12
Udp6InDatagrams                 \t7
Udp6NoPorts                     \t1
Ip6InTooBigErrors               \t0
";

    #[test]
    fn reads_the_named_counters() {
        let row = parse(SAMPLE, 42, 2);
        assert_eq!(row.ts.0, 42);
        assert_eq!(row.scope, 2);
        assert_eq!(row.ip6_in_receives, Some(1_000));
        assert_eq!(row.ip6_in_hdr_errors, Some(0));
        assert_eq!(row.ip6_in_delivers, Some(990));
        assert_eq!(row.ip6_out_requests, Some(980));
        assert_eq!(row.icmp6_in_msgs, Some(12));
        assert_eq!(row.udp6_in_datagrams, Some(7));
        assert_eq!(row.udp6_no_ports, Some(1));
    }

    #[test]
    fn counters_the_kernel_does_not_print_stay_null() {
        let row = parse(SAMPLE, 1, 0);
        assert_eq!(row.ip6_in_addr_errors, None);
        assert_eq!(row.udp6_sndbuf_errors, None);
        assert_eq!(row.icmp6_out_errors, None);
    }

    #[test]
    fn a_garbled_value_only_costs_its_own_column() {
        let row = parse("Ip6InReceives x\nIp6InDelivers 5\n", 1, 0);
        assert_eq!(row.ip6_in_receives, None);
        assert_eq!(row.ip6_in_delivers, Some(5));
    }
}
