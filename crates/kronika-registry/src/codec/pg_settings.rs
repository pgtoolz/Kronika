//! Type `1_019_001`: `pg_settings`, the metric session configuration snapshot.
//!
//! Roughly 350 rows. Every segment carries a full copy, so a segment stays
//! self-contained and a reader never goes back to an older one for the
//! configuration. `setting` is the value in the unit named by `unit`:
//! `work_mem` is stored as `4096` with `kB`, not as `4MB`.

use crate::{Section, StrId, Ts};

/// One row of type `1_019_001`; one `pg_settings` entry.
///
/// `pending_restart` is `true` when a changed value (e.g. via `ALTER SYSTEM`
/// plus reload) takes effect only after a server restart — the stored
/// `setting` still shows the running value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_019_001,
    name = "pg_settings",
    semantics = on_change,
    sort_key("datid", "usesysid", "name"),
    identity("datid", "usesysid", "name")
)]
pub struct PgSettings {
    /// Collection time, unix microseconds; one value for all rows of a read.
    #[column(t)]
    pub ts: Ts,
    /// OID of the database used by the metric session.
    #[column(l)]
    pub datid: u32,
    /// Name of the database used by the metric session.
    #[column(l)]
    pub datname: StrId,
    /// OID of the login role used by the metric session.
    #[column(l)]
    pub usesysid: u32,
    /// Name of the login role used by the metric session.
    #[column(l)]
    pub usename: StrId,
    /// Parameter name.
    #[column(l)]
    pub name: StrId,
    /// Running value, in `unit` units.
    #[column(l)]
    pub setting: StrId,
    /// Unit of `setting`; `None` for unitless parameters.
    #[column(l)]
    pub unit: Option<StrId>,
    /// How the running value was set (`default`, `configuration file`, …).
    #[column(l)]
    pub source: StrId,
    /// Config file that set the value; `None` unless set from a file.
    #[column(l)]
    pub sourcefile: Option<StrId>,
    /// Line within `sourcefile`; `None` unless set from a file.
    #[column(l)]
    pub sourceline: Option<i32>,
    /// The value changed but takes effect only after a restart.
    #[column(l)]
    pub pending_restart: bool,
    /// Required context to change the value (`postmaster`, `user`, …).
    #[column(l)]
    pub context: StrId,
    /// Value type (`bool`, `integer`, `real`, `string`, `enum`).
    #[column(l)]
    pub vartype: StrId,
    /// Compiled-in default; `None` when the server reports none.
    #[column(l)]
    pub boot_val: Option<StrId>,
    /// Value `RESET` would restore; `None` when the server reports none.
    #[column(l)]
    pub reset_val: Option<StrId>,
}

#[cfg(test)]
mod tests {
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
}
