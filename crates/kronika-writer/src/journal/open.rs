//! Opening a journal file and resetting it after a segment is written.

use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;

use kronika_format::{
    JOURNAL_HEADER_LEN, JournalHeader, JournalHeaderError, JournalState, RESET_MARKER_LEN,
    ResetMarker, scan_journal_streaming_strict_from,
};
use kronika_layout::{SegmentId, WriterOwner};

use super::error::map_scan_error;
use super::reset::{finish_committed_reset, recover_committed_reset, rollback, write_header};
use super::{
    Journal, JournalConfig, JournalError, JournalPartRef, next_journal_generation, validate_config,
};
use crate::journal_failpoint;

impl Journal {
    /// Opens or initializes the root journal through a writer-owner capability.
    ///
    /// Existing files are validated without truncation or repair. A newly
    /// created or zero-length file receives and synchronizes the canonical
    /// empty header before its root directory entry is synchronized.
    ///
    /// # Errors
    ///
    /// Returns a typed format, consistency, layout, or I/O error.
    pub fn open(owner: &WriterOwner, config: JournalConfig) -> Result<Self, JournalError> {
        validate_config(config)?;
        let owner_lease = owner.try_clone_lease()?;
        let (mut file, created) = owner.open_or_create_journal()?;
        if created {
            write_header(&mut file, JournalHeader::EMPTY)?;
            file.set_len(JOURNAL_HEADER_LEN as u64)?;
            file.sync_data()?;
        }

        let mut file_len = file.metadata()?.len();
        let max_journal_len = u64::try_from(config.max_journal_len).unwrap_or(u64::MAX);
        if file_len > max_journal_len {
            return Err(JournalError::JournalTooLarge {
                len: file_len,
                max: config.max_journal_len,
            });
        }
        if recover_committed_reset(&mut file, file_len, config)? {
            file_len = JOURNAL_HEADER_LEN as u64;
        }
        if file_len == 0 {
            // A crash between creation and the first header write leaves an
            // empty file; no journal state ever truncates below the header.
            write_header(&mut file, JournalHeader::EMPTY)?;
            file.set_len(JOURNAL_HEADER_LEN as u64)?;
            file.sync_data()?;
            file_len = JOURNAL_HEADER_LEN as u64;
        }
        if file_len < JOURNAL_HEADER_LEN as u64 {
            return Err(JournalError::TornHeader { len: file_len });
        }
        let mut header_bytes = [0_u8; JOURNAL_HEADER_LEN];
        file.read_exact_at(&mut header_bytes, 0)?;
        let header = JournalHeader::decode(header_bytes).map_err(|error| match error {
            JournalHeaderError::UnsupportedMagic { .. }
            | JournalHeaderError::UnsupportedVersion { .. } => {
                JournalError::UnsupportedJournalFormat
            }
            other => JournalError::InvalidHeader(other),
        })?;
        let actual_body_len = file_len - JOURNAL_HEADER_LEN as u64;
        if header.body_len != actual_body_len {
            return Err(JournalError::BodyLengthMismatch {
                recorded: header.body_len,
                actual: actual_body_len,
            });
        }

        let (segment_id, parts) = match header.state {
            JournalState::Empty => {
                if actual_body_len != 0 {
                    return Err(JournalError::EmptyWithFrames {
                        body_len: actual_body_len,
                    });
                }
                (None, Vec::new())
            }
            JournalState::Active { segment_id } => {
                let segment_id =
                    SegmentId::new(segment_id).map_err(JournalError::InvalidSegmentId)?;
                let scan = scan_journal_streaming_strict_from(
                    &file,
                    JOURNAL_HEADER_LEN as u64,
                    config.limits,
                    config.max_parts,
                )
                .map_err(map_scan_error)?;
                if u64::try_from(scan.valid_len).unwrap_or(u64::MAX) != file_len {
                    return Err(JournalError::DamagedBody);
                }
                if scan.parts.is_empty() {
                    return Err(JournalError::ActiveWithoutFirstFrame);
                }
                (Some(segment_id), scan.parts)
            }
        };
        let end = usize::try_from(file_len).map_err(|_overflow| {
            JournalError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "active.wal does not fit the address space",
            ))
        })?;
        // A preceding create may have synchronized the file but failed while
        // synchronizing its root entry. EEXIST alone does not mean durable.
        journal_failpoint!(OpenRootSync);
        owner.sync_root()?;
        let generation = next_journal_generation();
        let parts = parts
            .into_iter()
            .map(|raw| JournalPartRef::new(raw, generation))
            .collect();
        Ok(Self {
            _owner_lease: owner_lease,
            file,
            end,
            config,
            parts,
            generation,
            segment_id,
            poisoned: false,
        })
    }

    /// Commits a reset marker, then reduces the journal to the synchronized
    /// canonical empty version-1 header.
    ///
    /// A crash after the marker sync is completed by [`open`](Self::open).
    /// Until that marker is durable, a failed write is rolled back to the
    /// preceding active generation.
    ///
    /// # Errors
    ///
    /// Returns an I/O error. In-memory state changes only after persistence.
    pub fn reset(&mut self) -> Result<(), JournalError> {
        self.ensure_healthy()?;
        if self.parts.is_empty() {
            return Ok(());
        }
        let Some(segment_id) = self.segment_id else {
            self.poisoned = true;
            return Err(JournalError::Poisoned);
        };
        let next_generation = next_journal_generation();
        let marker_len = u64::try_from(self.end).map_err(|_overflow| JournalError::Full {
            len: self.end,
            max: self.config.max_journal_len,
        })?;
        if self
            .end
            .checked_add(RESET_MARKER_LEN)
            .is_none_or(|marker_end| marker_end > self.config.max_journal_len)
        {
            return Err(JournalError::Full {
                len: self.end,
                max: self.config.max_journal_len,
            });
        }
        let old_header = self.current_header();
        let Some(marker) = ResetMarker::new(marker_len, segment_id.get()) else {
            self.poisoned = true;
            return Err(JournalError::Poisoned);
        };
        let marker = marker.encode();

        let marker_result = (|| -> std::io::Result<()> {
            self.file.seek(SeekFrom::Start(self.end as u64))?;
            journal_failpoint!(ResetMarkerWrite);
            self.file.write_all(&marker)?;
            journal_failpoint!(ResetMarkerSync);
            self.file.sync_data()
        })();
        if let Err(error) = marker_result {
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

        if let Err(error) = finish_committed_reset(&mut self.file) {
            self.poisoned = true;
            return Err(JournalError::ResetIncomplete(error));
        }
        self.end = JOURNAL_HEADER_LEN;
        self.parts.clear();
        self.generation = next_generation;
        self.segment_id = None;
        Ok(())
    }
}
