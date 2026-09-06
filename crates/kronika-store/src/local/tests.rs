mod budget;
mod catalog;
mod journal;
mod segment;

use kronika_format::{
    Catalog, ENTRY_LEN, FORMAT_VERSION, FRAME_HEADER_LEN, JournalHeader, JournalState, MAGIC,
    MAX_JOURNAL_LEN, META_LEN, PartRef, ResetMarker, TAIL_INDEX_LEN,
};
use kronika_layout::{LimitKind, SegmentArtifacts, SegmentId};
use std::fs;

pub(super) use super::budget::{
    accounted_scan_metadata_bytes, active_metadata_bytes, ensure_active_part_budget,
    reserve_active_slots,
};
pub(super) use super::segment::{classify_zms_validation, read_validated_zms_summary};
pub(super) use super::*;
pub(super) use crate::catalog_summary::CatalogDigest;
pub(super) use crate::source::{
    ActiveJournalWarningReason, InvalidZmsReason, StoreIoOperation, StoreObject, StoreWarning,
    StoreWarningReason,
};
pub(super) use crate::zms::MAX_CATALOG_BYTES;
pub(super) use kronika_format::MAX_JOURNAL_PARTS;
use kronika_format::{FrameHeader, PartMeta, SectionInput, build_part, crc32c};
use kronika_layout::SegmentAddress;

fn part(ts: i64) -> Vec<u8> {
    part_with_body(ts, b"baseline")
}

fn part_with_body(ts: i64, body: &[u8]) -> Vec<u8> {
    build_part(
        &[SectionInput {
            type_id: 1_006_001,
            rows: 1,
            body,
        }],
        PartMeta {
            min_ts: ts,
            max_ts: ts + 1,
        },
    )
}

fn segment_path(root: &Path, raw_id: i64) -> std::path::PathBuf {
    let address = SegmentAddress::new(SegmentId::new(raw_id).expect("representable segment id"))
        .expect("segment address");
    let day = root
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    fs::create_dir_all(&day).expect("create test UTC day");
    day.join(address.zms_name())
}

fn write_segment(root: &Path, raw_id: i64, bytes: impl AsRef<[u8]>) {
    fs::write(segment_path(root, raw_id), bytes).expect("write test segment");
}

fn invalid_warning(scan: &LocalScan, raw_id: i64) -> StoreWarning {
    let address = SegmentAddress::new(SegmentId::new(raw_id).expect("representable segment id"))
        .expect("segment address");
    *scan
        .warnings
        .iter()
        .find(|warning| warning.affected == StoreObject::Segment(address))
        .expect("invalid segment warning")
}

fn frame(part_bytes: &[u8]) -> Vec<u8> {
    let mut out = FrameHeader {
        part_len: part_bytes.len() as u64,
    }
    .encode()
    .to_vec();
    out.extend_from_slice(part_bytes);
    out
}

fn journal(raw_id: i64, parts: &[Vec<u8>]) -> Vec<u8> {
    let body = parts
        .iter()
        .flat_map(|part| frame(part))
        .collect::<Vec<_>>();
    let header = JournalHeader {
        state: JournalState::Active { segment_id: raw_id },
        body_len: body.len() as u64,
    };
    let mut bytes = header.encode().to_vec();
    bytes.extend_from_slice(&body);
    bytes
}

fn write_journal(root: &Path, raw_id: i64, parts: &[Vec<u8>]) {
    fs::write(root.join("active.wal"), journal(raw_id, parts)).expect("write version-1 journal");
}

fn append_journal_part(path: &Path, raw_id: i64, part_bytes: &[u8]) -> Vec<u8> {
    let mut bytes = fs::read(path).expect("read journal");
    bytes.extend_from_slice(&frame(part_bytes));
    let body_len = bytes.len() - JOURNAL_HEADER_LEN;
    bytes[..JOURNAL_HEADER_LEN].copy_from_slice(
        &JournalHeader {
            state: JournalState::Active { segment_id: raw_id },
            body_len: body_len as u64,
        }
        .encode(),
    );
    fs::write(path, &bytes).expect("append journal part");
    bytes
}

fn write_empty_journal(root: &Path) {
    fs::write(root.join("active.wal"), JournalHeader::EMPTY.encode())
        .expect("write empty version-1 journal");
}

fn reset_catalog_summary_reads() {
    CATALOG_SUMMARY_READS.with(|reads| reads.set(0));
}

fn catalog_summary_reads() -> usize {
    CATALOG_SUMMARY_READS.with(std::cell::Cell::get)
}

#[derive(Clone, Copy)]
enum CommittedHeaderPhase {
    Previous,
    Empty,
    Torn,
}

fn committed_reset_journal(raw_id: i64, parts: &[Vec<u8>], phase: CommittedHeaderPhase) -> Vec<u8> {
    let mut bytes = journal(raw_id, parts);
    let previous_len = bytes.len() as u64;
    let previous_header: [u8; JOURNAL_HEADER_LEN] = bytes[..JOURNAL_HEADER_LEN]
        .try_into()
        .expect("complete previous header");
    bytes.extend_from_slice(
        &ResetMarker::new(previous_len, raw_id)
            .expect("non-empty journal reset marker")
            .encode(),
    );
    match phase {
        CommittedHeaderPhase::Previous => {}
        CommittedHeaderPhase::Empty => {
            bytes[..JOURNAL_HEADER_LEN].copy_from_slice(&JournalHeader::EMPTY.encode());
        }
        CommittedHeaderPhase::Torn => {
            let empty = JournalHeader::EMPTY.encode();
            let split = JOURNAL_HEADER_LEN / 2;
            bytes[..split].copy_from_slice(&empty[..split]);
            bytes[split..JOURNAL_HEADER_LEN]
                .copy_from_slice(&previous_header[split..JOURNAL_HEADER_LEN]);
        }
    }
    bytes
}

/// Build a minimal valid part with no sections and a specific `format_version`.
///
/// Layout: `MAGIC(4)` | catalog block (`META_LEN` bytes) | tail index (`TAIL_INDEX_LEN` bytes)
fn minimal_part_with_version(format_version: u32) -> Vec<u8> {
    let catalog = Catalog {
        entries: vec![],
        min_ts: 0,
        max_ts: 0,
        format_version,
        window_count: 0,
    };
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&catalog.encode());
    out
}

/// Locate the tail index within a part buffer.
///
/// Returns the byte offset of the 8-byte tail index at the end of the buffer.
fn tail_offset(buf: &[u8]) -> usize {
    buf.len() - TAIL_INDEX_LEN
}

/// Locate the catalog block start within a part buffer.
///
/// The `catalog_len` stored in the tail index tells us where the catalog starts.
fn catalog_offset(buf: &[u8]) -> usize {
    let tail_at = tail_offset(buf);
    let catalog_len = u32::from_le_bytes(buf[tail_at..tail_at + 4].try_into().unwrap()) as usize;
    tail_at - catalog_len
}

/// Offset of `format_version` within the meta block (20 bytes into meta).
fn format_version_offset(buf: &[u8]) -> usize {
    // meta block is the last META_LEN bytes of the catalog block (before the tail)
    let cat_start = catalog_offset(buf);
    let cat_end = tail_offset(buf);
    let cat_len = cat_end - cat_start;
    let entry_count = (cat_len - META_LEN) / ENTRY_LEN;
    cat_start + entry_count * ENTRY_LEN + 20
}

/// Offset of crc32c within the meta block (24 bytes into meta).
fn meta_crc_offset(buf: &[u8]) -> usize {
    let cat_start = catalog_offset(buf);
    let cat_end = tail_offset(buf);
    let cat_len = cat_end - cat_start;
    let entry_count = (cat_len - META_LEN) / ENTRY_LEN;
    cat_start + entry_count * ENTRY_LEN + 24
}

/// Recompute catalog CRC and patch it into `buf` at the crc field position.
fn repatch_catalog_crc(buf: &mut [u8]) {
    let crc_at = meta_crc_offset(buf);
    let tail_at = tail_offset(buf);
    // Zero the crc field before computing.
    buf[crc_at..crc_at + 4].copy_from_slice(&0_u32.to_le_bytes());
    let crc = crc32c(&buf[catalog_offset(buf)..tail_at]);
    buf[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());
}

// --- read_catalog branch tests ---

// --- scan() behavioral tests ---

// A mock ReadAt that claims a large byte_len but returns UnexpectedEof on
// body reads — simulating a file that was truncated between byte_len() and
// the first read_exact_at call (TOCTOU race with a concurrent write and reset).
struct TruncatedAfterHeader {
    data: Vec<u8>,
    /// Reported size is larger than `data.len()`, causing body reads to fail.
    reported_len: u64,
}

impl ReadAt for TruncatedAfterHeader {
    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.reported_len)
    }
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(buf.len());
        if end > self.data.len() {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
}

// A mock ReadAt that serves each offset once, then returns UnexpectedEof on
// any re-read of the same offset.
struct RejectsRepeatedOffsets {
    data: Vec<u8>,
    seen: std::cell::RefCell<std::collections::HashSet<u64>>,
}

impl ReadAt for RejectsRepeatedOffsets {
    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.data.len() as u64)
    }
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        if !self.seen.borrow_mut().insert(offset) {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(buf.len());
        if end > self.data.len() {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
}
