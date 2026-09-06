use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kronika_layout::SegmentId;
use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};
use kronika_reader::FinishedReader;
use kronika_store::EmbeddedSource;

use super::{
    HtmlReportInput, ReportTimeRange, isolated_index, write_html, write_html_from_file,
    write_html_from_file_with_segment_id,
};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const VISIBLE_TO: i64 = SEGMENT_ID + 1_000_001;
const ZMS: &[u8] = include_bytes!("../tests/fixtures/standalone.zms");
const IDX: &[u8] = include_bytes!("../tests/fixtures/standalone.idx");

#[test]
fn report_range_requires_positive_javascript_safe_microseconds() {
    const MAX_SAFE: i64 = 9_007_199_254_740_991;

    assert_eq!(
        ReportTimeRange::new(1, MAX_SAFE),
        Some(ReportTimeRange {
            from: 1,
            to_exclusive: MAX_SAFE,
        })
    );
    for (from, to_exclusive) in [
        (0, 1),
        (-1, 1),
        (1, 1),
        (2, 1),
        (1, MAX_SAFE + 1),
        (MAX_SAFE, MAX_SAFE + 1),
    ] {
        assert_eq!(ReportTimeRange::new(from, to_exclusive), None);
    }
}

fn script_blocks(html: &str) -> Option<usize> {
    let mut tail = html;
    let mut count = 0;
    while let Some((_head, body)) = tail.split_once("<script>") {
        let (_script, after) = body.split_once("</script>")?;
        count += 1;
        tail = after;
    }
    Some(count)
}

#[test]
fn isolated_builder_produces_the_committed_canonical_index() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("fixture segment id");
    let source = EmbeddedSource::from_owned(segment_id, ZMS.to_vec(), ZMS.len() as u64)
        .expect("embedded fixture");
    let reader = FinishedReader::new(source);
    let listing = reader.resources().expect("fixture resources");
    let (index, configured_sources) =
        isolated_index(&reader, &listing.resources[0]).expect("build isolated index");
    assert_eq!(index, IDX);
    assert_eq!(configured_sources, SOURCE_OS | SOURCE_POSTGRESQL);
    assert_eq!(super::configured_sources([]), SOURCE_OS);
    assert_eq!(
        super::configured_sources([1_001_001]),
        SOURCE_OS | SOURCE_POSTGRESQL
    );
}

#[test]
fn raw_postgresql_sections_enable_postgresql_without_a_health_block() {
    assert_eq!(
        super::configured_sources([1_020_001, 3_001_001]),
        SOURCE_OS | SOURCE_POSTGRESQL
    );
}

#[test]
fn file_and_vec_writers_produce_identical_html() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    std::fs::write(&path, ZMS).expect("write fixture ZMS");

    let mut from_vec = Vec::new();
    write_html(
        HtmlReportInput {
            segment_id: SegmentId::new(SEGMENT_ID).expect("fixture segment id"),
            zms: ZMS.to_vec(),
            max_zms_bytes: ZMS.len() as u64,
            visible_range: ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO)
                .expect("fixture report range"),
        },
        &mut from_vec,
    )
    .expect("write report from Vec");

    let mut from_file = Vec::new();
    let summary = write_html_from_file(
        std::fs::File::open(path).expect("open fixture ZMS"),
        ZMS.len() as u64,
        &mut from_file,
    )
    .expect("write report from file");

    assert_eq!(summary.segment_id.get(), SEGMENT_ID);
    assert_eq!(from_file, from_vec);
}

#[test]
fn file_writer_preserves_an_explicit_identity_distinct_from_the_first_row() {
    let segment_id = SegmentId::new(SEGMENT_ID + 1_000_000).expect("explicit segment id");
    let mut from_vec = Vec::new();
    write_html(
        HtmlReportInput {
            segment_id,
            zms: ZMS.to_vec(),
            max_zms_bytes: ZMS.len() as u64,
            visible_range: ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO)
                .expect("fixture report range"),
        },
        &mut from_vec,
    )
    .expect("write report from Vec");

    let mut file = tempfile::tempfile().expect("temporary ZMS");
    std::io::Write::write_all(&mut file, ZMS).expect("write fixture ZMS");
    let mut from_file = Vec::new();
    let summary = write_html_from_file_with_segment_id(
        segment_id,
        file,
        ZMS.len() as u64,
        ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO).expect("fixture report range"),
        &mut from_file,
    )
    .expect("write report from file with explicit identity");

    assert_eq!(summary.segment_id, segment_id);
    assert_eq!(from_file, from_vec);
}

#[test]
fn file_writer_applies_the_size_limit_before_output() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    std::fs::write(&path, ZMS).expect("write fixture ZMS");
    let mut output = Vec::new();

    let error = write_html_from_file(
        std::fs::File::open(path).expect("open fixture ZMS"),
        ZMS.len() as u64 - 1,
        &mut output,
    )
    .expect_err("file exceeds limit");

    assert!(matches!(
        error,
        super::HtmlReportError::Resource(kronika_store::ResourceError::TooLarge {
            len,
            max
        }) if len == ZMS.len() as u64 && max == ZMS.len() as u64 - 1
    ));
    assert!(output.is_empty());
}

#[test]
fn file_writer_validates_section_checksums_before_output() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    let mut damaged = ZMS.to_vec();
    damaged[4] ^= 1;
    std::fs::write(&path, damaged).expect("write damaged fixture ZMS");
    let mut output = Vec::new();

    let error = write_html_from_file(
        std::fs::File::open(path).expect("open fixture ZMS"),
        ZMS.len() as u64,
        &mut output,
    )
    .expect_err("section checksum must fail");

    assert!(matches!(
        error,
        super::HtmlReportError::Resource(kronika_store::ResourceError::SectionChecksum { .. })
    ));
    assert!(output.is_empty());
}

#[test]
fn report_is_self_contained_and_deterministic() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("fixture segment id");
    let mut first = Vec::new();
    let mut second = Vec::new();
    for output in [&mut first, &mut second] {
        write_html(
            HtmlReportInput {
                segment_id,
                zms: ZMS.to_vec(),
                max_zms_bytes: ZMS.len() as u64,
                visible_range: ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO)
                    .expect("fixture report range"),
            },
            output,
        )
        .expect("write report");
    }
    assert_eq!(first, second);
    assert!(first.starts_with(b"<!doctype html>"));
    let html = std::str::from_utf8(&first).expect("report HTML is UTF-8");
    assert_eq!(script_blocks(html), Some(2));
    for external in [
        "src=\"http:",
        "src=\"https:",
        "src=\"//",
        "src='http:",
        "src='https:",
        "src='//",
        "href=\"http:",
        "href=\"https:",
        "href=\"//",
        "href='http:",
        "href='https:",
        "href='//",
    ] {
        assert!(!html.contains(external), "external asset {external}");
    }
    assert!(
        !first
            .windows(super::RUNTIME_MARKER.len())
            .any(|bytes| bytes == super::RUNTIME_MARKER)
    );
    assert!(
        first
            .windows(b"__KRONIKA_REPORT_RUNTIME__".len())
            .any(|bytes| bytes == b"__KRONIKA_REPORT_RUNTIME__")
    );
    assert!(html.contains("m=await WebAssembly.compile(r)"));
    assert!(html.contains("await KronikaReportWasm.initEmbedded(m)"));
    assert!(html.contains(&format!(
        "new KronikaReportWasm.ReportSession(\"{SEGMENT_ID}\""
    )));
    assert!(html.contains(&format!("visibleFrom:\"{SEGMENT_ID}\"")));
    assert!(html.contains(&format!("visibleToExclusive:\"{VISIBLE_TO}\"")));
    assert!(!html.contains("KronikaReportWasm.initSync"));
    assert!(!html.contains("new WebAssembly.Module"));
    for encoded in [STANDARD.encode(ZMS), STANDARD.encode(IDX)] {
        assert!(
            first
                .windows(encoded.len())
                .any(|bytes| bytes == encoded.as_bytes()),
            "embedded artifact"
        );
    }
}
