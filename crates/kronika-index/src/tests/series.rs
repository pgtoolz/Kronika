use super::{
    ActiveBackendPoint, HealthPoint, SeriesBlock, TransactionPoint, pg_activity_layout,
    pg_database_layout,
};

#[test]
fn each_allowlisted_block_roundtrips() {
    let blocks = [
        SeriesBlock::OsHealth(vec![
            HealthPoint {
                timestamp: 10,
                value: None,
            },
            HealthPoint {
                timestamp: 20,
                value: Some(73),
            },
        ]),
        SeriesBlock::OverallHealth(vec![HealthPoint {
            timestamp: 20,
            value: Some(51),
        }]),
        SeriesBlock::PostgresHealth(vec![HealthPoint {
            timestamp: 20,
            value: Some(80),
        }]),
        SeriesBlock::PgTransactions {
            type_id: 1_005_004,
            points: vec![
                TransactionPoint {
                    timestamp: 10,
                    datid: 42,
                    value: None,
                },
                TransactionPoint {
                    timestamp: 20,
                    datid: 42,
                    value: Some(12.5),
                },
            ],
        },
        SeriesBlock::PgActiveBackends {
            type_id: 1_001_004,
            points: vec![ActiveBackendPoint {
                timestamp: 10,
                count: 4,
            }],
        },
    ];
    for block in blocks {
        let bytes = block.encode().expect("encode");
        assert_eq!(
            SeriesBlock::decode(block.key(), &bytes).expect("decode"),
            block
        );
    }
}

#[test]
fn a_real_zero_is_distinct_from_an_unknown_rate() {
    let block = SeriesBlock::PgTransactions {
        type_id: 1_005_003,
        points: vec![
            TransactionPoint {
                timestamp: 10,
                datid: 1,
                value: None,
            },
            TransactionPoint {
                timestamp: 20,
                datid: 1,
                value: Some(0.0),
            },
        ],
    };
    let bytes = block.encode().expect("encode");
    assert_eq!(
        SeriesBlock::decode(block.key(), &bytes).expect("decode"),
        block
    );
}

#[test]
fn statements_and_plans_are_not_index_layouts() {
    for type_id in [
        1_002_001, 1_002_002, 1_002_003, 1_002_004, 1_002_005, 1_002_006, 1_003_001, 1_004_001,
        1_018_001,
    ] {
        assert!(!pg_database_layout(type_id));
        assert!(!pg_activity_layout(type_id));
    }
}
