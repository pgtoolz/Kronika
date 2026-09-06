//! Observable slice CLI behavior at the explicit output boundary.

use std::process::Command;
use {
    arrow_array as _, arrow_select as _, chrono as _, kronika_dump as _, kronika_format as _,
    kronika_index as _, kronika_layout as _, kronika_reader as _, kronika_registry as _,
    kronika_report as _, kronika_store as _, kronika_writer as _, serde_json as _,
};

#[test]
fn slice_refuses_to_replace_an_explicit_output_file() {
    let storage = tempfile::tempdir().expect("storage fixture");
    let output_dir = tempfile::tempdir().expect("output fixture");
    let output = output_dir.path().join("incident.zms");
    std::fs::write(&output, b"keep this").expect("seed output");
    let status = Command::new(env!("CARGO_BIN_EXE_kronika-dump"))
        .args([
            "slice",
            "--from",
            "2023-11-14T22:13:20Z",
            "--to",
            "2023-11-14T22:13:20Z",
            "--out",
        ])
        .arg(&output)
        .env("KRONIKA_STORAGE_DIR", storage.path())
        .output()
        .expect("run slice CLI");
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("output already exists"));
    assert_eq!(
        std::fs::read(output).expect("read preserved output"),
        b"keep this"
    );
}
