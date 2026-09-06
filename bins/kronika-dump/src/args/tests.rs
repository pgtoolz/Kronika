use super::{Command, Want, parse};
use std::path::PathBuf;

fn args(line: &[&str]) -> Result<super::InspectArgs, String> {
    match parse(line.iter().map(|word| (*word).to_owned()))? {
        Command::Inspect(args) => Ok(args),
        Command::Slice(_) => Err("expected inspection command".to_owned()),
    }
}

#[test]
fn a_directory_alone_asks_for_sizes() {
    let parsed = args(&["/data"]).expect("parse");
    assert_eq!(parsed.root, PathBuf::from("/data"));
    assert_eq!(parsed.want, Want::Sizes);
    assert!(!parsed.json);
    assert_eq!(parsed.limit, 20);
}

#[test]
fn a_section_carries_its_type_id() {
    assert_eq!(
        args(&["/data", "--section", "1107001"])
            .expect("parse")
            .want,
        Want::Section(1_107_001)
    );
}

#[test]
fn flags_may_come_before_the_directory() {
    let parsed = args(&["--json", "--index", "/data"]).expect("parse");
    assert_eq!(parsed.root, PathBuf::from("/data"));
    assert_eq!(parsed.want, Want::Index);
    assert!(parsed.json);
}

#[test]
fn conflicting_selectors_are_refused_in_either_order() {
    assert!(args(&["/data", "--index", "--section", "5"]).is_err());
    assert!(args(&["/data", "--section", "5", "--index"]).is_err());
}

#[test]
fn a_limit_of_zero_means_every_row() {
    assert_eq!(
        args(&["/data", "--section", "5", "--limit", "0"])
            .expect("parse")
            .limit,
        0
    );
}

#[test]
fn a_limit_without_a_section_is_refused() {
    assert!(args(&["/data", "--limit", "1"]).is_err());
    assert!(args(&["/data", "--index", "--limit", "1"]).is_err());
}

#[test]
fn a_missing_directory_is_refused() {
    assert!(args(&[]).is_err());
    assert!(args(&["--json"]).is_err());
}

#[test]
fn a_flag_without_its_value_is_refused() {
    assert!(args(&["/data", "--section"]).is_err());
    assert!(args(&["/data", "--limit"]).is_err());
}

#[test]
fn a_value_that_is_not_a_number_is_refused_by_name() {
    let error = args(&["/data", "--section", "psi"]).expect_err("refused");
    assert!(
        error.contains("psi"),
        "the message names the value: {error}"
    );
}

#[test]
fn an_unknown_flag_is_refused_rather_than_ignored() {
    assert!(args(&["/data", "--colour"]).is_err());
}

#[test]
fn a_second_directory_is_refused_rather_than_silently_dropped() {
    assert!(args(&["/one", "/two"]).is_err());
}

#[test]
fn repeated_inspection_bounds_keep_the_last_value() {
    let parsed = args(&[
        "/data", "--from", "1", "--from", "2", "--to", "3", "--to", "4",
    ])
    .expect("parse repeated inspection bounds");
    assert_eq!(parsed.from, Some(2));
    assert_eq!(parsed.to, Some(4));
}

#[test]
fn slice_accepts_exact_leading_subcommand_and_equal_seconds() {
    let parsed = parse(
        [
            "slice",
            "--from",
            "2026-09-02T13:30:00Z",
            "--to",
            "2026-09-02T13:30:00+00:00",
            "--out",
            "/scratch/incident.zms",
        ]
        .into_iter()
        .map(str::to_owned),
    )
    .expect("parse slice");
    assert!(matches!(parsed, Command::Slice(_)));
    let Command::Slice(parsed) = parsed else {
        return;
    };
    assert_eq!(parsed.from.unix_seconds(), 1_788_355_800);
    assert_eq!(parsed.from, parsed.to);
    assert_eq!(parsed.out, PathBuf::from("/scratch/incident.zms"));
}

#[test]
fn slice_accepts_lowercase_rfc3339_separators() {
    let parsed = parse(
        [
            "slice",
            "--from",
            "2026-09-02t13:30:00z",
            "--to",
            "2026-09-02T13:30:00Z",
            "--out",
            "incident.zms",
        ]
        .into_iter()
        .map(str::to_owned),
    )
    .expect("parse lowercase RFC3339 separators");
    let Command::Slice(parsed) = parsed else {
        panic!("expected slice command");
    };
    assert_eq!(parsed.from, parsed.to);
}

#[test]
fn slice_rejects_fractional_or_non_rfc3339_bounds() {
    for timestamp in [
        "2026-09-02T13:30:00.1Z",
        "1788355800",
        "2026-09-02 13:30:00Z",
    ] {
        let result = parse(
            [
                "slice",
                "--from",
                timestamp,
                "--to",
                "2026-09-02T13:30:00Z",
                "--out",
                "x.zms",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        assert!(result.is_err(), "{timestamp} must be rejected");
    }
}

#[test]
fn slice_requires_each_named_flag_exactly_once() {
    for line in [
        vec!["slice", "--to", "2026-09-02T13:30:00Z", "--out", "x.zms"],
        vec!["slice", "--from", "2026-09-02T13:30:00Z", "--out", "x.zms"],
        vec![
            "slice",
            "--from",
            "2026-09-02T13:30:00Z",
            "--to",
            "2026-09-02T13:30:00Z",
        ],
        vec![
            "slice",
            "--from",
            "2026-09-02T13:30:00Z",
            "--from",
            "2026-09-02T13:30:00Z",
            "--to",
            "2026-09-02T13:30:00Z",
            "--out",
            "x.zms",
        ],
    ] {
        assert!(parse(line.into_iter().map(str::to_owned)).is_err());
    }
}

#[test]
fn slice_rejects_reversed_bounds_and_inspection_flags() {
    assert!(
        parse(
            [
                "slice",
                "--from",
                "2026-09-02T13:30:01Z",
                "--to",
                "2026-09-02T13:30:00Z",
                "--out",
                "x.zms"
            ]
            .into_iter()
            .map(str::to_owned)
        )
        .is_err()
    );
    assert!(
        parse(
            [
                "slice",
                "--from",
                "2026-09-02T13:30:00Z",
                "--to",
                "2026-09-02T13:30:00Z",
                "--out",
                "x.zms",
                "--json"
            ]
            .into_iter()
            .map(str::to_owned)
        )
        .is_err()
    );
    assert!(
        parse(
            [
                "slice",
                "--from",
                "2026-09-02T13:30:00Z",
                "--to",
                "2026-09-02T13:30:00Z",
                "--out",
                "incident.html",
            ]
            .into_iter()
            .map(str::to_owned)
        )
        .is_err()
    );
}

#[test]
fn short_options_are_not_storage_paths() {
    for values in [["-h", "--json"], ["-x", "--index"]] {
        assert!(
            parse(values.into_iter().map(str::to_owned))
                .is_err_and(|error| error.starts_with("unknown flag")),
            "short options must fail before opening storage"
        );
    }
    assert!(
        parse(std::iter::once("./-recording".to_owned())).is_ok(),
        "an explicit relative storage path may start with a dash"
    );
}
