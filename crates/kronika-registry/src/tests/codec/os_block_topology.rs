use super::OsBlockTopology;
use crate::{Section, Ts, contract::lint};

#[test]
fn contract_is_an_exact_edge_identity() {
    assert_eq!(lint(&[OsBlockTopology::CONTRACT]), Ok(()));
    assert_eq!(
        OsBlockTopology::CONTRACT.identity,
        ["major", "minor", "parent_major", "parent_minor"]
    );
    crate::assert_roundtrips(&[OsBlockTopology {
        ts: Ts(10),
        major: 252,
        minor: 0,
        parent_major: 259,
        parent_minor: 4,
        scope: 0,
    }]);
}
