//! Property tests for dictionary placement.
//!
//! The generators use small limits so random byte vectors hit normal strings,
//! blobs, and truncated blobs without large test inputs.

use kronika_format::{DictLimits, Resolved, SegmentDicts, StrId};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

// Dev-dependency of other test targets; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use crc as _;
use tempfile as _;
use xxhash_rust as _;

const BLOB_THRESHOLD: usize = 8;
const TRUNCATE_LIMIT: usize = 16;

fn small_limits() -> DictLimits {
    DictLimits::new(BLOB_THRESHOLD, TRUNCATE_LIMIT).expect("8 <= 16")
}

/// How a value is requested from the dictionaries.
///
/// `HotHard` is kept off values that would land in blobs. That combination is
/// a typed error and has separate unit tests.
#[derive(Debug, Clone, Copy)]
enum Op {
    Plain,
    Blob,
    HotSoft,
    HotHard,
}

/// A conflict-free batch of interning calls.
///
/// A value may appear more than once with compatible requirements. This keeps
/// requirement merging in the generated cases while excluding placement
/// conflicts that are covered by unit tests. Shuffling such a batch must not
/// change the final dictionaries.
fn conflict_free_batch() -> impl Strategy<Value = Vec<(Vec<u8>, Op)>> {
    let value_with_ops = proptest::collection::vec(any::<u8>(), 0..32).prop_flat_map(|bytes| {
        let blob_family = proptest::collection::vec(
            prop_oneof![Just(Op::Plain), Just(Op::Blob), Just(Op::HotSoft)],
            1..4,
        );
        let ops = if bytes.len() < BLOB_THRESHOLD {
            let hard_family = proptest::collection::vec(
                prop_oneof![Just(Op::Plain), Just(Op::HotHard), Just(Op::HotSoft)],
                1..4,
            );
            prop_oneof![blob_family, hard_family].boxed()
        } else {
            blob_family.boxed()
        };
        ops.prop_map(move |ops| (bytes.clone(), ops))
    });
    proptest::collection::vec(value_with_ops, 0..12).prop_map(|values| {
        // The same byte value generated twice could pick Blob in one copy
        // and HotHard in the other — a legitimate conflict this strategy
        // promises not to produce. Keep the first occurrence per value.
        let mut batch = Vec::new();
        let mut seen: Vec<Vec<u8>> = Vec::new();
        for (bytes, ops) in values {
            if seen.contains(&bytes) {
                continue;
            }
            seen.push(bytes.clone());
            batch.extend(ops.into_iter().map(|op| (bytes.clone(), op)));
        }
        batch
    })
}

fn apply(dicts: &mut SegmentDicts, bytes: &[u8], op: Op) -> StrId {
    match op {
        Op::Plain => dicts.intern(bytes).expect("conflict-free by construction"),
        Op::Blob => dicts
            .intern_blob(bytes)
            .expect("conflict-free by construction"),
        Op::HotSoft => {
            dicts
                .intern_hot_best_effort(bytes)
                .expect("soft hot never conflicts")
                .0
        }
        Op::HotHard => dicts
            .intern_hot(bytes)
            .expect("strategy keeps hard hot short and unforced"),
    }
}

/// One `dict.strings` row: (id, bytes).
type StringRow = (u64, Vec<u8>);
/// One `dict.blobs` row: (id, stored bytes, full length, truncated).
type BlobRow = (u64, Vec<u8>, u64, bool);

/// Full observable dictionary state for order-independence checks.
fn dump(dicts: &SegmentDicts) -> (Vec<StringRow>, Vec<BlobRow>, Vec<u64>) {
    let strings = dicts
        .strings()
        .map(|(id, bytes)| (id.get(), bytes.to_vec()))
        .collect();
    let blobs = dicts
        .blobs()
        .map(|entry| {
            (
                entry.str_id.get(),
                entry.stored_bytes.to_vec(),
                entry.full_len,
                entry.truncated,
            )
        })
        .collect();
    let hot = dicts.hot_strings().map(|(id, _)| id.get()).collect();
    (strings, blobs, hot)
}

proptest! {
    /// Every issued id resolves back to the value it was issued for.
    /// For truncated blobs "the same value" means the identity triple:
    /// prefix, full length, and SHA-256 of the original.
    #[test]
    fn every_issued_id_resolves(batch in conflict_free_batch()) {
        let mut dicts = SegmentDicts::new(small_limits());
        for (bytes, op) in &batch {
            let id = apply(&mut dicts, bytes, *op);
            prop_assert_eq!(Some(id), StrId::of(bytes), "id is the xxh3 of the full value");

            match dicts.resolve(id) {
                Some(Resolved::Str(stored)) => prop_assert_eq!(stored, bytes.as_slice()),
                Some(Resolved::Blob(entry)) => {
                    prop_assert_eq!(entry.full_len, bytes.len() as u64);
                    if entry.truncated {
                        prop_assert_eq!(entry.stored_bytes, &bytes[..TRUNCATE_LIMIT]);
                        let sha: [u8; 32] = Sha256::digest(bytes).into();
                        prop_assert_eq!(entry.full_sha256, Some(sha));
                    } else {
                        prop_assert_eq!(entry.stored_bytes, bytes.as_slice());
                        prop_assert_eq!(entry.full_sha256, None);
                    }
                }
                None => prop_assert!(false, "issued id must resolve"),
            }
        }
    }

    /// The dictionary contract: ids are unique across strings and blobs,
    /// hot is a subset of strings, sizes route by the threshold.
    #[test]
    fn dictionaries_stay_disjoint_and_hot_is_a_subset(batch in conflict_free_batch()) {
        let mut dicts = SegmentDicts::new(small_limits());
        let mut forced_blobs = Vec::new();
        for (bytes, op) in &batch {
            let id = apply(&mut dicts, bytes, *op);
            if matches!(op, Op::Blob) {
                forced_blobs.push(id.get());
            }
        }

        let string_ids: Vec<u64> = dicts.strings().map(|(id, _)| id.get()).collect();
        let blob_ids: Vec<u64> = dicts.blobs().map(|entry| entry.str_id.get()).collect();
        for id in &blob_ids {
            prop_assert!(!string_ids.contains(id), "id in both dictionaries");
        }
        for (id, bytes) in dicts.strings() {
            prop_assert!(bytes.len() < BLOB_THRESHOLD, "oversized value in strings");
            prop_assert!(!forced_blobs.contains(&id.get()), "forced blob leaked to strings");
        }
        for (id, _) in dicts.hot_strings() {
            prop_assert!(string_ids.contains(&id.get()), "hot id missing from strings");
        }
        prop_assert_eq!(string_ids.len() + blob_ids.len(), dicts.len());
    }

    /// Re-interning the same batch is idempotent: same ids, no growth.
    #[test]
    fn reinterning_is_idempotent(batch in conflict_free_batch()) {
        let mut dicts = SegmentDicts::new(small_limits());
        let first: Vec<StrId> = batch.iter().map(|(b, op)| apply(&mut dicts, b, *op)).collect();
        let len_after_first = dicts.len();
        let second: Vec<StrId> = batch.iter().map(|(b, op)| apply(&mut dicts, b, *op)).collect();
        prop_assert_eq!(first, second);
        prop_assert_eq!(dicts.len(), len_after_first);
    }

    /// Placement is defined by the requirement set, not by call order:
    /// any permutation of a conflict-free batch yields byte-identical
    /// dictionaries, including the hot subset.
    #[test]
    fn placement_is_order_independent(
        batch in conflict_free_batch(),
        seed in any::<u64>(),
    ) {
        let mut forward = SegmentDicts::new(small_limits());
        for (bytes, op) in &batch {
            apply(&mut forward, bytes, *op);
        }

        // A deterministic shuffle driven by the seed.
        let mut shuffled = batch.clone();
        let mut state = seed;
        for i in (1..shuffled.len()).rev() {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "modulo keeps the index in usize range"
            )]
            let j = (state % (i as u64 + 1)) as usize;
            shuffled.swap(i, j);
        }
        let mut reordered = SegmentDicts::new(small_limits());
        for (bytes, op) in &shuffled {
            apply(&mut reordered, bytes, *op);
        }

        prop_assert_eq!(dump(&forward), dump(&reordered));
    }
}
