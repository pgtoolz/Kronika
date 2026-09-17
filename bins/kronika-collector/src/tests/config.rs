use super::{validate_journal_max_bytes, validate_segment_max_bytes};
use kronika_format::{JOURNAL_HEADER_LEN, MAX_JOURNAL_LEN};

fn selected_dsn(canonical: Option<&str>, legacy: Option<&str>) -> anyhow::Result<Option<String>> {
    super::parse_pg_dsn(
        canonical.map(std::ffi::OsStr::new),
        legacy.map(std::ffi::OsStr::new),
    )
}

#[test]
fn canonical_postgres_dsn_is_one_connection_string_including_semicolons() {
    for dsn in [
        "host=db.example user=monitor password='private;password' dbname=postgres",
        "postgresql://monitor:private;password@db.example/postgres",
    ] {
        assert_eq!(
            selected_dsn(Some(&format!(" {dsn} ")), None).expect("one complete DSN"),
            Some(dsn.to_owned())
        );
    }
    assert_eq!(selected_dsn(None, None).expect("OS-only default"), None);
}

#[test]
fn legacy_postgres_dsn_uses_only_first_and_does_not_validate_ignored_tail() {
    let first = "host=db.example user=monitor";
    for tail in [
        "",
        ";",
        ";;",
        "; host='unterminated password=RAW_SECRET",
        ";host=other;;",
    ] {
        assert_eq!(
            selected_dsn(None, Some(&format!(" {first} {tail}"))).expect("selected first DSN"),
            Some(first.to_owned())
        );
    }
    for blank in ["", " \t\n "] {
        assert_eq!(
            selected_dsn(None, Some(blank)).expect("legacy blank is absent"),
            None
        );
    }
    for invalid in [";host=db.example", " ; ", "host='unterminated;host=valid"] {
        assert!(
            selected_dsn(None, Some(invalid)).is_err(),
            "reject invalid first DSN"
        );
    }
}

#[test]
fn postgres_dsn_conflicts_and_invalid_selected_values_never_echo_secrets() {
    for canonical in ["", " ", "host=db.example password=RAW_SECRET"] {
        for legacy in ["", " ", "host=other password=OTHER_SECRET"] {
            assert_eq!(
                selected_dsn(Some(canonical), Some(legacy))
                    .expect_err("conflicting presence")
                    .to_string(),
                "KRONIKA_PG_DSN and KRONIKA_PG_DSNS must not both be set"
            );
        }
    }
    for invalid in [
        "",
        " \n ",
        "host='unterminated password=RAW_SECRET dbname=PRIVATE_DATABASE",
    ] {
        let error = selected_dsn(Some(invalid), None).expect_err("invalid selected DSN");
        let message = format!("{error:#}");
        assert!(message.starts_with("KRONIKA_PG_DSN "));
        assert!(!message.contains("RAW_SECRET"));
        assert!(!message.contains("PRIVATE_DATABASE"));
    }
}

#[cfg(unix)]
#[test]
fn non_unicode_legacy_tail_is_ignored_but_selected_bytes_are_validated() {
    use std::os::unix::ffi::OsStrExt as _;

    let raw = std::ffi::OsStr::from_bytes(b"host=db.example;\xffRAW_SECRET");
    assert_eq!(
        super::parse_pg_dsn(None, Some(raw)).expect("ignored tail"),
        Some("host=db.example".to_owned())
    );
    let error = super::parse_pg_dsn(Some(raw), None).expect_err("canonical is one DSN");
    assert!(!format!("{error:#}").contains("RAW_SECRET"));
}

#[test]
fn the_journal_cap_must_fit_the_format() {
    assert!(validate_journal_max_bytes(JOURNAL_HEADER_LEN as u64).is_ok());
    assert!(validate_journal_max_bytes(MAX_JOURNAL_LEN as u64).is_ok());
    assert!(validate_journal_max_bytes(JOURNAL_HEADER_LEN as u64 - 1).is_err());
    assert!(validate_journal_max_bytes(MAX_JOURNAL_LEN as u64 + 1).is_err());
}

#[test]
fn the_segment_cap_must_be_positive() {
    assert!(validate_segment_max_bytes(1).is_ok());
    assert!(validate_segment_max_bytes(0).is_err());
}

#[test]
fn an_empty_element_in_a_list_is_a_refusal_naming_the_variable() {
    let error = super::parse_env_list("KRONIKA_PG_LOGS", "/var/log/a.log;;/var/log/b.log")
        .expect_err("a refusal");

    assert_eq!(error.to_string(), "KRONIKA_PG_LOGS has an empty element");
}

#[test]
fn a_list_is_split_on_semicolons_and_trimmed() {
    let entries = super::parse_env_list("KRONIKA_PG_LOGS", " /var/log/a.log ; /var/log/b.log ")
        .expect("a list");

    assert_eq!(entries, ["/var/log/a.log", "/var/log/b.log"]);
}

#[test]
fn a_blank_list_is_empty_rather_than_one_blank_element() {
    assert!(
        super::parse_env_list("KRONIKA_PGBOUNCER_LOGS", "   ")
            .expect("a list")
            .is_empty()
    );
}

#[test]
fn pg_log_max_lag_is_a_positive_configurable_duration() {
    const CHILD: &str = "KRONIKA_TEST_MAX_LOG_LAG";
    if let Ok(expected) = std::env::var(CHILD) {
        let result = super::Config::from_env();
        if expected == "invalid" {
            assert!(
                result
                    .err()
                    .expect("invalid config")
                    .to_string()
                    .contains("--pg-log-max-lag-s")
            );
        } else {
            assert_eq!(
                result
                    .expect("valid config")
                    .pg_log_max_lag_secs
                    .to_string(),
                expected
            );
        }
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    for (value, expected) in [
        (None, "900"),
        (Some("300"), "300"),
        (Some("0"), "invalid"),
        (Some("-1"), "invalid"),
        (Some("abc"), "invalid"),
    ] {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child
            .env_clear()
            .env("KRONIKA_STORAGE_DIR", dir.path())
            .env(CHILD, expected)
            .arg("--exact")
            .arg("config::tests::pg_log_max_lag_is_a_positive_configurable_duration");
        if let Some(value) = value {
            child.env("KRONIKA_PG_LOG_MAX_LAG_S", value);
        }
        let output = child.output().expect("config child");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn postgres_activity_and_statement_intervals_have_independent_bounds() {
    const CHILD: &str = "KRONIKA_TEST_PG_INTERVALS";
    if let Ok(expected) = std::env::var(CHILD) {
        let result = super::Config::from_env();
        if expected == "invalid" {
            let error = result
                .err()
                .expect("reject a statement interval below five minutes");
            assert!(error.to_string().contains("--pg-statements-interval-s"));
        } else {
            let intervals = result.expect("valid intervals").intervals;
            assert_eq!(
                format!(
                    "{}/{}/{}",
                    intervals.pg_activity,
                    intervals.pg_activity_blocked,
                    intervals.pg_statements_and_plans
                ),
                expected
            );
        }
        return;
    }
    let directory = tempfile::tempdir().expect("tempdir");
    for (activity, blocked, statements, expected) in [
        (None, None, None, "10/5/300"),
        (Some("0"), None, Some("300"), "0/5/300"),
        (Some("12"), Some("3"), Some("600"), "12/3/600"),
        (Some("12"), Some("0"), None, "12/0/300"),
        (None, None, Some("299"), "invalid"),
        (None, None, Some("0"), "invalid"),
    ] {
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child
            .env_clear()
            .env("KRONIKA_STORAGE_DIR", directory.path())
            .env(CHILD, expected)
            .arg("--exact")
            .arg(
                "config::tests::postgres_activity_and_statement_intervals_have_independent_bounds",
            );
        if let Some(value) = activity {
            child.env("KRONIKA_PG_ACTIVITY_INTERVAL_S", value);
        }
        if let Some(value) = blocked {
            child.env("KRONIKA_PG_ACTIVITY_BLOCKED_INTERVAL_S", value);
        }
        if let Some(value) = statements {
            child.env("KRONIKA_PG_STATEMENTS_INTERVAL_S", value);
        }
        let output = child.output().expect("config child");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

#[path = "config/cli.rs"]
mod cli;
