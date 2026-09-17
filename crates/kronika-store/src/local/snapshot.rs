//! Opening and reading generation-pinned active-journal prefixes.

use std::fs::File;
use std::io;
use std::sync::Arc;

use kronika_format::{JOURNAL_HEADER_LEN, MAX_PART_LEN, ReadAt};

use super::budget::layout_io;
use super::journal::{validate_active_part_reference, validate_active_snapshot};
use super::{ActivePart, ActiveSnapshot, LocalDir, LocalScan, StoreError};

impl LocalDir {
    /// Opens the root-level active journal for snapshot identity and prefix
    /// checks.
    ///
    /// # Errors
    ///
    /// Returns an error when the existing journal is unsafe or unreadable.
    pub fn open_active(&self) -> io::Result<Option<File>> {
        self.root.open_active_journal().map_err(layout_io)
    }

    /// Open the exact active prefix described by `scan`.
    ///
    /// Later appends to the same generation are allowed but remain outside the
    /// snapshot. A reset or generation change makes subsequent reads stale.
    ///
    /// # Errors
    ///
    /// Returns an error when the captured generation is no longer readable.
    pub fn open_active_snapshot(
        &self,
        scan: &LocalScan,
    ) -> Result<Option<ActiveSnapshot>, StoreError> {
        let Some(first) = scan.active.first() else {
            return Ok(None);
        };
        let file = self
            .root
            .open_active_journal()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "active.wal is absent"))?;
        validate_active_snapshot(&file, first.segment_id, scan.valid_len)?;
        Ok(Some(ActiveSnapshot {
            file: Arc::new(file),
            active: Arc::clone(&scan.active),
            part_count: scan.active.len(),
            valid_len: scan.valid_len,
            segment_id: first.segment_id,
        }))
    }

    /// Read the bytes of one active part from the journal.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::ActivePartTooLarge`] if the cached part reference
    /// exceeds the active part cap, or [`StoreError::Io`] if the journal file
    /// cannot be opened or the part bytes cannot be read.
    pub fn read_active_part(&self, p: &ActivePart) -> Result<Vec<u8>, StoreError> {
        let part_len = u64::try_from(p.part.len).unwrap_or(u64::MAX);
        if part_len > MAX_PART_LEN {
            return Err(StoreError::ActivePartTooLarge {
                len: p.part.len,
                max: MAX_PART_LEN,
            });
        }
        let file = self
            .root
            .open_active_journal()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "active.wal is absent"))?;
        validate_active_part_reference(&file, p)?;
        let mut buf = vec![0_u8; p.part.len];
        file.read_exact_at(&mut buf, p.part.offset as u64)?;
        Ok(buf)
    }
}

impl ActiveSnapshot {
    /// Identity of the logical segment captured from the journal header.
    #[must_use]
    pub const fn segment_id(&self) -> kronika_layout::SegmentId {
        self.segment_id
    }

    /// Valid journal parts in captured order.
    #[must_use]
    pub fn parts(&self) -> &[ActivePart] {
        &self.active[..self.part_count]
    }

    /// Return the same journal generation truncated at an earlier committed
    /// frame boundary.
    ///
    /// This lets a paged active reader keep the exact prefix named by its
    /// cursor even after later frames have been appended.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OutOfBounds`] when `position` is later than this
    /// capture or does not end at the journal header or a complete frame.
    pub fn at_position(&self, position: u64) -> Result<Self, StoreError> {
        if position > self.valid_len {
            return Err(StoreError::OutOfBounds);
        }
        let header_end = JOURNAL_HEADER_LEN as u64;
        let mut boundary = header_end;
        let mut count = 0_usize;
        for part in self.parts() {
            let offset =
                u64::try_from(part.part.offset).map_err(|_overflow| StoreError::OutOfBounds)?;
            let len = u64::try_from(part.part.len).map_err(|_overflow| StoreError::OutOfBounds)?;
            let end = offset.checked_add(len).ok_or(StoreError::OutOfBounds)?;
            if end > position {
                break;
            }
            boundary = end;
            count = count.saturating_add(1);
        }
        if position != boundary {
            return Err(StoreError::OutOfBounds);
        }
        Ok(Self {
            file: Arc::clone(&self.file),
            active: Arc::clone(&self.active),
            part_count: count,
            valid_len: position,
            segment_id: self.segment_id,
        })
    }

    /// Read a byte range relative to one captured part.
    ///
    /// The range is bounded by both the part and the captured journal prefix.
    /// Later appends are ignored. A reset or generation change makes the read
    /// stale instead of mixing generations.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::OutOfBounds`] for a range outside the captured
    /// part, or an I/O error when the journal generation changed.
    pub fn read_part_range(
        &self,
        part_index: usize,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, StoreError> {
        let part = self
            .parts()
            .get(part_index)
            .ok_or(StoreError::OutOfBounds)?;
        if part.segment_id != self.segment_id {
            return Err(StoreError::OutOfBounds);
        }
        let part_len = u64::try_from(part.part.len).unwrap_or(u64::MAX);
        if part_len > MAX_PART_LEN {
            return Err(StoreError::ActivePartTooLarge {
                len: part.part.len,
                max: MAX_PART_LEN,
            });
        }
        let relative_end = offset.checked_add(len).ok_or(StoreError::OutOfBounds)?;
        if relative_end > part_len {
            return Err(StoreError::OutOfBounds);
        }
        let part_offset = u64::try_from(part.part.offset).map_err(|_overflow| {
            StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "active part offset does not fit u64",
            ))
        })?;
        let absolute_offset = part_offset
            .checked_add(offset)
            .ok_or(StoreError::OutOfBounds)?;
        let absolute_end = absolute_offset
            .checked_add(len)
            .ok_or(StoreError::OutOfBounds)?;
        if absolute_end > self.valid_len {
            return Err(StoreError::OutOfBounds);
        }
        let buffer_len = usize::try_from(len).map_err(|_overflow| {
            StoreError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "active range length does not fit usize",
            ))
        })?;
        validate_active_snapshot(self.file.as_ref(), self.segment_id, self.valid_len)?;
        let mut buffer = vec![0_u8; buffer_len];
        self.file.read_exact_at(&mut buffer, absolute_offset)?;
        validate_active_snapshot(self.file.as_ref(), self.segment_id, self.valid_len)?;
        Ok(buffer)
    }
}
