use super::OsProcessStatus;
use crate::{Section, Ts, contract::lint};

fn row(ts: i64, pid: i32) -> OsProcessStatus {
    OsProcessStatus {
        ts: Ts(ts),
        pid,
        starttime: Ts(1_700_000_000_000_000 + i64::from(pid)),
        vm_data: 10,
        vm_stk: 11,
        vm_lib: 12,
        vm_lck: 13,
        vm_pte: 14,
        vm_peak: 15,
        vm_hwm: 16,
        threads: 2,
        fdsize: 64,
        voluntary_ctxt_switches: 100,
        nonvoluntary_ctxt_switches: 7,
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsProcessStatus::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsProcessStatus::CONTRACT;
    assert_eq!(c.type_id.get(), 1_101_001);
    assert_eq!(c.sort_key, ["pid", "ts"]);
    assert_eq!(c.identity, ["pid"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[row(1, 10), row(2, 11)]);
}
