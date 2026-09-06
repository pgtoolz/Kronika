//! Version-1 file-backed `active.wal` journal.
//!
//! The checksummed root header persists the exact [`SegmentId`] independently
//! of the ZMS catalogs carried by its frames. A fresh journal always contains a
//! valid empty header. A zero-length file provably never held data and is
//! re-initialized in place; headerless frame-only files are rejected without
//! modification.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU64, Ordering};

use kronika_format::{
    FRAME_HEADER_LEN, FrameHeader, JOURNAL_HEADER_LEN, JournalHeader, JournalHeaderError,
    JournalLimits, JournalScanError, JournalState, MAX_JOURNAL_LEN, MAX_JOURNAL_PARTS,
    MAX_PART_LEN, PartError, PartRef, RESET_MARKER_LEN, ResetMarker,
    scan_journal_streaming_strict_from, validate_part,
};
use kronika_layout::{LayoutError, SegmentId, WriterLease};

mod error;
pub(crate) mod faults;
mod open;
mod reset;

pub use error::JournalError;
use error::map_scan_error;
#[cfg(test)]
use faults::{JournalFaultPoint, arm_journal_faults};
use reset::{rollback, write_header};
static NEXT_JOURNAL_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Injects a test-only failure at one durability step.
#[macro_export]
macro_rules! journal_failpoint {
    ($point:ident) => {
        #[cfg(test)]
        $crate::journal::faults::inject_journal_fault(
            $crate::journal::faults::JournalFaultPoint::$point,
        )?;
    };
}

/// Configuration of one journal file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalConfig {
    /// Frame-level limits shared with the scanner.
    pub limits: JournalLimits,
    /// Cap for the complete journal file, including its versioned header.
    pub max_journal_len: usize,
    /// Cap for valid frames retained in one active generation.
    pub max_parts: usize,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            limits: JournalLimits::default(),
            max_journal_len: MAX_JOURNAL_LEN,
            max_parts: MAX_JOURNAL_PARTS,
        }
    }
}

/// Opaque reference to one part in a specific in-process journal generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalPartRef {
    raw: PartRef,
    generation: u64,
}

impl JournalPartRef {
    const fn new(raw: PartRef, generation: u64) -> Self {
        Self { raw, generation }
    }

    /// Offset of the part body in `active.wal`.
    #[must_use]
    pub const fn offset(self) -> usize {
        self.raw.offset
    }

    /// Length of the part body, bytes.
    #[must_use]
    pub const fn len(self) -> usize {
        self.raw.len
    }

    /// Whether the referenced part body is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.raw.len == 0
    }
}

/// Open active journal owned by exactly one collector process.
#[derive(Debug)]
pub struct Journal {
    _owner_lease: WriterLease,
    file: File,
    end: usize,
    config: JournalConfig,
    parts: Vec<JournalPartRef>,
    generation: u64,
    segment_id: Option<SegmentId>,
    poisoned: bool,
}

impl Journal {
    /// Complete file size, including the versioned header.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.end
    }

    /// Identity of the active segment, or `None` for the empty header.
    #[must_use]
    pub const fn segment_id(&self) -> Option<SegmentId> {
        self.segment_id
    }

    /// Appends one ZMS part under the exact active segment identity.
    ///
    /// The first append writes the identity header and first frame before one
    /// shared synchronization boundary. Later appends reject another identity.
    ///
    /// # Errors
    ///
    /// Returns a validation, capacity, identity, or I/O error. A transient I/O
    /// error is rolled back to the preceding valid generation. A failed
    /// rollback permanently poisons this handle.
    pub fn append(
        &mut self,
        segment_id: SegmentId,
        part: &[u8],
    ) -> Result<JournalPartRef, JournalError> {
        self.ensure_healthy()?;
        if let Some(expected) = self.segment_id
            && expected != segment_id
        {
            return Err(JournalError::SegmentIdMismatch {
                expected,
                got: segment_id,
            });
        }
        let part_len = part.len() as u64;
        if part_len > self.config.limits.max_part_len {
            return Err(JournalError::PartTooLarge {
                len: part.len(),
                max: self.config.limits.max_part_len,
            });
        }
        // Validate before flow control. A full journal must not cause the
        // caller to publish/reset valid accumulated data for an invalid
        // incoming part.
        validate_part(part).map_err(JournalError::InvalidPart)?;

        if self.parts.len() >= self.config.max_parts {
            return Err(JournalError::TooManyParts {
                max: self.config.max_parts,
            });
        }
        let frame_len = FRAME_HEADER_LEN
            .checked_add(part.len())
            .ok_or(JournalError::Full {
                len: self.end,
                max: self.config.max_journal_len,
            })?;
        let next_end = self.end.checked_add(frame_len).ok_or(JournalError::Full {
            len: self.end,
            max: self.config.max_journal_len,
        })?;
        let reset_end = next_end
            .checked_add(RESET_MARKER_LEN)
            .ok_or(JournalError::Full {
                len: self.end,
                max: self.config.max_journal_len,
            })?;
        if reset_end > self.config.max_journal_len {
            return Err(JournalError::Full {
                len: self.end,
                max: self.config.max_journal_len,
            });
        }
        let frame_header = FrameHeader { part_len }.encode();
        let old_header = self.current_header();
        let body_len = u64::try_from(next_end - JOURNAL_HEADER_LEN).map_err(|_overflow| {
            JournalError::Full {
                len: self.end,
                max: self.config.max_journal_len,
            }
        })?;
        let new_header = JournalHeader {
            state: JournalState::Active {
                segment_id: segment_id.get(),
            },
            body_len,
        };
        let write_result = (|| -> std::io::Result<()> {
            if self.parts.is_empty() {
                journal_failpoint!(AppendHeaderWrite);
                write_header(&mut self.file, new_header)?;
                self.write_frame(self.end, &frame_header, part)?;
            } else {
                self.write_frame(self.end, &frame_header, part)?;
                journal_failpoint!(AppendHeaderWrite);
                write_header(&mut self.file, new_header)?;
            }
            journal_failpoint!(AppendSync);
            self.file.sync_data()
        })();
        if let Err(error) = write_result {
            return match rollback(&mut self.file, self.end, old_header) {
                Ok(()) => Err(JournalError::Io(error)),
                Err(rollback) => {
                    self.poisoned = true;
                    Err(JournalError::RollbackFailed {
                        operation: error,
                        rollback,
                    })
                }
            };
        }

        let part_ref = JournalPartRef::new(
            PartRef {
                offset: self.end + FRAME_HEADER_LEN,
                len: part.len(),
            },
            self.generation,
        );
        self.end = next_end;
        self.parts.push(part_ref);
        self.segment_id = Some(segment_id);
        Ok(part_ref)
    }

    fn write_frame(&mut self, at: usize, header: &[u8], part: &[u8]) -> Result<(), std::io::Error> {
        self.file.seek(SeekFrom::Start(at as u64))?;
        journal_failpoint!(AppendFrameHeaderWrite);
        self.file.write_all(header)?;
        journal_failpoint!(AppendFrameBodyWrite);
        self.file.write_all(part)
    }

    fn current_header(&self) -> JournalHeader {
        JournalHeader {
            state: self
                .segment_id
                .map_or(JournalState::Empty, |segment_id| JournalState::Active {
                    segment_id: segment_id.get(),
                }),
            body_len: u64::try_from(self.end - JOURNAL_HEADER_LEN).unwrap_or(u64::MAX),
        }
    }

    /// Valid frame bodies in journal order.
    #[must_use]
    pub fn parts(&self) -> &[JournalPartRef] {
        &self.parts
    }

    /// Reads one referenced part body.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError::StalePartRef`] for a reference from another
    /// generation, or an I/O error.
    pub fn read_part(&self, part: JournalPartRef) -> Result<Vec<u8>, JournalError> {
        self.check_part_ref(part)?;
        let mut body = vec![0_u8; part.raw.len];
        self.file.read_exact_at(&mut body, part.raw.offset as u64)?;
        Ok(body)
    }

    /// Read a bounded byte range relative to one part body.
    ///
    /// Writing uses this after catalog validation to read one bounded Parquet
    /// body without allocating the rest of its journal part.
    pub(crate) fn read_part_range(
        &self,
        part: JournalPartRef,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, JournalError> {
        self.check_part_ref(part)?;
        let Some(relative_end) = offset.checked_add(len) else {
            return Err(JournalError::StalePartRef { offset, len });
        };
        if relative_end > part.raw.len {
            return Err(JournalError::StalePartRef { offset, len });
        }
        let absolute = part
            .raw
            .offset
            .checked_add(offset)
            .ok_or(JournalError::StalePartRef { offset, len })?;
        let mut body = vec![0_u8; len];
        self.file.read_exact_at(&mut body, absolute as u64)?;
        Ok(body)
    }

    fn check_part_ref(&self, part: JournalPartRef) -> Result<(), JournalError> {
        self.ensure_healthy()?;
        let minimum = JOURNAL_HEADER_LEN + FRAME_HEADER_LEN;
        let is_member = part.generation == self.generation
            && self
                .parts
                .binary_search_by_key(&part.raw.offset, |candidate| candidate.raw.offset)
                .is_ok_and(|index| self.parts[index] == part);
        let in_bounds = part.raw.offset >= minimum
            && part
                .raw
                .offset
                .checked_add(part.raw.len)
                .is_some_and(|end| end <= self.end);
        if !is_member || !in_bounds {
            return Err(JournalError::StalePartRef {
                offset: part.raw.offset,
                len: part.raw.len,
            });
        }
        Ok(())
    }

    /// Complete journal length, including the header.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.end
    }

    /// Whether the journal is in its canonical empty state.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Whether a partial persistence failure made further use unsafe.
    #[must_use]
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    const fn ensure_healthy(&self) -> Result<(), JournalError> {
        if self.poisoned {
            Err(JournalError::Poisoned)
        } else {
            Ok(())
        }
    }
}

pub(crate) const fn validate_config(config: JournalConfig) -> Result<(), JournalError> {
    if config.max_journal_len < JOURNAL_HEADER_LEN || config.max_journal_len > MAX_JOURNAL_LEN {
        return Err(JournalError::InvalidMaxJournalLen {
            value: config.max_journal_len,
            minimum: JOURNAL_HEADER_LEN,
            maximum: MAX_JOURNAL_LEN,
        });
    }
    if config.max_parts == 0 || config.max_parts > MAX_JOURNAL_PARTS {
        return Err(JournalError::InvalidMaxParts {
            value: config.max_parts,
            minimum: 1,
            maximum: MAX_JOURNAL_PARTS,
        });
    }
    if config.limits.max_part_len == 0 || config.limits.max_part_len > MAX_PART_LEN {
        return Err(JournalError::InvalidMaxPartLen {
            value: config.limits.max_part_len,
            minimum: 1,
            maximum: MAX_PART_LEN,
        });
    }
    Ok(())
}

fn next_journal_generation() -> u64 {
    NEXT_JOURNAL_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .expect("journal generation counter exhausted")
}

#[cfg(test)]
mod tests;
