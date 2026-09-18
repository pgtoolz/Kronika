//! Observable report CLI behavior over one production-written fixture.

use std::process::Command;
use {clap as _, kronika_report as _, kronika_store as _};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const ZMS: &[u8] = include_bytes!("../../../crates/kronika-report/tests/fixtures/standalone.zms");

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

#[test]
fn cli_accepts_bounds_after_paths_in_either_order() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("incident.zms");
    let output = directory.path().join("incident.html");
    std::fs::write(&input, ZMS).expect("write input ZMS");

    let run = Command::new(env!("CARGO_BIN_EXE_kronika-report"))
        .arg(&input)
        .arg(&output)
        .args(["--to-exclusive=1709164801000001", "--from=1709164800000000"])
        .output()
        .expect("run report CLI");

    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.stdout.is_empty());
    let html = std::fs::read_to_string(output).expect("read generated HTML");
    assert!(html.contains("visibleFrom:\"1709164800000000\""));
    assert!(html.contains("visibleToExclusive:\"1709164801000001\""));
}
