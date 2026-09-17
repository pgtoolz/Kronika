use clap::error::ErrorKind;
use kronika_report::ReportTimeRange;

use super::{Config, parse_from};

#[test]
fn accepts_paths_and_optional_bounds_in_any_order() {
    let mut expected = Config {
        input: "incident.zms".into(),
        output: "report.html".into(),
        visible_range: None,
    };
    assert_eq!(
        parse_from(["kronika-report", "incident.zms", "report.html"]).expect("two paths"),
        expected
    );
    expected.visible_range = ReportTimeRange::new(1_000_000, 2_000_000);
    for args in [
        vec![
            "--from",
            "1000000",
            "--to-exclusive",
            "2000000",
            "incident.zms",
            "report.html",
        ],
        vec![
            "incident.zms",
            "--to-exclusive",
            "2000000",
            "report.html",
            "--from",
            "1000000",
        ],
        vec![
            "incident.zms",
            "report.html",
            "--to-exclusive=2000000",
            "--from=1000000",
        ],
    ] {
        assert_eq!(
            parse_from(std::iter::once("kronika-report").chain(args)).expect("bounded report"),
            expected
        );
    }
}

#[test]
fn rejects_missing_paths_unpaired_bounds_and_unknown_options() {
    for (args, kind) in [
        (vec![], ErrorKind::MissingRequiredArgument),
        (vec!["incident.zms"], ErrorKind::MissingRequiredArgument),
        (
            vec!["incident.zms", "report.html", "extra"],
            ErrorKind::UnknownArgument,
        ),
        (
            vec!["incident.zms", "report.html", "--unknown"],
            ErrorKind::UnknownArgument,
        ),
        (
            vec!["incident.zms", "report.html", "--from", "1"],
            ErrorKind::MissingRequiredArgument,
        ),
        (
            vec!["incident.zms", "report.html", "--to-exclusive", "2"],
            ErrorKind::MissingRequiredArgument,
        ),
    ] {
        let error = parse_from(std::iter::once("kronika-report").chain(args))
            .expect_err("invalid arguments");
        assert_eq!(error.kind(), kind);
    }
}

#[test]
fn rejects_invalid_visible_intervals_before_file_access() {
    for (from, to) in [
        ("0", "1"),
        ("1", "1"),
        ("2", "1"),
        ("1", "9007199254740992"),
        ("no", "2"),
    ] {
        let error = parse_from([
            "kronika-report",
            "missing.zms",
            "report.html",
            "--from",
            from,
            "--to-exclusive",
            to,
        ])
        .expect_err("invalid bounds");
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }
}

#[test]
fn help_and_version_are_generated_before_required_arguments() {
    for (flag, kind) in [
        ("-h", ErrorKind::DisplayHelp),
        ("--help", ErrorKind::DisplayHelp),
        ("--version", ErrorKind::DisplayVersion),
    ] {
        let error = parse_from(["kronika-report", flag]).expect_err("display request");
        assert_eq!(error.kind(), kind);
        assert_eq!(error.exit_code(), 0);
    }
    let help = parse_from(["kronika-report", "--help"])
        .expect_err("long help")
        .to_string();
    for detail in [
        "--from",
        "--to-exclusive",
        "INPUT.zms",
        "OUTPUT.html",
        "Examples:",
        "TMPDIR",
    ] {
        assert!(help.contains(detail), "missing {detail}");
    }
}

#[test]
fn accepts_non_utf8_paths_and_explicit_dash_paths() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let input = OsString::from_vec(b"incident-\xff.zms".to_vec());
    let parsed = parse_from([
        OsString::from("kronika-report"),
        input.clone(),
        OsString::from("report.html"),
    ])
    .expect("non-UTF-8 input path");
    assert_eq!(parsed.input.as_os_str(), input);
    for args in [
        vec!["kronika-report", "./-input.zms", "./-output.html"],
        vec!["kronika-report", "--", "-input.zms", "-output.html"],
    ] {
        assert!(parse_from(args).is_ok());
    }
}
