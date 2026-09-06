use super::{Finding, FindingBlock, FindingKind};
use crate::IndexError;

#[test]
fn event_and_known_bad_roundtrip_in_locator_order() {
    let block = FindingBlock {
        type_id: 2_004_001,
        total_hits: 2,
        truncated: false,
        findings: vec![
            Finding {
                kind: FindingKind::Event,
                category: None,
                field_ordinal: 0,
                row_ordinal: 7,
                timestamp: 10,
            },
            Finding {
                kind: FindingKind::KnownBad,
                category: None,
                field_ordinal: 6,
                row_ordinal: 7,
                timestamp: 10,
            },
        ],
    };
    let bytes = block.encode().expect("encode");
    assert_eq!(FindingBlock::decode(block.type_id, &bytes), Ok(block));
}

#[test]
fn error_event_category_roundtrips_and_rejects_invalid_bytes() {
    let block = FindingBlock {
        type_id: 2_001_001,
        total_hits: 1,
        truncated: false,
        findings: vec![Finding {
            kind: FindingKind::Event,
            category: Some(10),
            field_ordinal: 0,
            row_ordinal: 7,
            timestamp: 10,
        }],
    };
    let bytes = block.encode().expect("encode categorized event");
    assert_eq!(
        FindingBlock::decode(block.type_id, &bytes),
        Ok(block.clone())
    );

    let mut invalid = bytes.clone();
    *invalid.last_mut().expect("category byte") = 11;
    assert_eq!(
        FindingBlock::decode(block.type_id, &invalid),
        Err(IndexError::BadLayout)
    );

    let mut missing = bytes;
    *missing.last_mut().expect("category byte") = u8::MAX;
    assert_eq!(
        FindingBlock::decode(block.type_id, &missing),
        Err(IndexError::BadLayout)
    );

    let mut missing = block.clone();
    missing.findings[0].category = None;
    assert_eq!(missing.encode(), Err(IndexError::BadLayout));

    let mut other_layout = block;
    other_layout.type_id = 2_006_001;
    assert_eq!(other_layout.encode(), Err(IndexError::BadLayout));
}

#[test]
fn finding_block_rejects_out_of_order_and_truncated_payloads() {
    let block = FindingBlock {
        type_id: 1_100_001,
        total_hits: 2,
        truncated: false,
        findings: vec![
            Finding {
                kind: FindingKind::Spike,
                category: None,
                field_ordinal: 34,
                row_ordinal: 8,
                timestamp: 20,
            },
            Finding {
                kind: FindingKind::KnownBad,
                category: None,
                field_ordinal: 3,
                row_ordinal: 7,
                timestamp: 10,
            },
        ],
    };
    assert!(block.encode().is_err());

    let valid = FindingBlock {
        type_id: 1_100_001,
        total_hits: 0,
        truncated: false,
        findings: Vec::new(),
    }
    .encode()
    .expect("encode");
    assert!(FindingBlock::decode(1_100_001, &valid[..valid.len() - 1]).is_err());
}
