//! Property tests for the end catalog.
//!
//! A generated catalog must survive encode/decode. Flipping one byte in the
//! encoded catalog should make decoding fail.
//! Detection relies on CRC32C, so a missed corruption is theoretically
//! possible but unlikely in these generated cases.

use kronika_format::{Catalog, Entry, TAIL_INDEX_LEN, TailIndex};
use proptest::prelude::*;

// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use crc as _;
use sha2 as _;
use tempfile as _;
use xxhash_rust as _;

fn entry_strategy() -> impl Strategy<Value = Entry> {
    (
        any::<u32>(),
        any::<u32>(),
        any::<u64>(),
        any::<u64>(),
        any::<u32>(),
        any::<u32>(),
    )
        .prop_map(|(type_id, flags, offset, len, rows, crc32c)| Entry {
            type_id,
            flags,
            offset,
            len,
            rows,
            crc32c,
        })
}

fn catalog_strategy() -> impl Strategy<Value = Catalog> {
    (
        proptest::collection::vec(entry_strategy(), 0..64),
        any::<i64>(),
        any::<i64>(),
        any::<u32>(),
        any::<u32>(),
    )
        .prop_map(
            |(entries, min_ts, max_ts, format_version, window_count)| Catalog {
                entries,
                min_ts,
                max_ts,
                format_version,
                window_count,
            },
        )
}

/// Decode the way a reader does: tail index first, then the catalog bytes
/// it points to.
fn read_back(encoded: &[u8]) -> Result<Catalog, String> {
    if encoded.len() < TAIL_INDEX_LEN {
        return Err("shorter than the tail index".to_owned());
    }
    let tail_bytes: [u8; TAIL_INDEX_LEN] = encoded[encoded.len() - TAIL_INDEX_LEN..]
        .try_into()
        .map_err(|_infallible| "fixed-size tail".to_owned())?;
    let tail = TailIndex::decode(tail_bytes).map_err(|e| e.to_string())?;
    let catalog_end = encoded.len() - TAIL_INDEX_LEN;
    let catalog_start = catalog_end
        .checked_sub(tail.catalog_len as usize)
        .ok_or_else(|| "catalog_len exceeds the buffer".to_owned())?;
    Catalog::decode(&encoded[catalog_start..catalog_end]).map_err(|e| e.to_string())
}

proptest! {
    #[test]
    fn roundtrip(catalog in catalog_strategy()) {
        let encoded = catalog.encode();
        prop_assert_eq!(read_back(&encoded), Ok(catalog));
    }

    #[test]
    fn single_byte_corruption_is_detected(
        catalog in catalog_strategy(),
        position in any::<proptest::sample::Index>(),
        xor in 1..=u8::MAX,
    ) {
        let mut encoded = catalog.encode();
        let at = position.index(encoded.len());
        encoded[at] ^= xor;
        prop_assert!(
            read_back(&encoded).is_err(),
            "corruption at byte {} (xor {:#04x}) went unnoticed", at, xor
        );
    }
}
