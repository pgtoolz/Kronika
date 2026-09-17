use super::PgSettings;
use crate::{Section, Semantics, StrId, Ts};

fn row(name: u64) -> PgSettings {
    PgSettings {
        ts: Ts(1_000_000),
        datid: 16_384,
        datname: StrId(2),
        usesysid: 16_385,
        usename: StrId(3),
        name: StrId(name),
        setting: StrId(10),
        unit: Some(StrId(11)),
        source: StrId(12),
        sourcefile: None,
        sourceline: None,
        pending_restart: false,
        context: StrId(13),
        vartype: StrId(14),
        boot_val: Some(StrId(15)),
        reset_val: Some(StrId(16)),
    }
}

#[test]
fn contract_shape() {
    let c = PgSettings::CONTRACT;
    assert_eq!(c.type_id.get(), 1_019_001);
    assert_eq!(c.columns.len(), 16);
    assert_eq!(c.semantics, Semantics::OnChange);
    assert_eq!(c.sort_key, ["datid", "usesysid", "name"]);
    assert_eq!(c.identity, ["datid", "usesysid", "name"]);
    assert_eq!(c.column("datid").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("datname").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("usesysid").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("usename").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("name").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("unit").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("boot_val").map(|col| col.nullable), Some(true));
    assert_eq!(
        c.column("pending_restart").map(|col| col.nullable),
        Some(false)
    );
}

#[test]
fn roundtrip_preserves_values_and_nulls() {
    let mut file_backed = row(2);
    file_backed.sourcefile = Some(StrId(20));
    file_backed.sourceline = Some(42);
    file_backed.pending_restart = true;
    file_backed.unit = None;
    // Interner hands out ids in insertion order, and rows arrive from the
    // server sorted by name, so ordering by the `name` id is ordering by
    // name.
    crate::assert_roundtrips(&[row(1), file_backed]);
}
