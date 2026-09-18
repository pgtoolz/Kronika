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
