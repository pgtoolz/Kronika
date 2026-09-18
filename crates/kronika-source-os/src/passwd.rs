//! Bounded parsing of the collector-visible `/etc/passwd`.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::Path;

/// Maximum bytes read from `/etc/passwd`.
pub const MAX_PASSWD_BYTES: usize = 256 * 1024;
/// Maximum accepted bytes in one `/etc/passwd` line.
pub const MAX_PASSWD_LINE_BYTES: usize = 4 * 1024;
/// Maximum accepted user records in one `/etc/passwd` file.
pub const MAX_PASSWD_ENTRIES: usize = 4 * 1024;
/// Maximum accepted user-name length in bytes.
pub const MAX_USERNAME_BYTES: usize = 256;

/// Bounded names parsed from one `/etc/passwd` read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdSnapshot {
    users: BTreeMap<u32, String>,
    rejected_lines: usize,
}

impl PasswdSnapshot {
    /// Read the production `/etc/passwd` source.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read within the fixed byte and
    /// entry bounds. Individual malformed or overlong lines are skipped.
    pub fn system() -> io::Result<Self> {
        Self::read(Path::new("/etc/passwd"))
    }

    /// Read and parse a passwd file at `path` with production bounds.
    ///
    /// # Errors
    /// Returns an error when the source cannot be read, exceeds
    /// [`MAX_PASSWD_BYTES`], or contains more than [`MAX_PASSWD_ENTRIES`]
    /// non-empty records.
    pub fn read(path: &Path) -> io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut bytes = Vec::with_capacity(MAX_PASSWD_BYTES.min(16 * 1024));
        file.by_ref()
            .take((MAX_PASSWD_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_PASSWD_BYTES {
            return Err(io::Error::other(format!(
                "{} exceeds {MAX_PASSWD_BYTES} byte passwd read limit",
                path.display()
            )));
        }
        parse(&bytes)
    }

    /// Resolve one exact numeric UID.
    #[must_use]
    pub fn username(&self, uid: u32) -> Option<&str> {
        self.users.get(&uid).map(String::as_str)
    }

    /// Number of accepted UID/name mappings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.users.len()
    }

    /// Whether no mappings were accepted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }

    /// Number of malformed or individually oversized records skipped.
    #[must_use]
    pub const fn rejected_lines(&self) -> usize {
        self.rejected_lines
    }
}

fn parse(bytes: &[u8]) -> io::Result<PasswdSnapshot> {
    let mut users = BTreeMap::new();
    let mut rejected_lines = 0_usize;
    let mut records = 0_usize;

    for raw in bytes.split(|byte| *byte == b'\n') {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        if raw.is_empty() {
            continue;
        }
        records = records.saturating_add(1);
        if records > MAX_PASSWD_ENTRIES {
            return Err(io::Error::other(format!(
                "passwd exceeds {MAX_PASSWD_ENTRIES} record limit"
            )));
        }
        if raw.len() > MAX_PASSWD_LINE_BYTES {
            rejected_lines = rejected_lines.saturating_add(1);
            continue;
        }
        let mut fields = raw.split(|byte| *byte == b':');
        let Some(name) = fields.next() else {
            rejected_lines = rejected_lines.saturating_add(1);
            continue;
        };
        let _password = fields.next();
        let Some(uid) = fields.next() else {
            rejected_lines = rejected_lines.saturating_add(1);
            continue;
        };
        if name.is_empty() || name.len() > MAX_USERNAME_BYTES {
            rejected_lines = rejected_lines.saturating_add(1);
            continue;
        }
        let (Ok(name), Ok(uid)) = (std::str::from_utf8(name), std::str::from_utf8(uid)) else {
            rejected_lines = rejected_lines.saturating_add(1);
            continue;
        };
        let Ok(uid) = uid.parse::<u32>() else {
            rejected_lines = rejected_lines.saturating_add(1);
            continue;
        };
        if fields.count() < 4 {
            rejected_lines = rejected_lines.saturating_add(1);
            continue;
        }
        users.entry(uid).or_insert_with(|| name.to_owned());
    }

    Ok(PasswdSnapshot {
        users,
        rejected_lines,
    })
}

#[cfg(test)]
#[path = "tests/passwd.rs"]
mod tests;
