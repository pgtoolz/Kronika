use super::parse;

#[test]
fn matches_by_name_not_position() {
    let c = "\
Tcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts\n\
Tcp: 1 200 120000 -1 5 6 7 8 9 100 110 3 1 2\n\
Udp: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors\n\
Udp: 500 4 2 600 0 0\n";
    let r = parse(c).unwrap();
    assert_eq!(r.tcp_active_opens, 5);
    assert_eq!(r.tcp_curr_estab, 9);
    assert_eq!(r.tcp_out_rsts, 2);
    assert_eq!(r.udp_in_datagrams, 500);
    assert_eq!(r.udp_no_ports, 4);
}

#[test]
fn missing_udp_group_yields_zeros_no_error() {
    let c = "\
Tcp: RtoAlgorithm ActiveOpens PassiveOpens AttemptFails EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts\n\
Tcp: 1 5 6 7 8 9 100 110 3 1 2\n";
    let r = parse(c).unwrap();
    assert_eq!(r.tcp_active_opens, 5);
    assert_eq!(r.udp_in_datagrams, 0);
    assert_eq!(r.udp_no_ports, 0);
    assert_eq!(r.udp_in_errors, 0);
    assert_eq!(r.udp_out_datagrams, 0);
}

#[test]
fn missing_key_within_group_yields_zero() {
    // Missing keys default to 0.
    let c = "\
Tcp: ActiveOpens PassiveOpens\n\
Tcp: 42 17\n\
Udp: InDatagrams OutDatagrams\n\
Udp: 10 20\n";
    let r = parse(c).unwrap();
    assert_eq!(r.tcp_active_opens, 42);
    assert_eq!(r.tcp_passive_opens, 17);
    assert_eq!(r.tcp_in_errs, 0);
    assert_eq!(r.udp_in_datagrams, 10);
}

#[test]
fn garbled_value_is_an_error() {
    let c = "\
Tcp: ActiveOpens\n\
Tcp: notanumber\n";
    assert!(parse(c).is_err());
}

#[test]
fn to_section_carries_all_fields_and_scope() {
    let c = "\
Tcp: RtoAlgorithm RtoMin RtoMax MaxConn ActiveOpens PassiveOpens AttemptFails EstabResets CurrEstab InSegs OutSegs RetransSegs InErrs OutRsts\n\
Tcp: 1 200 120000 -1 5 6 7 8 9 100 110 3 1 2\n\
Udp: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors\n\
Udp: 500 4 2 600 0 0\n";
    let section = parse(c).unwrap().to_section(3, 9_999);
    assert_eq!(section.ts.0, 9_999);
    assert_eq!(section.scope, 3);
    assert_eq!(section.tcp_active_opens, 5);
    assert_eq!(section.tcp_curr_estab, 9);
    assert_eq!(section.tcp_out_rsts, 2);
    assert_eq!(section.udp_in_datagrams, 500);
    assert_eq!(section.udp_no_ports, 4);
    assert_eq!(section.udp_out_datagrams, 600);
    assert_eq!(section.udp_in_errors, 2);
}
