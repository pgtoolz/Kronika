//! Public HTML writer boundary over one production-written fixture.

#![cfg(feature = "generator")]

use kronika_layout::SegmentId;
use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};
use kronika_report::{HtmlReportInput, ReportTimeRange, write_html};
use std::process::Command;
use {
    base64 as _, flate2 as _, kronika_format as _, kronika_index as _, kronika_reader as _,
    kronika_store as _, serde_json as _, tempfile as _,
};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const ZMS: &[u8] = include_bytes!("fixtures/standalone.zms");
const IDX: &[u8] = include_bytes!("fixtures/standalone.idx");

#[test]
fn public_writer_builds_one_self_contained_document() {
    let mut html = Vec::new();
    let summary = write_html(
        HtmlReportInput {
            segment_id: SegmentId::new(SEGMENT_ID).expect("fixture segment id"),
            zms: ZMS.to_vec(),
            max_zms_bytes: u64::try_from(ZMS.len()).expect("fixture length fits u64"),
            visible_range: ReportTimeRange::new(SEGMENT_ID, SEGMENT_ID + 1_000_001)
                .expect("fixture report range"),
        },
        &mut html,
    )
    .expect("write report HTML");

    assert!(html.starts_with(b"<!doctype html>"));
    assert_eq!(summary.segment_id.get(), SEGMENT_ID);
    assert_eq!(summary.zms_bytes, ZMS.len() as u64);
    assert_eq!(summary.idx_bytes, IDX.len() as u64);
    assert_eq!(summary.configured_sources, SOURCE_OS | SOURCE_POSTGRESQL);
}

#[test]
fn cli_accepts_an_arbitrary_zms_basename_directly() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("incident.zms");
    let output = directory.path().join("incident.html");
    std::fs::write(&input, ZMS).expect("write input ZMS");

    let run = Command::new(env!("CARGO_BIN_EXE_kronika-report"))
        .arg(&input)
        .arg(&output)
        .output()
        .expect("run report CLI");

    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let html = std::fs::read_to_string(output).expect("read generated HTML");
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains(&format!(
        "new KronikaReportWasm.ReportSession(\"{SEGMENT_ID}\""
    )));
}
