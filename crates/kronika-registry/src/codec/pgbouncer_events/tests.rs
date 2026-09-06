use super::PgBouncerEvents;
use crate::{Section, StrId, Ts, VerifiedSection, lint};

const TS: i64 = 1_780_000_000_000_000;

fn event(ts: i64, level: u8, text: u64) -> PgBouncerEvents {
    PgBouncerEvents {
        ts: Ts(ts),
        source_file: StrId(9),
        level,
        database: Some(StrId(1)),
        username: Some(StrId(2)),
        host: Some(StrId(3)),
        text: StrId(text),
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[PgBouncerEvents::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let contract = PgBouncerEvents::CONTRACT;
    assert_eq!(contract.type_id.get(), 2_100_001);
    assert_eq!(contract.name, "pgbouncer_events");
    assert_eq!(contract.columns.len(), 7);
    assert_eq!(contract.sort_key, ["ts", "level", "text"]);
    assert!(
        contract.column("kind").is_none(),
        "the message text is the category"
    );
    assert_eq!(
        contract.column("text").map(|column| column.nullable),
        Some(false)
    );
}

#[test]
fn a_line_with_no_connection_behind_it_keeps_its_text() {
    let janitor = PgBouncerEvents {
        database: None,
        username: None,
        host: None,
        ..event(TS, 3, 4)
    };

    let bytes = PgBouncerEvents::encode(&[janitor]).expect("encode");
    let decoded = PgBouncerEvents::decode(VerifiedSection::for_test(bytes.into())).expect("decode");

    assert_eq!(decoded[0], janitor);
    assert_eq!(decoded[0].host, None);
}

#[test]
fn rows_sort_by_time_then_level_then_text() {
    let rows = [event(TS + 1, 3, 9), event(TS, 3, 8), event(TS, 0, 7)];

    let bytes = PgBouncerEvents::encode(&rows).expect("encode");
    let decoded = PgBouncerEvents::decode(VerifiedSection::for_test(bytes.into())).expect("decode");

    assert_eq!(
        decoded
            .iter()
            .map(|row| (row.ts.0, row.level, row.text.0))
            .collect::<Vec<_>>(),
        [(TS, 0, 7), (TS, 3, 8), (TS + 1, 3, 9)]
    );
}
