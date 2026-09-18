//! User names captured for process UIDs in one segment.
//!
//! Prepared rows remain pending until their window is written to the WAL.

use std::collections::BTreeMap;

use kronika_registry::{Section, Ts, os_user::OsUser};
use kronika_source_os::PasswdSnapshot;
use kronika_writer::Interner;

use super::io::{intern_str, log_degraded};
use crate::logging::log_count_degraded;

/// Maximum tracked (scope, UID) pairs, including written and unknown users.
const MAX_USERS_PER_SEGMENT: usize = 4 * 1024;
const USER_TYPE_ID: u32 = OsUser::CONTRACT.type_id.get();

#[derive(Debug, Default)]
enum PasswdState {
    #[default]
    Unread,
    Ready(PasswdSnapshot),
    Unavailable,
}

#[derive(Debug)]
enum UserNameState {
    PendingWrite,
    Written,
    NameNotFound,
}

/// Captures each observed user's name at most once per segment.
#[derive(Debug, Default)]
pub(crate) struct SegmentUserNames {
    passwd: PasswdState,
    users: BTreeMap<(u8, u32), UserNameState>,
    limit_reported: bool,
}

impl SegmentUserNames {
    /// Remember a process UID or effective UID within the segment's limit.
    pub(crate) fn observe_user(&mut self, scope: u8, uid: u32) {
        let key = (scope, uid);
        if self.users.contains_key(&key) {
            return;
        }
        if self.users.len() >= MAX_USERS_PER_SEGMENT {
            if !self.limit_reported {
                log_count_degraded(
                    USER_TYPE_ID,
                    "/etc/passwd",
                    "observed_uid_limit",
                    MAX_USERS_PER_SEGMENT,
                );
                self.limit_reported = true;
            }
            return;
        }
        self.users.insert(key, UserNameState::PendingWrite);
    }

    /// Build unconfirmed rows and their (scope, UID) keys.
    ///
    /// Preparation does not mark rows written. Pass the returned keys to
    /// [`Self::confirm_written`] only after the window is appended to the WAL.
    pub(crate) fn prepare_rows(
        &mut self,
        interner: &mut Interner,
        ts: i64,
    ) -> (Vec<OsUser>, Vec<(u8, u32)>) {
        self.load_passwd_once();
        let PasswdState::Ready(passwd) = &self.passwd else {
            return (Vec::new(), Vec::new());
        };
        let mut rows = Vec::new();
        let mut pending = Vec::new();
        for (&(scope, uid), state) in &mut self.users {
            if !matches!(state, UserNameState::PendingWrite) {
                continue;
            }
            let Some(username) = passwd.username(uid) else {
                *state = UserNameState::NameNotFound;
                continue;
            };
            let Some(username) = intern_str(interner, USER_TYPE_ID, "/etc/passwd", username) else {
                continue;
            };
            rows.push(OsUser {
                ts: Ts(ts),
                uid,
                username,
                scope,
            });
            pending.push((scope, uid));
        }
        (rows, pending)
    }

    /// Confirm exactly the keys returned by preparation after a successful WAL append.
    pub(crate) fn confirm_written(&mut self, keys: &[(u8, u32)]) {
        for key in keys {
            if let Some(state) = self.users.get_mut(key) {
                *state = UserNameState::Written;
            }
        }
    }

    fn load_passwd_once(&mut self) {
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
#[path = "../tests/os_sources/user_names.rs"]
mod tests;
