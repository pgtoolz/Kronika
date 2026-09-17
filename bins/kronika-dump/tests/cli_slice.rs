//! Observable slice CLI behavior at the explicit output boundary.

use std::process::Command;
use {
    chrono as _, clap as _, kronika_format as _, kronika_index as _, kronika_layout as _,
    kronika_reader as _, kronika_registry as _, kronika_slice as _, kronika_store as _,
    kronika_writer as _, serde_json as _,
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

#[test]
fn explicit_storage_directory_overrides_the_environment() {
    let storage = tempfile::tempdir().expect("storage fixture");
    let output_dir = tempfile::tempdir().expect("output fixture");
    let output = output_dir.path().join("incident.zms");
    std::fs::write(&output, b"keep this").expect("seed output");
    let result = Command::new(env!("CARGO_BIN_EXE_kronika-dump"))
        .args(["slice", "--storage-dir"])
        .arg(storage.path())
        .args([
            "--from",
            "2023-11-14T22:13:20Z",
            "--to",
            "2023-11-14T22:13:20Z",
            "--out",
        ])
        .arg(&output)
        .env("KRONIKA_STORAGE_DIR", output_dir.path().join("missing"))
        .output()
        .expect("run slice CLI");
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("output already exists"));
    assert_eq!(
        std::fs::read(output).expect("preserved output"),
        b"keep this"
    );
}

#[test]
fn help_and_invalid_arguments_do_not_create_files() {
    let directory = tempfile::tempdir().expect("empty fixture");
    for (arguments, success) in [
        (vec!["--help"], true),
        (vec!["--version"], true),
        (vec!["slice", "--help"], true),
        (
            vec![
                "slice",
                "--storage-dir",
                "missing",
                "--from",
                "invalid",
                "--out",
                "new.zms",
            ],
            false,
        ),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_kronika-dump"))
            .args(&arguments)
            .current_dir(directory.path())
            .env_remove("KRONIKA_STORAGE_DIR")
            .output()
            .expect("run CLI without a recording");
        assert_eq!(result.status.success(), success, "arguments: {arguments:?}");
        if success {
            assert!(result.stderr.is_empty(), "help belongs on stdout");
            assert!(!result.stdout.is_empty(), "help must be printed");
        } else {
            assert_eq!(result.status.code(), Some(1));
            assert!(result.stdout.is_empty(), "errors belong on stderr");
        }
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("list fixture")
                .count(),
            0
        );
    }
}
