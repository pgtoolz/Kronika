//! Measure completed segments and aggregate their section catalogs in one scan.
//!
//! Read from each segment's catalog, so a stage that starts collecting a new
//! source shows up in the demo without the demo learning anything about it.

use anyhow::{Context, Result, bail};
use kronika_format::{Catalog, TAIL_INDEX_LEN, TailIndex};
use std::collections::BTreeMap;
use std::path::Path;

/// One section's total across every segment of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectionRows {
    /// The section's type id.
    pub(crate) type_id: u32,
    /// Its registry name, or `unknown` for an id this build does not know.
    pub(crate) name: &'static str,
    /// Rows summed over the run's segments.
    pub(crate) rows: u64,
}

#[derive(Default)]
pub(crate) struct SegmentSummary {
    pub(crate) count: usize,
    pub(crate) bytes: u64,
    pub(crate) sections: Vec<SectionRows>,
}

/// Scan each completed segment once, without following directory symlinks.
pub(crate) fn measure(root: &Path) -> Result<SegmentSummary> {
    let mut summary = SegmentSummary::default();
    let mut totals: BTreeMap<u32, u64> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("read {}", dir.display()))?;
            let path = entry.path();
            let file_type = entry.file_type().context("stat a data-root entry")?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "zms") {
                summary.count += 1;
                summary.bytes += entry.metadata().context("stat a segment")?.len();
                for entry in catalog(&path)?.entries {
                    *totals.entry(entry.type_id).or_default() += u64::from(entry.rows);
                }
            }
        }
    }
    summary.sections = totals
        .into_iter()
        .map(|(type_id, rows)| SectionRows {
            type_id,
            name: kronika_registry::section_name(type_id).unwrap_or("unknown"),
            rows,
        })
        .collect();
    Ok(summary)
}

fn catalog(path: &Path) -> Result<Catalog> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let tail_at = bytes
        .len()
        .checked_sub(TAIL_INDEX_LEN)
        .with_context(|| format!("{} is too short to be a segment", path.display()))?;
    let mut tail = [0_u8; TAIL_INDEX_LEN];
    tail.copy_from_slice(bytes.get(tail_at..).unwrap_or_default());
    let index = match TailIndex::decode(tail) {
        Ok(index) => index,
        Err(error) => bail!("{}: {error:?}", path.display()),
    };
    let catalog_len = usize::try_from(index.catalog_len).context("catalog length exceeds usize")?;
    let catalog_at = tail_at
        .checked_sub(catalog_len)
        .with_context(|| format!("{} claims a catalog longer than itself", path.display()))?;
    match Catalog::decode(bytes.get(catalog_at..tail_at).unwrap_or_default()) {
        Ok(catalog) => Ok(catalog),
        Err(error) => bail!("{}: {error:?}", path.display()),
    }
}

#[cfg(test)]
#[path = "tests/sections.rs"]
mod tests;
