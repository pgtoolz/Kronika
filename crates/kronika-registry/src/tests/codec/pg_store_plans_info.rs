use super::PgStorePlansInfo;
use crate::{Section, Ts};

#[test]
fn contract_and_codec_match_the_single_row_view() {
    let contract = PgStorePlansInfo::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_016_001);
    assert_eq!(contract.columns.len(), 3);
    assert_eq!(contract.sort_key, ["ts"]);
    crate::assert_roundtrips(&[PgStorePlansInfo {
        ts: Ts(2_000_000),
        dealloc: 5,
        stats_reset: Ts(1_000_000),
    }]);
}
