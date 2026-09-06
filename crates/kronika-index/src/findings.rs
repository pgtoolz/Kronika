//! Sparse source locators.

use crate::decode::{i64_at, u16_at, u32_at};
use crate::file::IndexError;

/// Maximum sparse locators stored for one physical section.
pub const MAX_FINDINGS_PER_BLOCK: usize = 4_096;
pub(crate) const PG_LOG_ERRORS_TYPE_ID: u32 = 2_001_001;
pub(crate) const MAX_LOG_ERROR_CATEGORY: u8 = 10;

const HEADER_LEN: usize = 9;
const FINDING_LEN: usize = 15;
const NO_CATEGORY: u8 = u8::MAX;

/// A compact source-row locator kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FindingKind {
    /// An explicit known-bad boundary was crossed.
    KnownBad = 1,
    /// Reserved statistical locator kind retained for file compatibility.
    Spike = 2,
    /// A stored row in an explicit event-stream layout.
    Event = 3,
}

impl FindingKind {
    const fn from_raw(raw: u8) -> Result<Self, IndexError> {
        match raw {
            1 => Ok(Self::KnownBad),
            2 => Ok(Self::Spike),
            3 => Ok(Self::Event),
            _ => Err(IndexError::BadLayout),
        }
    }
}

/// One locator into a physical ZMS section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    /// Source locator kind.
    pub kind: FindingKind,
    /// Zero-based field position in the physical registry contract.
    pub field_ordinal: u16,
    /// Physical row position returned by the production reader.
    pub row_ordinal: u32,
    /// Current snapshot timestamp in unix microseconds.
    pub timestamp: i64,
    /// Stored `pg_log_errors.category` for a locator in that layout only,
    /// regardless of kind.
    pub category: Option<u8>,
}

impl Finding {
    const fn order_key(self) -> (i64, u32, u16, FindingKind) {
        (
            self.timestamp,
            self.row_ordinal,
            self.field_ordinal,
            self.kind,
        )
    }
}

/// Sparse locators for one physical section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingBlock {
    /// Physical source layout.
    pub type_id: u32,
    /// Locators before the fixed storage cap was applied.
    pub total_hits: u32,
    /// Whether qualifying locators were omitted by the fixed cap.
    pub truncated: bool,
    /// Stored locators in timestamp and locator order.
    pub findings: Vec<Finding>,
}

impl FindingBlock {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, IndexError> {
        validate(self)?;
        let count = u32::try_from(self.findings.len()).map_err(|_overflow| IndexError::TooLarge)?;
        let finding_len = finding_len(self.type_id);
        let capacity = HEADER_LEN
            .checked_add(
                self.findings
                    .len()
                    .checked_mul(finding_len)
                    .ok_or(IndexError::TooLarge)?,
            )
            .ok_or(IndexError::TooLarge)?;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&self.total_hits.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.push(u8::from(self.truncated));
        for finding in &self.findings {
            out.push(finding.kind as u8);
            out.extend_from_slice(&finding.field_ordinal.to_le_bytes());
            out.extend_from_slice(&finding.row_ordinal.to_le_bytes());
            out.extend_from_slice(&finding.timestamp.to_le_bytes());
            if self.type_id == PG_LOG_ERRORS_TYPE_ID {
                out.push(finding.category.unwrap_or(NO_CATEGORY));
            }
        }
        Ok(out)
    }

    pub(crate) fn decode(type_id: u32, bytes: &[u8]) -> Result<Self, IndexError> {
        let total_hits = u32_at(bytes, 0)?;
        let count = usize::try_from(u32_at(bytes, 4)?).map_err(|_overflow| IndexError::TooLarge)?;
        let truncated = match bytes.get(8) {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(IndexError::BadLayout),
        };
        let finding_len = finding_len(type_id);
        let expected = HEADER_LEN
            .checked_add(count.checked_mul(finding_len).ok_or(IndexError::TooLarge)?)
            .ok_or(IndexError::TooLarge)?;
        if bytes.len() != expected || count > MAX_FINDINGS_PER_BLOCK {
            return Err(IndexError::BadLayout);
        }
        let mut findings = Vec::with_capacity(count);
        for raw in bytes[HEADER_LEN..].chunks_exact(finding_len) {
            findings.push(Finding {
                kind: FindingKind::from_raw(raw[0])?,
                field_ordinal: u16_at(raw, 1)?,
                row_ordinal: u32_at(raw, 3)?,
                timestamp: i64_at(raw, 7)?,
                category: if type_id == PG_LOG_ERRORS_TYPE_ID {
                    match raw[15] {
                        NO_CATEGORY => None,
                        value => Some(value),
                    }
                } else {
                    None
                },
            });
        }
        let block = Self {
            type_id,
            total_hits,
            truncated,
            findings,
        };
        validate(&block)?;
        Ok(block)
    }
}

const fn finding_len(type_id: u32) -> usize {
    if type_id == PG_LOG_ERRORS_TYPE_ID {
        FINDING_LEN + 1
    } else {
        FINDING_LEN
    }
}

fn validate(block: &FindingBlock) -> Result<(), IndexError> {
    if block.findings.len() > MAX_FINDINGS_PER_BLOCK {
        return Err(IndexError::BadLayout);
    }
    let stored = u32::try_from(block.findings.len()).map_err(|_overflow| IndexError::TooLarge)?;
    if block.total_hits < stored || block.truncated != (block.total_hits > stored) {
        return Err(IndexError::BadLayout);
    }
    let mut previous = None;
    for finding in &block.findings {
        if block.type_id == PG_LOG_ERRORS_TYPE_ID {
            if !matches!(finding.category, Some(0..=MAX_LOG_ERROR_CATEGORY)) {
                return Err(IndexError::BadLayout);
            }
        } else if finding.category.is_some() {
            return Err(IndexError::BadLayout);
        }
        if previous.is_some_and(|before| before >= finding.order_key()) {
            return Err(IndexError::BadLayout);
        }
        previous = Some(finding.order_key());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
