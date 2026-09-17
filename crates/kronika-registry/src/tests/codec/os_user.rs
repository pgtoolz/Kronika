use super::OsUser;
use crate::{Section, Semantics, StrId, Ts, contract::lint};

#[test]
fn contract_shape_and_roundtrip() {
    let contract = OsUser::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_124_002);
    assert_eq!(contract.semantics, Semantics::OnChange);
    assert_eq!(contract.sort_key, ["scope", "uid", "ts"]);
    assert_eq!(contract.identity, ["scope", "uid"]);
    assert_eq!(lint(&[contract]), Ok(()));

    crate::assert_roundtrips(&[
        OsUser {
            ts: Ts(1),
            uid: 26,
            username: StrId(10),
            scope: 0,
        },
        OsUser {
            ts: Ts(2),
            uid: 1_000,
            username: StrId(11),
            scope: 3,
        },
    ]);
}
