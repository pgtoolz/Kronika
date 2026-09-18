use super::OsStat;
use crate::{Section, Ts, contract::lint};

fn row(ts: i64) -> OsStat {
    OsStat {
        ts: Ts(ts),
        ctxt: 1_234_567,
        processes: 42,
        procs_running: 3,
        procs_blocked: 1,
        btime: Ts(1_700_000_000_000_000),
        intr_total: Some(9_000_000),
        softirq_total: Some(8_000_000),
        uptime_us: Some(3_600_000_000),
        idle_us: Some(28_000_000_000),
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsStat::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsStat::CONTRACT;
    assert_eq!(c.type_id.get(), 1_103_001);
    assert_eq!(c.sort_key, ["ts"]);
}

#[test]
fn encode_sorts_by_ts() {
    let bytes = OsStat::encode(&[row(2_000), row(1_000), row(3_000)]).expect("encode");
    let decoded =
        OsStat::decode(kronika_registry::VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded.iter().map(|r| r.ts.0).collect::<Vec<_>>(),
        [1_000, 2_000, 3_000]
    );
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[row(1_000), row(2_000)]);
}
