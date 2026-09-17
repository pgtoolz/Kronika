//! Projected row and batch traversal over finished bodies and captured parts.

#[cfg(feature = "posix")]
use super::active_body;
use super::{
    ReaderError, RecordBatch, Row, Segment, Source, VerifiedSection, contract, entry, finished_body,
};
use kronika_registry::{visit_batches as visit_registry_batches, visit_rows};

#[derive(Debug, Clone, Copy)]
struct BodyVisit {
    expected_rows: u64,
    offset: usize,
    limit: usize,
    base: u64,
}

impl Segment {
    /// Decode every section of `type_id` into column-addressable rows.
    ///
    /// Current-segment sections are concatenated in journal order.
    ///
    /// # Errors
    ///
    /// Returns an error when a body fails its checksum or the codec rejects it.
    pub fn rows(&self, type_id: u32) -> Result<Vec<Row>, ReaderError> {
        let Some(contract) = contract(type_id) else {
            return Err(ReaderError::Section {
                type_id,
                source: kronika_registry::CodecError::UnknownType { type_id },
            });
        };
        let columns: Vec<&str> = contract.columns.iter().map(|column| column.name).collect();
        let mut rows = Vec::new();
        self.visit_rows(type_id, &columns, 0, usize::MAX, |_ordinal, row| {
            rows.push(row);
            true
        })?;
        Ok(rows)
    }

    /// Visit a projected physical row range in stable ascending order.
    ///
    /// Current-segment ordinals span all journal parts carrying `type_id`.
    /// Catalog row counts skip earlier parts without opening their bodies, and
    /// returning `false` from `visitor` stops decoding immediately.
    ///
    /// # Errors
    ///
    /// Returns an error when a selected body fails its checksum or projection
    /// decode.
    pub fn visit_rows(
        &self,
        type_id: u32,
        columns: &[&str],
        offset: u64,
        limit: usize,
        visitor: impl FnMut(u64, Row) -> bool,
    ) -> Result<usize, ReaderError> {
        self.visit_section(type_id, offset, limit, visitor, |body, request, visitor| {
            visit_rows(
                type_id,
                body,
                columns,
                Some(request.expected_rows),
                request.offset,
                request.limit,
                |ordinal, row| visitor(request.base.saturating_add(ordinal), row),
            )
            .map_err(|source| ReaderError::Section { type_id, source })
        })
    }

    /// Visit a physical row range as Arrow record batches.
    ///
    /// Current-segment ordinals span all journal parts carrying `type_id`.
    /// `columns` selects exact contract column names; `None` retains the full
    /// schema. Returning `false` from `visitor` stops after the current batch.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown type, or when a selected body fails its
    /// checksum, catalog row-count check, schema check, or Parquet decode.
    pub fn visit_batches(
        &self,
        type_id: u32,
        columns: Option<&[&str]>,
        offset: u64,
        limit: usize,
        visitor: impl FnMut(u64, RecordBatch) -> bool,
    ) -> Result<usize, ReaderError> {
        if contract(type_id).is_none() {
            return Err(ReaderError::Section {
                type_id,
                source: kronika_registry::CodecError::UnknownType { type_id },
            });
        }
        self.visit_section(type_id, offset, limit, visitor, |body, request, visitor| {
            visit_registry_batches(
                type_id,
                body,
                columns,
                Some(request.expected_rows),
                request.offset,
                request.limit,
                |ordinal, batch| visitor(request.base.saturating_add(ordinal), batch),
            )
            .map_err(|source| ReaderError::Section { type_id, source })
        })
    }

    fn visit_section<T>(
        &self,
        type_id: u32,
        offset: u64,
        limit: usize,
        mut visitor: impl FnMut(u64, T) -> bool,
        mut decode: impl FnMut(
            VerifiedSection,
            BodyVisit,
            &mut dyn FnMut(u64, T) -> bool,
        ) -> Result<usize, ReaderError>,
    ) -> Result<usize, ReaderError> {
        let mut visited = 0_usize;
        match &self.source {
            Source::Finished { bytes, catalog } => {
                if let Some(entry) = entry(catalog, type_id) {
                    let rows = u64::from(entry.rows);
                    if offset < rows {
                        let local_offset = usize::try_from(offset).map_err(|_overflow| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "row offset does not fit usize",
                            )
                        })?;
                        let count = decode(
                            finished_body(bytes, entry)?,
                            BodyVisit {
                                expected_rows: u64::from(entry.rows),
                                offset: local_offset,
                                limit,
                                base: 0,
                            },
                            &mut visitor,
                        )?;
                        visited = visited.saturating_add(count);
                    }
                }
            }
            #[cfg(feature = "posix")]
            Source::Active(snapshot) => {
                let mut global_base = 0_u64;
                let mut remaining_offset = offset;
                for (part_index, part) in snapshot.parts().iter().enumerate() {
                    let Some(entry) = entry(&part.catalog, type_id) else {
                        continue;
                    };
                    if visited >= limit {
                        break;
                    }
                    if remaining_offset >= u64::from(entry.rows) {
                        remaining_offset -= u64::from(entry.rows);
                        global_base = global_base.saturating_add(u64::from(entry.rows));
                        continue;
                    }
                    let local_offset = usize::try_from(remaining_offset).map_err(|_overflow| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "row offset does not fit usize",
                        )
                    })?;
                    let mut keep_going = true;
                    let count = decode(
                        active_body(snapshot, part_index, entry)?,
                        BodyVisit {
                            expected_rows: u64::from(entry.rows),
                            offset: local_offset,
                            limit: limit.saturating_sub(visited),
                            base: global_base,
                        },
                        &mut |ordinal, row| {
                            keep_going = visitor(ordinal, row);
                            keep_going
                        },
                    )?;
                    visited = visited.saturating_add(count);
                    remaining_offset = 0;
                    global_base = global_base.saturating_add(u64::from(entry.rows));
                    if !keep_going {
                        break;
                    }
                }
            }
        }
        Ok(visited)
    }
}
