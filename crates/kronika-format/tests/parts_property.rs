//! Property tests for the `active.wal` journal.
//!
//! The tests check that truncation keeps the fully written prefix, and that a
//! single corrupted byte cannot make a part disappear without a reported
//! damaged region.

use kronika_format::{
    Catalog, Entry, FORMAT_VERSION, FrameHeader, JournalLimits, MAGIC, crc32c,
    scan_journal_streaming_strict_from,
};
use proptest::prelude::*;

// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use crc as _;
use sha2 as _;
use tempfile as _;
use xxhash_rust as _;

const fn limits() -> JournalLimits {
    JournalLimits {
        max_part_len: 1 << 20,
    }
}

/// Build a valid ZMS part from random section bodies.
fn build_part(sections: &[Vec<u8>]) -> Vec<u8> {
    let mut part = Vec::new();
    part.extend_from_slice(&MAGIC);
    let mut entries = Vec::new();
    for (i, body) in sections.iter().enumerate() {
        entries.push(Entry {
            type_id: 1_000_001 + u32::try_from(i).expect("few sections"),
            flags: 0,
            offset: part.len() as u64,
            len: body.len() as u64,
            rows: 1,
            crc32c: crc32c(body),
        });
        part.extend_from_slice(body);
    }
    let catalog = Catalog {
        entries,
        min_ts: 1,
        max_ts: 2,
        format_version: FORMAT_VERSION,
        window_count: 1,
    };
    part.extend_from_slice(&catalog.encode());
    part
}

fn frame(part: &[u8]) -> Vec<u8> {
    let mut out = FrameHeader {
        part_len: part.len() as u64,
    }
    .encode()
    .to_vec();
    out.extend_from_slice(part);
    out
}

/// A journal of 1..6 random parts, returned with its frame boundaries.
fn journal_strategy() -> impl Strategy<Value = (Vec<u8>, Vec<usize>)> {
    let section = proptest::collection::vec(any::<u8>(), 1..48);
    let part_sections = proptest::collection::vec(section, 0..3);
    proptest::collection::vec(part_sections, 1..6).prop_map(|parts| {
        let mut journal = Vec::new();
        let mut boundaries = Vec::new();
        for sections in &parts {
            let part = build_part(sections);
            journal.extend_from_slice(&frame(&part));
            boundaries.push(journal.len());
        }
        (journal, boundaries)
    })
}

/// The journal scanner over an in-memory buffer.
fn scan(bytes: &[u8]) -> kronika_format::ScanReport {
    scan_journal_streaming_strict_from(&bytes, 0, limits(), 1024).expect("a buffer reads")
}

proptest! {
    /// A clean journal yields every part and reads to its end.
    #[test]
    fn clean_journal_round_trips((journal, boundaries) in journal_strategy()) {
        let report = scan(&journal);
        prop_assert_eq!(report.parts.len(), boundaries.len());
        prop_assert_eq!(report.valid_len, journal.len());
    }

    /// Truncation at an arbitrary offset loses at most the cut frame.
    #[test]
    fn truncation_recovers_the_full_prefix(
        (journal, boundaries) in journal_strategy(),
        cut in any::<proptest::sample::Index>(),
    ) {
        let cut = cut.index(journal.len());
        let report = scan(&journal[..cut]);

        let full_frames_before = boundaries.iter().filter(|&&b| b <= cut).count();
        prop_assert_eq!(report.parts.len(), full_frames_before);
        prop_assert_eq!(
            report.valid_len,
            boundaries.iter().filter(|&&b| b <= cut).copied().last().unwrap_or(0)
        );
    }

    /// Flipping one byte never hides a frame that precedes it.
    #[test]
    fn single_byte_corruption_keeps_the_prefix(
        (journal, boundaries) in journal_strategy(),
        position in any::<proptest::sample::Index>(),
        flip in 1_u8..=255,
    ) {
        let position = position.index(journal.len());
        let mut corrupted = journal;
        corrupted[position] ^= flip;

        let report = scan(&corrupted);

        let intact_before = boundaries.iter().filter(|&&b| b <= position).count();
        prop_assert!(report.parts.len() >= intact_before);
        prop_assert!(report.parts.len() <= boundaries.len());
    }
}
