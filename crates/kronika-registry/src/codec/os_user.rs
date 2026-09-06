//! Type `1_124_002`: bounded UID-to-name references captured from `/etc/passwd`.

use crate::{Section, StrId, Ts};

/// A Linux user name observed while collecting process identities.
///
/// At most one row for a `(scope, uid)` pair is emitted in an open segment.
/// `username` is a reference into that segment's string dictionary. The source
/// is the collector-visible `/etc/passwd`; NSS and live reader-side lookups are
/// deliberately outside this contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_124_002,
    name = "os_user",
    semantics = on_change,
    sort_key("scope", "uid", "ts"),
    identity("scope", "uid")
)]
pub struct OsUser {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Numeric Linux user ID.
    #[column(l)]
    pub uid: u32,
    /// User name captured from the collector-visible `/etc/passwd`.
    #[column(l)]
    pub username: StrId,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
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
}
