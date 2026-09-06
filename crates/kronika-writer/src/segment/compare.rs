//! Accepting an existing final file only after an exact byte comparison.

use super::*;
use crate::write_test_hook;

pub(super) fn validate_segment(file: &File, expected: WriteSummary) -> Result<bool, io::Error> {
    let length = file.metadata()?.len();
    if length != expected.bytes {
        return Ok(false);
    }
    let minimum = MAGIC
        .len()
        .checked_add(META_LEN)
        .and_then(|value| value.checked_add(TAIL_INDEX_LEN))
        .expect("fixed ZMS lengths fit usize");
    if length < minimum as u64 {
        return Ok(false);
    }

    let mut magic = [0_u8; MAGIC.len()];
    file.read_exact_at(&mut magic, 0)?;
    if magic != MAGIC {
        return Ok(false);
    }

    let tail_at = length - TAIL_INDEX_LEN as u64;
    let mut tail_bytes = [0_u8; TAIL_INDEX_LEN];
    file.read_exact_at(&mut tail_bytes, tail_at)?;
    let Ok(tail) = TailIndex::decode(tail_bytes) else {
        return Ok(false);
    };
    let expected_catalog_len = expected
        .sections
        .checked_mul(ENTRY_LEN)
        .and_then(|value| value.checked_add(META_LEN));
    let Some(expected_catalog_len) = expected_catalog_len else {
        return Ok(false);
    };
    if expected_catalog_len > MAX_CATALOG_BYTES
        || usize::try_from(tail.catalog_len).ok() != Some(expected_catalog_len)
    {
        return Ok(false);
    }
    let catalog_at = match tail_at.checked_sub(u64::from(tail.catalog_len)) {
        Some(offset) if offset >= MAGIC.len() as u64 => offset,
        _ => return Ok(false),
    };
    let mut catalog_bytes = vec![0_u8; expected_catalog_len];
    file.read_exact_at(&mut catalog_bytes, catalog_at)?;
    let Ok(catalog) = Catalog::view(&catalog_bytes) else {
        return Ok(false);
    };
    if catalog.format_version != FORMAT_VERSION
        || usize::try_from(catalog.entry_count).ok() != Some(expected.sections)
        || catalog.min_ts != expected.min_ts
        || catalog.max_ts != expected.max_ts
    {
        return Ok(false);
    }
    Ok(catalog.entries().all(|entry| {
        entry.offset >= MAGIC.len() as u64
            && entry
                .offset
                .checked_add(entry.len)
                .is_some_and(|end| end <= catalog_at)
    }))
}

pub(super) fn files_equal(left: &File, right: &File) -> Result<bool, io::Error> {
    let left_identity = FileIdentity::from_file(left)?;
    let right_identity = FileIdentity::from_file(right)?;
    let length = left_identity.len;
    if right_identity.len != length {
        return Ok(false);
    }
    let mut left_buffer = vec![0_u8; COMPARE_BUFFER_BYTES].into_boxed_slice();
    let mut right_buffer = vec![0_u8; COMPARE_BUFFER_BYTES].into_boxed_slice();
    let mut offset = 0_u64;
    while offset < length {
        let remaining = usize::try_from((length - offset).min(COMPARE_BUFFER_BYTES as u64))
            .map_err(|_overflow| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "comparison chunk does not fit the address space",
                )
            })?;
        left.read_exact_at(&mut left_buffer[..remaining], offset)?;
        right.read_exact_at(&mut right_buffer[..remaining], offset)?;
        if left_buffer[..remaining] != right_buffer[..remaining] {
            return Ok(false);
        }
        write_test_hook!(AfterFirstComparisonChunk);
        offset = offset
            .checked_add(remaining as u64)
            .expect("comparison offset is bounded by file length");
    }
    Ok(FileIdentity::from_file(left)? == left_identity
        && FileIdentity::from_file(right)? == right_identity)
}

#[cfg(test)]
pub(super) struct ComparisonHookGuard;

#[cfg(test)]
impl ComparisonHookGuard {
    pub(super) fn assert_consumed(self) {
        AFTER_FIRST_COMPARISON_CHUNK.with(|hook| {
            assert!(hook.borrow().is_none(), "comparison hook was not exercised");
        });
        drop(self);
    }
}

#[cfg(test)]
impl Drop for ComparisonHookGuard {
    fn drop(&mut self) {
        AFTER_FIRST_COMPARISON_CHUNK.with(|hook| {
            hook.borrow_mut().take();
        });
    }
}

#[cfg(test)]
pub(super) fn arm_after_first_comparison_chunk(
    hook: impl FnOnce() + 'static,
) -> ComparisonHookGuard {
    AFTER_FIRST_COMPARISON_CHUNK.with(|armed| {
        assert!(armed.borrow_mut().replace(Box::new(hook)).is_none());
    });
    ComparisonHookGuard
}

#[cfg(test)]
pub(super) fn run_after_first_comparison_chunk() {
    let hook = AFTER_FIRST_COMPARISON_CHUNK.with(|armed| armed.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}
