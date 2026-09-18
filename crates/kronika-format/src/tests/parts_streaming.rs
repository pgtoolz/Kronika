use super::*;

const TEST_MAX_PARTS: usize = 16;

fn scan_streaming(bytes: &[u8], start_at: u64) -> Result<ScanReport, JournalScanError> {
    scan_journal_streaming_strict_from(&bytes, start_at, JournalLimits::default(), TEST_MAX_PARTS)
}

#[test]
fn journal_v1_header_has_the_initial_magic_and_version() {
    let bytes = JournalHeader::EMPTY.encode();
    assert_eq!(&bytes[..8], b"KRNJNL1\0");
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 1);
    assert_eq!(JournalHeader::decode(bytes), Ok(JournalHeader::EMPTY));
}

#[test]
fn reset_marker_accepts_only_the_two_headers_and_their_prefix_transition() {
    let segment_id = 0x0102_0304_0506_0708;
    let marker = ResetMarker::new(4096, segment_id).unwrap();
    assert_eq!(ResetMarker::decode(marker.encode()), Some(marker));
    let previous = marker.expected_previous_header().unwrap().encode();
    let empty = JournalHeader::EMPTY.encode();
    assert_eq!(
        marker.classify_header_transition(previous),
        Some(ResetHeaderTransition::Previous)
    );
    assert_eq!(
        marker.classify_header_transition(empty),
        Some(ResetHeaderTransition::Empty)
    );
    for split in 1..JOURNAL_HEADER_LEN {
        let mut torn = previous;
        torn[..split].copy_from_slice(&empty[..split]);
        assert!(
            marker.classify_header_transition(torn).is_some(),
            "prefix split {split} must be an admissible reset transition"
        );
    }

    let mut non_prefix = previous;
    non_prefix[20] = empty[20];
    assert_eq!(marker.classify_header_transition(non_prefix), None);
    assert_eq!(
        marker.classify_header_transition([0xA5; JOURNAL_HEADER_LEN]),
        None
    );
}

fn framed(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(
            &FrameHeader {
                part_len: p.len() as u64,
            }
            .encode(),
        );
        out.extend_from_slice(p);
    }
    out
}
fn sample_part() -> Vec<u8> {
    build_part(
        &[],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    )
}
#[test]
fn a_clean_journal_reads_to_its_end() {
    let p = sample_part();
    let buf = framed(&[&p, &p]);
    let report = scan_streaming(&buf, 0).unwrap();
    assert_eq!(report.parts.len(), 2);
    assert_eq!(report.valid_len, buf.len());
}

#[test]
fn catalog_visitor_receives_each_validated_part_once() {
    let p = sample_part();
    let buf = framed(&[&p, &p]);
    let mut catalogs = Vec::new();
    let valid_len = visit_journal_streaming_strict_from(
        &buf.as_slice(),
        0,
        JournalLimits::default(),
        TEST_MAX_PARTS,
        |part, catalog, part_buffer_capacity| {
            assert_eq!(part.len, p.len());
            assert!(part_buffer_capacity >= part.len);
            catalogs.push(catalog);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(valid_len, buf.len());
    assert_eq!(catalogs.len(), 2);
    assert!(catalogs.iter().all(|catalog| catalog.min_ts == 1));
}

#[test]
fn bounded_streaming_scan_stops_before_exceeding_the_part_limit() {
    let part = sample_part();
    let bytes = framed(&[&part, &part]);
    assert!(matches!(
        scan_journal_streaming_strict_from(&bytes.as_slice(), 0, JournalLimits::default(), 1,),
        Err(JournalScanError::PartLimitExceeded { limit: 1 })
    ));
}
#[test]
fn a_frame_header_without_its_body_ends_the_scan() {
    let p = sample_part();
    let mut buf = framed(&[&p]);
    let first_frame_len = buf.len();
    buf.extend_from_slice(&FrameHeader { part_len: 999 }.encode()); // header for absent body
    let report = scan_streaming(&buf, 0).unwrap();
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.valid_len, first_frame_len);
}

#[test]
fn corruption_between_two_valid_frames_ends_the_scan() {
    let p = sample_part();
    let mut buf = framed(&[&p]);
    let first_frame_len = buf.len();
    buf.extend_from_slice(&[0xFF; 8]); // garbage between valid frames
    buf.extend_from_slice(&framed(&[&p]));
    let report = scan_streaming(&buf, 0).unwrap();
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.valid_len, first_frame_len);
}

#[test]
fn streaming_from_valid_len_scans_only_the_tail() {
    // A two-part journal. The first frame ends at `first_len`; scanning from
    // there must find only the second part, with an absolute offset, and not
    // re-report the first.
    let p = sample_part();
    let buf = framed(&[&p, &p]);
    let first_len = FRAME_HEADER_LEN + p.len();

    let report = scan_streaming(&buf, first_len as u64).unwrap();
    assert_eq!(report.parts.len(), 1, "only the tail part is scanned");
    assert_eq!(
        report.parts[0].offset,
        first_len + FRAME_HEADER_LEN,
        "the tail part offset is absolute from the file start"
    );
    assert_eq!(report.parts[0].len, p.len());
    assert_eq!(
        report.valid_len,
        buf.len(),
        "valid_len spans the whole file"
    );
}

#[test]
fn streaming_from_end_of_journal_is_empty() {
    // Starting exactly at the journal length yields no parts and a valid_len
    // pinned to the start offset (nothing new to read).
    let p = sample_part();
    let buf = framed(&[&p, &p]);
    let report = scan_streaming(&buf, buf.len() as u64).unwrap();
    assert!(report.parts.is_empty(), "no parts past the end");
    assert_eq!(
        report.valid_len,
        buf.len(),
        "valid_len stays at the start offset when nothing follows"
    );
}

#[test]
fn streaming_rejects_a_start_offset_beyond_the_source() {
    let p = sample_part();
    let buf = framed(&[&p]);
    let JournalScanError::Io(err) = scan_streaming(&buf, buf.len() as u64 + 1)
        .expect_err("start_at beyond the source must be rejected")
    else {
        panic!("invalid start offset must be reported as an I/O validation error");
    };
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}
