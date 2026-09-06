use super::{Interner, Ts, intern_str, log_count_degraded, log_degraded};
use kronika_registry::os_user::OsUser;
use kronika_source_os::PasswdSnapshot;
use std::collections::BTreeSet;

/// Maximum distinct `(scope, uid)` references retained in one open segment.
pub(crate) const MAX_OBSERVED_USERS: usize = 4 * 1024;
const USER_TYPE_ID: u32 = 1_124_002;

#[derive(Debug)]
enum PasswdState {
    Unread,
    Ready(PasswdSnapshot),
    Unavailable,
}

/// Segment-local user-name capture and append bookkeeping.
#[derive(Debug)]
pub(crate) struct UserReferences {
    passwd: PasswdState,
    observed: BTreeSet<(u8, u32)>,
    recorded: BTreeSet<(u8, u32)>,
    unresolved: BTreeSet<(u8, u32)>,
    cardinality_reported: bool,
}

impl Default for UserReferences {
    fn default() -> Self {
        Self {
            passwd: PasswdState::Unread,
            observed: BTreeSet::new(),
            recorded: BTreeSet::new(),
            unresolved: BTreeSet::new(),
            cardinality_reported: false,
        }
    }
}

impl UserReferences {
    /// Retain one observed process UID within the fixed per-segment bound.
    pub(crate) fn observe(&mut self, scope: u8, uid: u32) {
        if self.observed.contains(&(scope, uid)) {
            return;
        }
        if self.observed.len() >= MAX_OBSERVED_USERS {
            if !self.cardinality_reported {
                log_count_degraded(
                    USER_TYPE_ID,
                    "/etc/passwd",
                    "observed_uid_limit",
                    MAX_OBSERVED_USERS,
                );
                self.cardinality_reported = true;
            }
            return;
        }
        self.observed.insert((scope, uid));
    }

    /// Observe current process identities and build still-unrecorded mappings.
    ///
    /// The returned keys must be passed to [`Self::mark_recorded`] only after
    /// the containing window has reached the WAL.
    pub(crate) fn prepare_rows(
        &mut self,
        interner: &mut Interner,
        scope: u8,
        ts: i64,
        uids: impl IntoIterator<Item = u32>,
    ) -> (Vec<OsUser>, Vec<(u8, u32)>) {
        for uid in uids {
            self.observe(scope, uid);
        }

        self.load_passwd();
        let PasswdState::Ready(passwd) = &self.passwd else {
            return (Vec::new(), Vec::new());
        };
        let candidates = self
            .observed
            .iter()
            .copied()
            .filter(|key| !self.recorded.contains(key) && !self.unresolved.contains(key))
            .collect::<Vec<_>>();
        let mut rows = Vec::with_capacity(candidates.len());
        let mut pending = Vec::with_capacity(candidates.len());
        let mut unresolved = Vec::new();
        for (row_scope, uid) in candidates {
            let Some(username) = passwd.username(uid) else {
                unresolved.push((row_scope, uid));
                continue;
            };
            let Some(username) = intern_str(interner, USER_TYPE_ID, "/etc/passwd", username) else {
                continue;
            };
            rows.push(OsUser {
                ts: Ts(ts),
                uid,
                username,
                scope: row_scope,
            });
            pending.push((row_scope, uid));
        }
        self.unresolved.extend(unresolved);
        (rows, pending)
    }

    /// Mark exact mappings durable after their normal window append succeeds.
    pub(crate) fn mark_recorded(&mut self, pending: &[(u8, u32)]) {
        self.recorded.extend(pending.iter().copied());
    }

    fn load_passwd(&mut self) {
        if !matches!(self.passwd, PasswdState::Unread) {
            return;
        }
        self.passwd = match PasswdSnapshot::system() {
            Ok(snapshot) => {
                if snapshot.rejected_lines() > 0 {
                    log_count_degraded(
                        USER_TYPE_ID,
                        "/etc/passwd",
                        "passwd_lines_rejected",
                        snapshot.rejected_lines(),
                    );
                }
                PasswdState::Ready(snapshot)
            }
            Err(error) => {
                log_degraded(USER_TYPE_ID, "/etc/passwd", &error);
                PasswdState::Unavailable
            }
        };
    }

    #[cfg(test)]
    pub(crate) fn with_passwd(passwd: PasswdSnapshot) -> Self {
        Self {
            passwd: PasswdState::Ready(passwd),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserReferences;
    use kronika_source_os::PasswdSnapshot;
    use kronika_writer::Interner;
    use std::io::Write;

    fn passwd() -> PasswdSnapshot {
        let mut file = tempfile::NamedTempFile::new().expect("tempfile");
        file.write_all(
            b"root:x:0:0:root:/root:/bin/sh\npostgres:x:26:26::/var/lib/postgresql:/bin/false\n",
        )
        .expect("write");
        PasswdSnapshot::read(file.path()).expect("read")
    }

    #[test]
    fn deduplicates_real_and_effective_uids_until_append_is_recorded() {
        let mut references = UserReferences::with_passwd(passwd());
        let mut interner = Interner::new(kronika_format::DictLimits::default());
        let (first, pending) = references.prepare_rows(&mut interner, 0, 1, [26, 26, 0, 999]);
        assert_eq!(first.iter().map(|row| row.uid).collect::<Vec<_>>(), [0, 26]);

        let (retry, _) = references.prepare_rows(&mut interner, 0, 2, [26]);
        assert_eq!(retry.len(), 2, "a failed append must remain retryable");

        references.mark_recorded(&pending);
        let (next, pending) = references.prepare_rows(&mut interner, 0, 3, [0, 26, 999]);
        assert!(next.is_empty());
        assert!(pending.is_empty());
    }
}
