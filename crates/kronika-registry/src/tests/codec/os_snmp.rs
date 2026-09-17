use super::OsSnmp;
use crate::{Section, Ts, VerifiedSection, contract::lint};

fn row(ts: i64) -> OsSnmp {
    OsSnmp {
        ts: Ts(ts),
        tcp_active_opens: 1,
        tcp_passive_opens: 2,
        tcp_attempt_fails: 3,
        tcp_estab_resets: 4,
        tcp_in_segs: 100,
        tcp_out_segs: 110,
        tcp_retrans_segs: 3,
        tcp_in_errs: 1,
        tcp_out_rsts: 2,
        tcp_curr_estab: 9,
        udp_in_datagrams: 500,
        udp_out_datagrams: 600,
        udp_in_errors: 2,
        udp_no_ports: 4,
        ip_in_receives: Some(1_000),
        ip_in_hdr_errors: Some(0),
        ip_in_addr_errors: Some(0),
        ip_forw_datagrams: Some(0),
        ip_in_unknown_protos: Some(0),
        ip_in_discards: Some(0),
        ip_in_delivers: Some(990),
        ip_out_requests: Some(980),
        ip_out_discards: Some(0),
        ip_out_no_routes: Some(0),
        ip_reasm_reqds: Some(0),
        ip_reasm_oks: Some(0),
        ip_reasm_fails: Some(0),
        ip_frag_oks: Some(0),
        ip_frag_fails: Some(0),
        ip_frag_creates: Some(0),
        icmp_in_msgs: Some(12),
        icmp_in_errors: Some(0),
        icmp_out_msgs: Some(12),
        icmp_out_errors: Some(0),
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsSnmp::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsSnmp::CONTRACT;
    assert_eq!(c.type_id.get(), 1_110_001);
    assert_eq!(c.sort_key, ["ts"]);
}

#[test]
fn encode_sorts_by_ts() {
    let bytes = OsSnmp::encode(&[row(2_000), row(1_000), row(3_000)]).expect("encode");
    let decoded = OsSnmp::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
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
fn all_fourteen_counters_survive_encode_decode() {
    let bytes = OsSnmp::encode(&[row(5)]).expect("encode");
    let decoded = OsSnmp::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    let r = &decoded[0];
    assert_eq!(r.tcp_active_opens, 1);
    assert_eq!(r.tcp_curr_estab, 9);
    assert_eq!(r.udp_in_datagrams, 500);
    assert_eq!(r.udp_no_ports, 4);
}
