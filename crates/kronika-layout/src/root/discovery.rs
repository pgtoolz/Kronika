//! The strict, bounded walk of the calendar tree.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use rustix::fs::{Dir, FileType};

use crate::error::{LayoutError, LimitKind};
use crate::time::{SegmentAddress, SegmentId, UtcDay};

use super::entry::{
    EntryParent, ForeignEntryReason, LayoutSnapshot, SegmentArtifacts, TemporaryObject,
};
use super::fsops::{errno_to_layout, open_directory_at, stat_no_follow_name};
use super::names::{
    ascii_name, foreign_type_reason, is_control_name, is_dot, parse_day, parse_leaf, parse_month,
    parse_year,
};
use super::scan::{DayArtifacts, ParsedLeaf, ScanState};
use super::{DataRoot, ENTRY_METADATA_BYTES, FileIdentity, LayoutLimits};

impl DataRoot {
    pub(super) fn scan_once(
        &self,
        limits: LayoutLimits,
        visited_entries: &mut usize,
    ) -> Result<LayoutSnapshot, LayoutError> {
        let mut state = ScanState::new(limits, visited_entries);
        let mut root_entries = Dir::read_from(&*self.directory).map_err(errno_to_layout)?;
        for entry in &mut root_entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(&self.directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    ForeignEntryReason::NonAsciiName,
                )?;
                continue;
            };
            if is_control_name(name_string) {
                if file_type != FileType::RegularFile {
                    state.record_foreign(
                        EntryParent::Root,
                        name,
                        &stat,
                        foreign_type_reason(file_type),
                    )?;
                }
                continue;
            }
            let root_extension = Path::new(name_string).extension();
            if root_extension.is_some_and(|extension| extension == "zms" || extension == "idx") {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    if file_type == FileType::RegularFile {
                        ForeignEntryReason::UnsupportedFlatArtifact
                    } else {
                        foreign_type_reason(file_type)
                    },
                )?;
                continue;
            }
            let Some(year) = parse_year(name_string) else {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    if file_type == FileType::Symlink {
                        ForeignEntryReason::SymbolicLink
                    } else {
                        ForeignEntryReason::UnsupportedName
                    },
                )?;
                continue;
            };
            if file_type != FileType::Directory {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    foreign_type_reason(file_type),
                )?;
                continue;
            }
            let year_directory = open_directory_at(&self.directory, name_string)?;
            Self::scan_year(&year_directory, year, &mut state)?;
        }
        Ok(state.finish())
    }

    fn scan_year(
        year_directory: &File,
        year: u16,
        state: &mut ScanState<'_>,
    ) -> Result<(), LayoutError> {
        let mut entries = Dir::read_from(year_directory).map_err(errno_to_layout)?;
        for entry in &mut entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(year_directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let parent = EntryParent::Year(year);
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(parent, name, &stat, ForeignEntryReason::NonAsciiName)?;
                continue;
            };
            let Some(month) = parse_month(name_string) else {
                state.record_foreign(
                    parent,
                    name,
                    &stat,
                    if file_type == FileType::Symlink {
                        ForeignEntryReason::SymbolicLink
                    } else {
                        ForeignEntryReason::UnsupportedName
                    },
                )?;
                continue;
            };
            if file_type != FileType::Directory {
                state.record_foreign(parent, name, &stat, foreign_type_reason(file_type))?;
                continue;
            }
            let month_directory = open_directory_at(year_directory, name_string)?;
            Self::scan_month(&month_directory, year, month, state)?;
        }
        Ok(())
    }

    fn scan_month(
        month_directory: &File,
        year: u16,
        month: u8,
        state: &mut ScanState<'_>,
    ) -> Result<(), LayoutError> {
        let mut entries = Dir::read_from(month_directory).map_err(errno_to_layout)?;
        for entry in &mut entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(month_directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let parent = EntryParent::Month { year, month };
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(parent, name, &stat, ForeignEntryReason::NonAsciiName)?;
                continue;
            };
            let Some(day_number) = parse_day(year, month, name_string) else {
                state.record_foreign(
                    parent,
                    name,
                    &stat,
                    if file_type == FileType::Symlink {
                        ForeignEntryReason::SymbolicLink
                    } else {
                        ForeignEntryReason::UnsupportedName
                    },
                )?;
                continue;
            };
            if file_type != FileType::Directory {
                state.record_foreign(parent, name, &stat, foreign_type_reason(file_type))?;
                continue;
            }
            let day_directory = open_directory_at(month_directory, name_string)?;
            let day = UtcDay::new(year, month, day_number)?;
            state.account_metadata(size_of::<UtcDay>())?;
            state.days.push(day);
            Self::scan_day(&day_directory, day, state)?;
        }
        Ok(())
    }

    fn scan_day(
        day_directory: &File,
        day: UtcDay,
        state: &mut ScanState<'_>,
    ) -> Result<(), LayoutError> {
        let mut entries = Dir::read_from(day_directory).map_err(errno_to_layout)?;
        let mut day_entries = 0_usize;
        let mut finals: BTreeMap<SegmentId, DayArtifacts> = BTreeMap::new();
        for entry in &mut entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            day_entries =
                day_entries
                    .checked_add(1)
                    .ok_or(LayoutError::TraversalLimitExceeded {
                        kind: LimitKind::EntriesPerDay,
                        limit: state.limits.max_entries_per_day,
                    })?;
            if day_entries > state.limits.max_entries_per_day {
                return Err(LayoutError::TraversalLimitExceeded {
                    kind: LimitKind::EntriesPerDay,
                    limit: state.limits.max_entries_per_day,
                });
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(day_directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let parent = EntryParent::Day(day);
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(parent, name, &stat, ForeignEntryReason::NonAsciiName)?;
                continue;
            };
            if file_type != FileType::RegularFile {
                state.record_foreign(parent, name, &stat, foreign_type_reason(file_type))?;
                continue;
            }
            let parsed = match parse_leaf(name_string, day) {
                Ok(parsed) => parsed,
                Err(error) => {
                    state.record_foreign(
                        parent,
                        name,
                        &stat,
                        if matches!(error, LayoutError::MisbucketedSegment { .. }) {
                            ForeignEntryReason::MisbucketedSegment
                        } else {
                            ForeignEntryReason::UnsupportedName
                        },
                    )?;
                    continue;
                }
            };
            let identity = FileIdentity::from_stat(&stat);
            let bytes = identity.len;
            match parsed {
                ParsedLeaf::Zms(address) => {
                    finals.entry(address.id).or_default().zms = Some(identity);
                }
                ParsedLeaf::Idx(address) => {
                    finals.entry(address.id).or_default().idx = Some(bytes);
                }
                ParsedLeaf::Temporary(address, kind) => {
                    state.account_metadata(ENTRY_METADATA_BYTES)?;
                    state.temporaries.push(TemporaryObject {
                        address,
                        kind,
                        identity,
                        file_name: name_string.to_owned(),
                    });
                }
            }
        }

        for (id, artifacts) in finals {
            let address = SegmentAddress::in_day(id, day)?;
            if let Some(zms_identity) = artifacts.zms {
                state.account_segment()?;
                state.segments.push(SegmentArtifacts {
                    address,
                    zms_identity,
                    zms_bytes: zms_identity.len,
                    idx_bytes: artifacts.idx,
                });
            } else if artifacts.idx.is_some() {
                state.account_metadata(size_of::<SegmentAddress>())?;
                state.orphan_indexs.push(address);
            }
        }
        Ok(())
    }
}
