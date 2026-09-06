//! Per-type row buffers before a journal part is written.

use std::any::Any;
use std::collections::BTreeMap;

use kronika_format::{PartMeta, SectionInput, build_part};
use kronika_registry::{CodecError, SECTION_WRITE_BATCH_ROWS, Section};

/// Buffered rows for one section type.
trait TypeBuffer: Any {
    fn section_type_id(&self) -> u32;
    fn is_empty(&self) -> bool;
    /// Encode the buffered rows to a section body, its row count, and ts range.
    fn encode(&self) -> Result<EncodedRows, CodecError>;
    fn clear(&mut self);
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// One encoded section: the Parquet body plus the catalog fields derived from it.
struct EncodedRows {
    body: Vec<u8>,
    rows: u32,
    ts_range: Option<(i64, i64)>,
    list_i32_child_value_count: usize,
}

/// Encoded bytes and row count for one section in a flushed journal part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionFlushSummary {
    /// Section `type_id`.
    pub type_id: u32,
    /// Rows encoded into this section.
    pub rows: u32,
    /// Encoded section body bytes.
    pub body_bytes: usize,
    /// Child values across all `ListI32` columns in this section.
    pub list_i32_child_value_count: usize,
}

/// Accounting for one flushed collection window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushSummary {
    /// One entry per section written into the part.
    pub sections: Vec<SectionFlushSummary>,
    /// Total ZMS part bytes appended to the journal frame body.
    pub part_bytes: usize,
}

/// A flushed journal part plus its section-level accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushedPart {
    /// ZMS part body ready for `Journal::append`.
    pub body: Vec<u8>,
    /// Section and byte counts for logs.
    pub summary: FlushSummary,
}

struct RowBuffer<T: Section> {
    rows: Vec<T>,
}

impl<T: Section + 'static> TypeBuffer for RowBuffer<T> {
    fn section_type_id(&self) -> u32 {
        T::CONTRACT.type_id.get()
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn encode(&self) -> Result<EncodedRows, CodecError> {
        let list_i32_child_value_count = T::list_i32_child_value_count(&self.rows);
        let body = T::encode(&self.rows)?;
        // `encode` already enforced the row cap; the catalog row field is `u32`.
        let rows = u32::try_from(self.rows.len()).unwrap_or(u32::MAX);
        Ok(EncodedRows {
            body,
            rows,
            ts_range: T::ts_range(&self.rows),
            list_i32_child_value_count,
        })
    }

    fn clear(&mut self) {
        self.rows.clear();
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The writer's collection window: typed rows buffered per section type until a
/// flush turns them into one ZMS part.
#[derive(Default)]
pub struct SectionBuffers {
    by_type: BTreeMap<u32, Box<dyn TypeBuffer>>,
}

impl std::fmt::Debug for SectionBuffers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectionBuffers")
            .field("type_ids", &self.by_type.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SectionBuffers {
    /// An empty set of buffers.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            by_type: BTreeMap::new(),
        }
    }

    /// Buffer one row of section type `T`.
    ///
    /// # Errors
    ///
    /// Returns the input row when this type reached the row cap.
    ///
    /// # Panics
    ///
    /// Panics if two `Section` types use the same `type_id`.
    pub fn push<T: Section + 'static>(&mut self, row: T) -> Result<(), T> {
        let type_id = T::CONTRACT.type_id.get();
        let buffer = self
            .by_type
            .entry(type_id)
            .or_insert_with(|| Box::new(RowBuffer::<T> { rows: Vec::new() }));
        let rows = &mut buffer
            .as_any_mut()
            .downcast_mut::<RowBuffer<T>>()
            .expect("a type_id maps to exactly one Section type")
            .rows;
        if rows.len() >= SECTION_WRITE_BATCH_ROWS {
            return Err(row);
        }
        rows.push(row);
        Ok(())
    }

    /// Whether no rows are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_type.values().all(|buffer| buffer.is_empty())
    }

    /// Encode buffered rows and dictionary sections into one ZMS part.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] when section encoding or part assembly fails.
    pub fn flush(
        &mut self,
        dict_sections: &[crate::dict::DictSection],
    ) -> Result<Option<Vec<u8>>, CodecError> {
        Ok(self
            .flush_with_summary(dict_sections)?
            .map(|flushed| flushed.body))
    }

    /// Encode buffered rows and dictionary sections into one ZMS part.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] when section encoding or part assembly fails.
    pub fn flush_with_summary(
        &mut self,
        dict_sections: &[crate::dict::DictSection],
    ) -> Result<Option<FlushedPart>, CodecError> {
        let encoded: Vec<(u32, EncodedRows)> = self
            .by_type
            .values()
            .filter(|buffer| !buffer.is_empty())
            .map(|buffer| Ok((buffer.section_type_id(), buffer.encode()?)))
            .collect::<Result<_, CodecError>>()?;
        if encoded.is_empty() && dict_sections.is_empty() {
            return Ok(None);
        }

        // Dictionary-only parts use an empty interval; `write_segment` ignores it while
        // folding the segment range.
        let lo = encoded
            .iter()
            .filter_map(|(_, section)| section.ts_range.map(|(lo, _)| lo))
            .min();
        let hi = encoded
            .iter()
            .filter_map(|(_, section)| section.ts_range.map(|(_, hi)| hi))
            .max();
        let (min_ts, max_ts) = match (lo, hi) {
            (Some(lo), Some(hi)) => (lo, hi),
            _ => (i64::MAX, i64::MIN),
        };

        let mut sections: Vec<SectionInput<'_>> = encoded
            .iter()
            .map(|(type_id, section)| SectionInput {
                type_id: *type_id,
                rows: section.rows,
                body: &section.body,
            })
            .collect();
        sections.extend(dict_sections.iter().map(|dict| SectionInput {
            type_id: dict.type_id,
            rows: dict.rows,
            body: &dict.body,
        }));
        let part = build_part(&sections, PartMeta { min_ts, max_ts });
        let mut summary_sections = Vec::with_capacity(sections.len());
        for (type_id, section) in &encoded {
            summary_sections.push(SectionFlushSummary {
                type_id: *type_id,
                rows: section.rows,
                body_bytes: section.body.len(),
                list_i32_child_value_count: section.list_i32_child_value_count,
            });
        }
        for section in dict_sections {
            summary_sections.push(SectionFlushSummary {
                type_id: section.type_id,
                rows: section.rows,
                body_bytes: section.body.len(),
                list_i32_child_value_count: 0,
            });
        }
        let summary = FlushSummary {
            sections: summary_sections,
            part_bytes: part.len(),
        };

        for buffer in self.by_type.values_mut() {
            buffer.clear();
        }
        Ok(Some(FlushedPart {
            body: part,
            summary,
        }))
    }
}

#[cfg(test)]
mod tests;
