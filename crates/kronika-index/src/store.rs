//! Validated targeted reads and atomic publication beside immutable segments.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::build::{BuildError, build_from_reader, build_selected_from_reader};
use crate::file::{Index, IndexError, TargetedIndex, read_all, read_target};
use crate::series::SeriesKey;
use kronika_layout::{DataRoot, LayoutError, LayoutLimits, OwnerKind, SegmentAddress, SegmentId};
use kronika_reader::{Reader, ReaderError, SegmentKind, SegmentRef};

/// Extension of an index file.
pub const EXTENSION: &str = "idx";
const SEGMENT_EXTENSION: &str = "zms";
const OWNER_RETRY: std::time::Duration = std::time::Duration::from_millis(200);

/// A validated resource and whether it came from an immutable sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceIndex {
    /// Selected blocks.
    pub index: TargetedIndex,
    /// `true` only for an immutable persisted IDX.
    pub persisted: bool,
}

/// Why a targeted index could not be read or rebuilt.
#[derive(Debug)]
pub enum LoadError {
    /// Ordinary file I/O failed.
    Io(io::Error),
    /// The index container or selected block was invalid.
    Bad(IndexError),
    /// Typed data-root access or publication failed.
    Layout(LayoutError),
    /// The production segment reader failed.
    Reader(ReaderError),
    /// Exact summary construction failed.
    Build(BuildError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Bad(error) => error.fmt(f),
            Self::Layout(error) => error.fmt(f),
            Self::Reader(error) => error.fmt(f),
            Self::Build(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Bad(error) => Some(error),
            Self::Layout(error) => Some(error),
            Self::Reader(error) => Some(error),
            Self::Build(error) => Some(error),
        }
    }
}

impl From<io::Error> for LoadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<LayoutError> for LoadError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

impl From<ReaderError> for LoadError {
    fn from(error: ReaderError) -> Self {
        Self::Reader(error)
    }
}

impl From<BuildError> for LoadError {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
    }
}

/// Where the index of a finished segment lives, for diagnostics and tests.
#[must_use]
pub fn path_of(segment: &Path) -> Option<PathBuf> {
    if segment.extension()? != SEGMENT_EXTENSION {
        return None;
    }
    Some(segment.with_extension(EXTENSION))
}

/// Decode a complete index file, primarily for diagnostics and tests.
///
/// # Errors
///
/// Returns I/O or container validation failures.
pub fn read(path: &Path) -> Result<Index, LoadError> {
    let mut file = std::fs::File::open(path).map_err(LoadError::Io)?;
    read_all(&mut file).map_err(LoadError::Bad)
}

/// Load selected blocks through the same validated resource path.
///
/// # Errors
///
/// Returns reader, index, layout, build, or publication failures.
pub fn resource_selected(
    root: &Path,
    reader: &Reader,
    segment_ref: &SegmentRef,
    keys: &[SeriesKey],
) -> Result<ResourceIndex, LoadError> {
    match segment_ref.kind() {
        SegmentKind::Active => {
            let segment = reader.open_segment(segment_ref)?;
            let index = build_selected_from_reader(reader, segment_ref, &segment, keys)?;
            Ok(ResourceIndex {
                index: targeted(index, keys, None),
                persisted: false,
            })
        }
        SegmentKind::Finished => {
            let data_root = DataRoot::open(root)?;
            let address = address_of(segment_ref.id())?;
            // Give a concurrent publisher one short recheck.
            for wait in [true, false] {
                if let Some(mut file) = data_root.open_idx(address)?
                    && let Ok(selected) = read_target(&mut file, keys)
                    && selected.contains_targets(keys)
                {
                    return Ok(ResourceIndex {
                        index: selected,
                        persisted: true,
                    });
                }

                match data_root.acquire_index(LayoutLimits::default()) {
                    Ok(owner) => {
                        // Publication revalidates this source identity.
                        let mut temporary = owner.create_idx_temp(address)?;
                        let segment = reader.open_segment(segment_ref)?;
                        let index = build_from_reader(reader, segment_ref, &segment)?;
                        let bytes = index.encode().map_err(LoadError::Bad)?;
                        let checksum = encoded_checksum(&bytes)?;
                        temporary.file_mut().write_all(&bytes)?;
                        drop(temporary.try_clone_file()?);
                        temporary.publish()?;
                        return Ok(ResourceIndex {
                            index: targeted(index, keys, Some(checksum)),
                            persisted: true,
                        });
                    }
                    // The root-wide writer lock avoids duplicate builds.
                    Err(LayoutError::OwnerContended {
                        owner: OwnerKind::Index,
                    }) if wait => std::thread::sleep(OWNER_RETRY),
                    Err(LayoutError::OwnerContended {
                        owner: OwnerKind::Index,
                    }) => break,
                    Err(error) => return Err(LoadError::Layout(error)),
                }
            }

            // Build locally when the publisher still holds the lock.
            let segment = reader.open_segment(segment_ref)?;
            let index = build_from_reader(reader, segment_ref, &segment)?;
            let bytes = index.encode().map_err(LoadError::Bad)?;
            let checksum = encoded_checksum(&bytes)?;
            Ok(ResourceIndex {
                index: targeted(index, keys, Some(checksum)),
                persisted: false,
            })
        }
    }
}

fn address_of(raw_id: i64) -> Result<SegmentAddress, LayoutError> {
    SegmentAddress::new(SegmentId::new(raw_id)?)
}

fn targeted(index: Index, keys: &[SeriesKey], checksum: Option<u32>) -> TargetedIndex {
    let wanted: std::collections::HashSet<SeriesKey> = keys.iter().copied().collect();
    TargetedIndex {
        checksum,
        blocks: index
            .blocks
            .into_iter()
            .filter(|block| wanted.contains(&block.key()))
            .collect(),
    }
}

fn encoded_checksum(bytes: &[u8]) -> Result<u32, LoadError> {
    let raw: [u8; 4] = bytes
        .get(12..16)
        .ok_or(LoadError::Bad(IndexError::Truncated))?
        .try_into()
        .map_err(|_error| LoadError::Bad(IndexError::Truncated))?;
    Ok(u32::from_le_bytes(raw))
}

#[cfg(test)]
mod tests;
