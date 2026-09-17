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
