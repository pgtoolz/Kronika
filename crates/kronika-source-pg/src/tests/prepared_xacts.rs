use super::{PreparedXactsRow, to_prepared_xacts};
use crate::tests::intern as fake_intern;

#[test]
fn maps_every_field_and_interns_datname() {
    let r = PreparedXactsRow {
        ts: 2_000,
        datname: "appdb".to_owned(),
        prepared_count: 3,
        max_age_us: 4_200_000,
        max_xid_age_tx: 88,
    };
    let typed = to_prepared_xacts(&r, fake_intern).expect("infallible intern");
    assert_eq!(typed.ts.0, 2_000);
    assert_eq!(typed.prepared_count, 3);
    assert_eq!(typed.max_age_us, 4_200_000);
    assert_eq!(typed.max_xid_age_tx, 88);
    assert_eq!(typed.datname, fake_intern(b"appdb").unwrap());
}

#[test]
fn intern_failure_propagates() {
    let r = PreparedXactsRow {
        ts: 1,
        datname: "db".to_owned(),
        prepared_count: 1,
        max_age_us: 1,
        max_xid_age_tx: 1,
    };
    assert_eq!(to_prepared_xacts(&r, |_| Err("full")), Err("full"));
}
