//! Name grammar and small pure helpers used while scanning.

use std::ffi::CStr;

use rustix::fs::FileType;

use crate::error::LayoutError;
use crate::time::{SegmentAddress, SegmentId, UtcDay};

use super::entry::{EntryFileType, EntryParent, ForeignEntryReason, PathIdentity, TemporaryKind};
use super::fsops::stat_no_follow;
use super::scan::ParsedLeaf;
use super::{
    ACTIVE_JOURNAL_NAME, DataRoot, FileIdentity, INDEX_OWNER_LOCK_NAME, LOG_OFFSETS_NAME,
    LOG_OFFSETS_TEMP_NAME, WRITER_OWNER_LOCK_NAME,
};

pub(super) fn is_control_name(name: &str) -> bool {
    matches!(
        name,
        ACTIVE_JOURNAL_NAME
            | WRITER_OWNER_LOCK_NAME
            | INDEX_OWNER_LOCK_NAME
            | LOG_OFFSETS_NAME
            | LOG_OFFSETS_TEMP_NAME
    )
}

pub(super) fn writer_lock_is_poisoned(root: &DataRoot) -> bool {
    stat_no_follow(&root.directory, WRITER_OWNER_LOCK_NAME)
        .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile)
}

pub(super) fn is_dot(name: &[u8]) -> bool {
    name == b"." || name == b".."
}

pub(super) fn ascii_name(name: &[u8]) -> Option<&str> {
    name.is_ascii()
        .then(|| std::str::from_utf8(name).ok())
        .flatten()
}

pub(super) const fn entry_file_type(file_type: FileType) -> EntryFileType {
    match file_type {
        FileType::RegularFile => EntryFileType::RegularFile,
        FileType::Directory => EntryFileType::Directory,
        FileType::Symlink => EntryFileType::Symlink,
        _ => EntryFileType::Other,
    }
}

pub(super) fn foreign_type_reason(file_type: FileType) -> ForeignEntryReason {
    if file_type == FileType::Symlink {
        ForeignEntryReason::SymbolicLink
    } else {
        ForeignEntryReason::UnsupportedType
    }
}

pub(super) fn path_identity(
    parent: EntryParent,
    name: &CStr,
    stat: &rustix::fs::Stat,
) -> PathIdentity {
    PathIdentity {
        scope: parent.scope(),
        name_hash: hash_name(name.to_bytes()),
        name_len: u16::try_from(name.to_bytes().len()).unwrap_or(u16::MAX),
        file_type: entry_file_type(FileType::from_raw_mode(stat.st_mode)),
        file: FileIdentity::from_stat(stat),
    }
}

pub(super) const fn hash_name(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

pub(super) fn parse_year(name: &str) -> Option<u16> {
    (name.len() == 4 && name.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| name.parse().ok())
        .flatten()
}

pub(super) fn parse_month(name: &str) -> Option<u8> {
    if name.len() != 2 || !name.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let month = name.parse().ok()?;
    (1..=12).contains(&month).then_some(month)
}

pub(super) fn parse_day(year: u16, month: u8, name: &str) -> Option<u8> {
    if name.len() != 2 || !name.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let day = name.parse().ok()?;
    UtcDay::new(year, month, day).ok().map(|valid| valid.day)
}

pub(super) fn parse_leaf(name: &str, day: UtcDay) -> Result<ParsedLeaf, LayoutError> {
    let mut fields = name.split('.');
    let fields = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    );
    let parsed = match fields {
        (Some(id), Some("zms"), None, None, None, None, None) => {
            ParsedLeaf::Zms(parse_address(id, day)?)
        }
        (Some(id), Some("idx"), None, None, None, None, None) => {
            ParsedLeaf::Idx(parse_address(id, day)?)
        }
        (Some(id), Some("zms"), Some(pid), Some(seq), Some("tmp"), None, None)
            if parse_canonical_u64(pid).is_some() && parse_canonical_u64(seq).is_some() =>
        {
            ParsedLeaf::Temporary(parse_address(id, day)?, TemporaryKind::Zms)
        }
        (Some(id), Some("idx"), Some(pid), Some(seq), Some("tmp"), None, None)
            if parse_canonical_u64(pid).is_some() && parse_canonical_u64(seq).is_some() =>
        {
            ParsedLeaf::Temporary(parse_address(id, day)?, TemporaryKind::Idx)
        }
        (Some(id), Some("idx"), Some("probe"), Some(pid), Some(seq), Some("tmp"), None)
            if parse_canonical_u64(pid).is_some() && parse_canonical_u64(seq).is_some() =>
        {
            ParsedLeaf::Temporary(parse_address(id, day)?, TemporaryKind::IndexProbe)
        }
        _ => {
            return Err(LayoutError::UnexpectedLeafEntry {
                name: name.to_owned(),
            });
        }
    };
    Ok(parsed)
}

pub(super) fn parse_address(id: &str, day: UtcDay) -> Result<SegmentAddress, LayoutError> {
    let value = parse_canonical_i64(id).ok_or_else(|| LayoutError::UnexpectedLeafEntry {
        name: id.to_owned(),
    })?;
    SegmentAddress::in_day(SegmentId::new(value)?, day)
}

pub(super) fn parse_canonical_i64(value: &str) -> Option<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || value == "-0"
        || (value.starts_with('0') && value.len() > 1)
        || (value.starts_with("-0") && value.len() > 2)
    {
        return None;
    }
    value.parse().ok()
}

pub(super) fn parse_canonical_u64(value: &str) -> Option<u64> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.starts_with('0') && value.len() > 1)
    {
        return None;
    }
    value.parse().ok()
}
