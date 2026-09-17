//! Bounded journal catalog planning and verified positional section reads.

use std::collections::BTreeMap;

use kronika_format::{
    Catalog, Entry, FORMAT_VERSION, MAGIC, META_LEN, PartError, TAIL_INDEX_LEN, TailIndex, crc32c,
    validate_catalog_layout,
};
use kronika_registry::{Bytes, CodecError, MAX_SECTION_ROWS, VerifiedSection};

use super::WriteError;
use crate::{Journal, JournalPartRef};

#[derive(Debug, Clone, Copy)]
pub(super) struct SectionDescriptor {
    pub(super) part: JournalPartRef,
    pub(super) entry: Entry,
}

#[derive(Debug)]
pub(super) struct SegmentPlan {
    pub(super) by_type: BTreeMap<u32, Vec<SectionDescriptor>>,
    pub(super) min_ts: i64,
    pub(super) max_ts: i64,
    pub(super) window_count: u32,
}

pub(super) fn plan_segment(journal: &Journal) -> Result<SegmentPlan, WriteError> {
    let mut by_type = BTreeMap::<u32, Vec<SectionDescriptor>>::new();
    let mut section_count = 0_usize;
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    let window_count =
        u32::try_from(journal.parts().len()).map_err(|_error| WriteError::ArithmeticOverflow {
            what: "window count",
        })?;
    for &part_ref in journal.parts() {
        // Recheck framing immediately before publication. Each body is CRC
        // checked separately just before its type is finalized.
        let catalog = read_part_catalog(journal, part_ref)?;
        if catalog.format_version != FORMAT_VERSION {
            return Err(WriteError::UnsupportedFormat {
                version: catalog.format_version,
            });
        }
        min_ts = min_ts.min(catalog.min_ts);
        max_ts = max_ts.max(catalog.max_ts);
        for entry in catalog.entries {
            section_count = section_count
                .checked_add(1)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "section descriptor count",
                })?;
            if section_count > MAX_SECTION_ROWS {
                return Err(WriteError::TooManySections {
                    sections: section_count,
                    max: MAX_SECTION_ROWS,
                });
            }
            let descriptors = by_type.entry(entry.type_id).or_default();
            descriptors
                .try_reserve(1)
                .map_err(WriteError::CatalogAllocation)?;
            descriptors.push(SectionDescriptor {
                part: part_ref,
                entry,
            });
        }
    }
    if min_ts > max_ts {
        min_ts = 0;
        max_ts = 0;
    }
    Ok(SegmentPlan {
        by_type,
        min_ts,
        max_ts,
        window_count,
    })
}

fn read_part_catalog(journal: &Journal, part: JournalPartRef) -> Result<Catalog, WriteError> {
    let minimum = MAGIC.len() + META_LEN + TAIL_INDEX_LEN;
    if part.len() < minimum {
        return Err(WriteError::Part(PartError::TooShort { actual: part.len() }));
    }
    let magic = journal.read_part_range(part, 0, MAGIC.len())?;
    if magic.as_slice() != MAGIC {
        let mut actual = [0_u8; 4];
        actual.copy_from_slice(&magic);
        return Err(WriteError::Part(PartError::BadMagic { actual }));
    }
    let tail_at = part.len() - TAIL_INDEX_LEN;
    let tail = journal.read_part_range(part, tail_at, TAIL_INDEX_LEN)?;
    let tail: [u8; TAIL_INDEX_LEN] = tail
        .try_into()
        .map_err(|_bytes| WriteError::Part(PartError::TooShort { actual: part.len() }))?;
    let tail = TailIndex::decode(tail).map_err(|error| WriteError::Part(PartError::Tail(error)))?;
    let catalog_len = tail.catalog_len as usize;
    let Some(catalog_at) = tail_at.checked_sub(catalog_len) else {
        return Err(WriteError::Part(PartError::BadCatalogLen {
            catalog_len: tail.catalog_len,
        }));
    };
    if catalog_at < MAGIC.len() {
        return Err(WriteError::Part(PartError::BadCatalogLen {
            catalog_len: tail.catalog_len,
        }));
    }
    let bytes = journal.read_part_range(part, catalog_at, catalog_len)?;
    let catalog =
        Catalog::decode(&bytes).map_err(|error| WriteError::Part(PartError::Catalog(error)))?;
    validate_catalog_layout(&catalog, catalog_at as u64)
        .map_err(|error| WriteError::Part(PartError::Layout(error)))?;
    Ok(catalog)
}

pub(super) fn aggregate_rows(
    type_id: u32,
    descriptors: &[SectionDescriptor],
) -> Result<usize, WriteError> {
    let rows = descriptors.iter().try_fold(0_usize, |rows, descriptor| {
        rows.checked_add(descriptor.entry.rows as usize)
            .ok_or(WriteError::ArithmeticOverflow {
                what: "section row count",
            })
    })?;
    if rows > MAX_SECTION_ROWS {
        return Err(CodecError::TooManyRows {
            rows,
            max: MAX_SECTION_ROWS,
        }
        .into());
    }
    if type_id == 0 {
        return Err(CodecError::UnknownType { type_id }.into());
    }
    Ok(rows)
}

pub(super) fn read_verified_body(
    journal: &Journal,
    descriptor: SectionDescriptor,
) -> Result<VerifiedSection, WriteError> {
    let body = read_section_body(journal, descriptor)?;
    VerifiedSection::verify(Bytes::from(body), descriptor.entry.crc32c, crc32c)
        .map_err(WriteError::Codec)
}

pub(super) fn read_section_body(
    journal: &Journal,
    descriptor: SectionDescriptor,
) -> Result<Vec<u8>, WriteError> {
    let start = usize::try_from(descriptor.entry.offset).map_err(|_overflow| {
        WriteError::ArithmeticOverflow {
            what: "section offset",
        }
    })?;
    let len = usize::try_from(descriptor.entry.len).map_err(|_overflow| {
        WriteError::ArithmeticOverflow {
            what: "section length",
        }
    })?;
    journal
        .read_part_range(descriptor.part, start, len)
        .map_err(WriteError::Journal)
}
