use super::PgStatStatementsInfo;
use crate::{Section, Ts};

#[test]
fn contract_and_codec_match_the_single_row_view() {
    let contract = PgStatStatementsInfo::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_015_001);
    assert_eq!(contract.columns.len(), 3);
    assert_eq!(contract.sort_key, ["ts"]);
    crate::assert_roundtrips(&[PgStatStatementsInfo {
        ts: Ts(2_000_000),
        dealloc: 7,
        stats_reset: Ts(1_000_000),
    }]);
}
