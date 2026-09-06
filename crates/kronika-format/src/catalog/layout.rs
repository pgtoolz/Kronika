//! The canonical order and geometry a catalog must have.

use super::{
    Catalog, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, Error, MAGIC, MAX_PHYSICAL_SECTION_BYTES,
    fmt,
};

/// Why a decoded catalog is not the canonical physical section layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogLayoutError {
    type_id: Option<u32>,
    reason: &'static str,
}

impl CatalogLayoutError {
    pub(super) const fn entry(type_id: u32, reason: &'static str) -> Self {
        Self {
            type_id: Some(type_id),
            reason,
        }
    }

    pub(super) const fn container(reason: &'static str) -> Self {
        Self {
            type_id: None,
            reason,
        }
    }
}

impl fmt::Display for CatalogLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(type_id) = self.type_id {
            write!(f, "section {type_id}: {}", self.reason)
        } else {
            f.write_str(self.reason)
        }
    }
}

impl Error for CatalogLayoutError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum SectionOrder {
    Data(u32),
    Strings,
    Blobs,
}

/// Validate the canonical physical section inventory and exact body geometry.
///
/// Data sections must be unique and ordered by ascending `type_id`, followed
/// by at most one strings dictionary and at most one blobs dictionary. Bodies
/// must be contiguous from the opening magic through `catalog_at`, with zero
/// flags and bounded lengths and row counts.
///
/// # Errors
///
/// Returns [`CatalogLayoutError`] for any non-canonical entry or body range.
pub fn validate_catalog_layout(
    catalog: &Catalog,
    catalog_at: u64,
) -> Result<(), CatalogLayoutError> {
    let mut expected_offset = MAGIC.len() as u64;
    let mut previous: Option<SectionOrder> = None;

    for entry in &catalog.entries {
        if entry.flags != 0 {
            return Err(CatalogLayoutError::entry(
                entry.type_id,
                "reserved flags are not zero",
            ));
        }
        if entry.rows == 0 {
            return Err(CatalogLayoutError::entry(
                entry.type_id,
                "populated section has zero rows",
            ));
        }
        if entry.len == 0 {
            return Err(CatalogLayoutError::entry(
                entry.type_id,
                "section body is empty",
            ));
        }
        if entry.len > MAX_PHYSICAL_SECTION_BYTES {
            return Err(CatalogLayoutError::entry(
                entry.type_id,
                "body length is above the physical cap",
            ));
        }
        if entry.offset != expected_offset {
            return Err(CatalogLayoutError::entry(
                entry.type_id,
                "body is not contiguous with the preceding section",
            ));
        }

        let order = match entry.type_id {
            DICT_STRINGS_TYPE_ID => SectionOrder::Strings,
            DICT_BLOBS_TYPE_ID => SectionOrder::Blobs,
            type_id => SectionOrder::Data(type_id),
        };
        if let Some(previous_order) = previous {
            if order == previous_order {
                return Err(CatalogLayoutError::entry(
                    entry.type_id,
                    "section type occurs more than once",
                ));
            }
            if order < previous_order {
                return Err(CatalogLayoutError::entry(
                    entry.type_id,
                    "section is out of canonical order",
                ));
            }
        }
        previous = Some(order);
        expected_offset = entry.offset.checked_add(entry.len).ok_or_else(|| {
            CatalogLayoutError::entry(entry.type_id, "body range overflows the container")
        })?;
    }

    if expected_offset != catalog_at {
        return Err(CatalogLayoutError::container(
            "section bodies do not end at the catalog start",
        ));
    }
    Ok(())
}
