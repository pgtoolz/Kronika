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
