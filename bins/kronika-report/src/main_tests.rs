use std::ffi::OsString;

use super::{Arguments, arguments};
use kronika_report::ReportTimeRange;

#[test]
fn options_cannot_be_interpreted_as_file_paths() {
    for values in [
        vec!["--help", "--unknown-option"],
        vec!["--unknown-option", "-h"],
        vec!["-h", "report.html"],
        vec!["incident.zms", "--help"],
        vec![
            "--from",
            "1",
            "--to-exclusive",
            "2",
            "--help",
            "report.html",
        ],
    ] {
        assert_eq!(
            arguments(values.into_iter().map(OsString::from)),
            Err("unknown or misplaced option; use ./ before a path beginning with '-'")
        );
    }
    assert!(
        arguments(["./-input.zms", "./-output.html"].map(OsString::from)).is_ok(),
        "an explicit relative path may start with a dash"
    );
}

#[test]
fn command_line_requires_exactly_two_paths() {
    let parsed = arguments([
        OsString::from("incident.zms"),
        OsString::from("report.html"),
    ])
    .expect("two paths");
    assert_eq!(
        parsed,
        Arguments {
            input: "incident.zms".into(),
            output: "report.html".into(),
            visible_range: None,
        }
    );

    assert_eq!(
        arguments([
            OsString::from("--from"),
            OsString::from("1000000"),
            OsString::from("--to-exclusive"),
            OsString::from("2000000"),
            OsString::from("incident.zms"),
            OsString::from("report.html"),
        ]),
        Ok(Arguments {
            input: "incident.zms".into(),
            output: "report.html".into(),
            visible_range: ReportTimeRange::new(1_000_000, 2_000_000),
        })
    );

    assert_eq!(
        arguments(std::iter::empty()),
        Err("missing standalone ZMS input")
    );
    assert_eq!(
        arguments([OsString::from("incident.zms")]),
        Err("missing HTML output")
    );
    assert_eq!(
        arguments([
            OsString::from("incident.zms"),
            OsString::from("report.html"),
            OsString::from("extra"),
        ]),
        Err("expected one ZMS input and one HTML output")
    );
    assert_eq!(
        arguments([
            OsString::from("--from"),
            OsString::from("2"),
            OsString::from("--to-exclusive"),
            OsString::from("1"),
            OsString::from("incident.zms"),
            OsString::from("report.html"),
        ]),
        Err("report range must use positive JavaScript-safe Unix microseconds")
    );

    assert_eq!(
        arguments([
            OsString::from("--from"),
            OsString::from("9007199254740992"),
            OsString::from("--to-exclusive"),
            OsString::from("9007199254740993"),
            OsString::from("incident.zms"),
            OsString::from("report.html"),
        ]),
        Err("report range must use positive JavaScript-safe Unix microseconds")
    );
}
