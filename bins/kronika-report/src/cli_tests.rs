use std::error::Error as _;
use std::path::{Path, PathBuf};

use kronika_report::HtmlReportError;
use kronika_report::ReportTimeRange;

use super::{GenerateError, TEMP_PREFIX, generate};

const ZMS: &[u8] = include_bytes!("../tests/fixtures/standalone.zms");

fn fixture(directory: &Path) -> PathBuf {
    let input = directory.join("incident.zms");
    std::fs::write(&input, ZMS).expect("write standalone fixture");
    input
}

fn temporary_names(directory: &Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .expect("read temporary directory")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(TEMP_PREFIX))
        .collect()
}

#[test]
fn arbitrary_input_name_is_deterministic_and_replaces_output() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = fixture(directory.path());
    let first = directory.path().join("first.html");
    let second = directory.path().join("second.html");
    std::fs::write(&first, b"old report").expect("write existing report");
    generate(&input, &first, None).expect("first report");
    generate(&input, &second, None).expect("second report");

    let first = std::fs::read(first).expect("read first report");
    let second = std::fs::read(second).expect("read second report");
    assert_eq!(first, second);
    assert!(first.starts_with(b"<!doctype html>"));
    assert!(temporary_names(directory.path()).is_empty());
}

#[test]
fn explicit_time_range_is_embedded_for_report_navigation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = fixture(directory.path());
    let output = directory.path().join("bounded.html");
    let range = ReportTimeRange::new(1_709_164_800_000_000, 1_709_164_801_000_001)
        .expect("fixture report range");

    generate(&input, &output, Some(range)).expect("bounded report");

    let html = std::fs::read_to_string(output).expect("read bounded report");
    assert!(html.contains("visibleFrom:\"1709164800000000\""));
    assert!(html.contains("visibleToExclusive:\"1709164801000001\""));
}

#[test]
fn failure_keeps_the_existing_output_and_removes_the_temporary() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("incident.zms");
    let output = directory.path().join("report.html");
    let mut damaged = ZMS.to_vec();
    damaged[4] ^= 1;
    std::fs::write(&input, damaged).expect("write damaged input");
    std::fs::write(&output, b"existing report").expect("write existing output");

    let error = generate(&input, &output, None).expect_err("damaged ZMS");
    assert!(matches!(
        error,
        GenerateError::Document(HtmlReportError::Resource(
            kronika_store::ResourceError::SectionChecksum { .. }
        ))
    ));
    assert!(error.source().is_some());
    assert_eq!(
        std::fs::read(&output).expect("read existing output"),
        b"existing report"
    );
    assert!(temporary_names(directory.path()).is_empty());
}

#[test]
fn output_must_have_the_html_extension() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = fixture(directory.path());
    let output = directory.path().join("report.htm");
    assert!(matches!(
        generate(&input, &output, None),
        Err(GenerateError::InvalidOutputName(path)) if path == output
    ));
    assert!(!output.exists());
}
