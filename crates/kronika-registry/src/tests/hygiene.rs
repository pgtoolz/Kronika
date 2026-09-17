use crate::{Section, StrId, Ts, VerifiedSection};

// These names collide with generated locals and tuple structs if hygiene
// regresses.
#[allow(
    non_snake_case,
    reason = "fields are deliberately named like the Ts/StrId types to test decode hygiene"
)]
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(id = 1_099_001, name = "hygiene probe", semantics = snapshot_full, sort_key("ts"))]
struct Weird {
    #[column(t)]
    ts: Ts,
    #[column(c, unit = count)]
    batch: i64,
    #[column(c, unit = count)]
    out: i64,
    #[column(c, unit = count)]
    i: i64,
    #[column(c, unit = count)]
    rows: Option<i64>,
    #[column(g, unit = count)]
    columns: bool,
    #[column(l)]
    label: StrId,
    #[column(c, unit = count)]
    Ts: i64,
    #[column(l)]
    StrId: u64,
}

#[test]
fn collision_named_fields_roundtrip() {
    let want = vec![
        Weird {
            ts: Ts(1),
            batch: 2,
            out: 3,
            i: 4,
            rows: Some(5),
            columns: true,
            label: StrId(10),
            Ts: 11,
            StrId: 12,
        },
        Weird {
            ts: Ts(6),
            batch: 7,
            out: 8,
            i: 9,
            rows: None,
            columns: false,
            label: StrId(13),
            Ts: 14,
            StrId: 15,
        },
    ];
    let bytes = Weird::encode(&want).expect("encode");
    assert_eq!(
        Weird::decode(VerifiedSection::for_test(bytes.into())).expect("decode"),
        want
    );
}
