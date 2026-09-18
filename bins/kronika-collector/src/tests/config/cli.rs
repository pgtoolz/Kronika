use std::ffi::OsString;

use clap::error::ErrorKind;

use crate::config::{self, CollectorMode, Config, RetentionConfig};
use crate::logging::LogLevel;

fn isolated(test: &str, env: &[(&str, OsString)], check: impl FnOnce()) {
    const CHILD: &str = "KRONIKA_TEST_CONFIG_CLI";
    if std::env::var_os(CHILD).is_some() {
        check();
        return;
    }
    run_isolated(test, env);
}

fn run_isolated(test: &str, env: &[(&str, OsString)]) {
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
    child.env_clear().env("KRONIKA_TEST_CONFIG_CLI", "1").args([
        "--exact",
        &format!("config::tests::cli::{test}"),
        "--nocapture",
    ]);
    for (key, value) in env {
        child.env(key, value);
    }
    let output = child.output().expect("isolated configuration test");
    assert!(
        output.status.success(),
        "{test}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

fn parse(args: &[&str]) -> Result<Config, clap::Error> {
    config::parse_from(
        ["kronika-collector", "--storage-dir", "/recording"]
            .into_iter()
            .chain(args.iter().copied()),
    )
}

#[test]
fn arguments_override_even_invalid_environment_values() {
    isolated(
        "arguments_override_even_invalid_environment_values",
        &[
            ("KRONIKA_STORAGE_DIR", "/old-recording".into()),
            ("KRONIKA_COLLECTOR_MODE", "invalid-mode".into()),
            ("KRONIKA_INTERVAL_S", "often".into()),
            ("KRONIKA_PG_DSN", "password='RAW_SECRET".into()),
            ("KRONIKA_PG_DSNS", "password='LEGACY_SECRET".into()),
            ("KRONIKA_LOG_LEVEL", "invalid-level".into()),
        ],
        || {
            let config = parse(&[
                "--mode",
                "postgresql",
                "--interval-s",
                "12",
                "--pg-dsn",
                "host=example.invalid user=monitor",
                "--log-level",
                "debug",
            ])
            .expect("CLI replaces env before parsing it");
            assert_eq!(config.storage_dir, std::path::Path::new("/recording"));
            assert_eq!(config.mode, CollectorMode::Postgresql);
            assert_eq!(config.tick_secs, 12);
            assert_eq!(
                config.pg_dsn.as_deref(),
                Some("host=example.invalid user=monitor")
            );
            assert_eq!(config.log_level, LogLevel::Debug);
        },
    );
}

#[test]
fn repeated_arguments_replace_env_lists_without_splitting_values() {
    isolated(
        "repeated_arguments_replace_env_lists_without_splitting_values",
        &[
            ("KRONIKA_PG_LOGS", "invalid;;list".into()),
            ("KRONIKA_PGBOUNCER_DSNS", "invalid;;list".into()),
            ("KRONIKA_PGBOUNCER_LOGS", "invalid;;list".into()),
        ],
        || {
            let dsn = "host=example.invalid password='private;password'";
            let config = parse(&[
                "--pg-dsn",
                dsn,
                "--pg-log",
                "/logs/first;part.csv",
                "--pg-log",
                "/logs/second.csv",
                "--pgbouncer-dsn",
                dsn,
                "--pgbouncer-dsn",
                "host=other.invalid",
                "--pgbouncer-log",
                "/logs/pool;one.log",
                "--pgbouncer-log",
                "/logs/pool-two.log",
            ])
            .expect("atomic CLI arguments");
            assert_eq!(config.pg_dsn.as_deref(), Some(dsn));
            assert_eq!(config.pg_logs, ["/logs/first;part.csv", "/logs/second.csv"]);
            assert_eq!(config.pgbouncer_dsns, [dsn, "host=other.invalid"]);
            assert_eq!(
                config.pgbouncer_logs,
                ["/logs/pool;one.log", "/logs/pool-two.log"]
            );
        },
    );
}

#[test]
fn environment_lists_keep_their_existing_separator_and_whitespace_rules() {
    isolated(
        "environment_lists_keep_their_existing_separator_and_whitespace_rules",
        &[
            ("KRONIKA_PG_LOGS", " /logs/a.csv ; /logs/b.csv ".into()),
            (
                "KRONIKA_PGBOUNCER_DSNS",
                " host=one.invalid ; host=two.invalid ".into(),
            ),
            ("KRONIKA_PGBOUNCER_LOGS", " \t ".into()),
        ],
        || {
            let config = parse(&[]).expect("legacy lists");
            assert_eq!(config.pg_logs, ["/logs/a.csv", "/logs/b.csv"]);
            assert_eq!(
                config.pgbouncer_dsns,
                ["host=one.invalid", "host=two.invalid"]
            );
            assert!(config.pgbouncer_logs.is_empty());
        },
    );
}

#[test]
fn cli_defaults_preserve_storage_and_sampling_behavior() {
    isolated(
        "cli_defaults_preserve_storage_and_sampling_behavior",
        &[],
        || {
            let config = parse(&[]).expect("OS-only collector");
            assert_eq!(config.mode, CollectorMode::Local);
            assert_eq!(config.tick_secs, 5);
            assert_eq!(config.pg_dsn, None);
            assert_eq!(config.postgres_effective_cpus, None);
            assert_eq!(config.segment_max_bytes, 64 * 1024 * 1024);
            assert_eq!(
                config.journal_max_bytes,
                kronika_format::MAX_JOURNAL_LEN as u64
            );
            assert_eq!(config.segment_max_age_secs, 900);
            assert_eq!(
                config.retention,
                Some(RetentionConfig::Fixed(2 * 1024 * 1024 * 1024))
            );
            assert_eq!(config.intervals.pg_activity, 10);
            assert_eq!(config.intervals.pg_activity_blocked, 5);
            assert_eq!(config.intervals.pg_instance, 30);
            assert_eq!(config.intervals.pg_tables_and_indexes, 300);
            assert_eq!(config.intervals.pg_statements_and_plans, 300);
            assert_eq!(config.proc_root, None);
            assert_eq!(config.sys_root, std::path::Path::new("/sys"));
        },
    );
}

// These values fit the journal bounds as well as segment and retention sizes.
const STORAGE_SIZE_CASES: &[(&str, u64)] = &[
    ("4096", 4096),
    ("4096.25", 4096),
    ("4096.75", 4097),
    ("4kb", 4000),
    ("4kB", 4000),
    ("4KB", 4000),
    ("1.5Mb", 1_500_000),
    ("1.5mB", 1_500_000),
    ("1.5M", 1_500_000),
    ("1.5m", 1_500_000),
    (" 1.5 KiB ", 1536),
    ("1.5MiB", 1_572_864),
    ("1.5mIb", 1_572_864),
    ("0.5GiB", 536_870_912),
    ("1G", 1_000_000_000),
];

#[test]
fn storage_sizes_accept_decimal_and_binary_units() {
    isolated("storage_sizes_accept_decimal_and_binary_units", &[], || {
        for &(raw, expected) in STORAGE_SIZE_CASES {
            let config = parse(&["--segment-max-bytes", "1", "--retention", raw]).expect(raw);
            assert_eq!(
                config.retention,
                Some(RetentionConfig::Fixed(expected)),
                "--retention {raw}"
            );

            let config = parse(&[
                "--segment-max-bytes",
                raw,
                "--journal-max-bytes",
                raw,
                "--retention",
                "auto",
            ])
            .expect(raw);
            assert_eq!(
                config.segment_max_bytes, expected,
                "--segment-max-bytes {raw}"
            );
            assert_eq!(
                config.journal_max_bytes, expected,
                "--journal-max-bytes {raw}"
            );
        }
        for (raw, expected) in [
            ("10GiB", 10 * 1024 * 1024 * 1024),
            ("10G", 10_000_000_000),
            ("10GB", 10_000_000_000),
            ("10gb", 10_000_000_000),
            (" 1.5 GiB ", 1_610_612_736),
            ("10737418240", 10_737_418_240),
            ("18446744073709551615", u64::MAX),
        ] {
            let config = parse(&["--retention", raw]).expect(raw);
            assert_eq!(
                config.retention,
                Some(RetentionConfig::Fixed(expected)),
                "{raw}"
            );
        }
        let config = parse(&[
            "--segment-max-bytes",
            "0.5GiB",
            "--journal-max-bytes",
            "1G",
            "--retention",
            "2GB",
        ])
        .expect("all storage limits accept units");
        assert_eq!(config.segment_max_bytes, 536_870_912);
        assert_eq!(config.journal_max_bytes, 1_000_000_000);
        assert_eq!(
            config.retention,
            Some(RetentionConfig::Fixed(2_000_000_000))
        );
    });
}

#[test]
fn storage_size_environment_values_match_cli() {
    const CASE: &str = "KRONIKA_TEST_STORAGE_SIZE_CASE";
    if let Ok(raw) = std::env::var(CASE) {
        let &(_, expected) = STORAGE_SIZE_CASES
            .iter()
            .find(|(candidate, _)| *candidate == raw)
            .expect("known storage size case");
        let config = parse(&["--segment-max-bytes", "1"]).expect("retention size from env");
        assert_eq!(
            config.retention,
            Some(RetentionConfig::Fixed(expected)),
            "{raw}"
        );

        let config = parse(&["--retention", "auto"]).expect("segment and journal sizes from env");
        assert_eq!(config.segment_max_bytes, expected, "segment {raw}");
        assert_eq!(config.journal_max_bytes, expected, "journal {raw}");
        return;
    }
    for &(raw, _) in STORAGE_SIZE_CASES {
        run_isolated(
            "storage_size_environment_values_match_cli",
            &[
                (CASE, raw.into()),
                ("KRONIKA_RETENTION", raw.into()),
                ("KRONIKA_SEGMENT_MAX_BYTES", raw.into()),
                ("KRONIKA_JOURNAL_MAX_BYTES", raw.into()),
            ],
        );
    }
}

#[test]
fn storage_size_environment_values_use_the_same_units_and_cli_precedence() {
    isolated(
        "storage_size_environment_values_use_the_same_units_and_cli_precedence",
        &[
            ("KRONIKA_RETENTION", "10GiB".into()),
            ("KRONIKA_SEGMENT_MAX_BYTES", "32MiB".into()),
            ("KRONIKA_JOURNAL_MAX_BYTES", "512MiB".into()),
        ],
        || {
            let config = parse(&[]).expect("human sizes in env");
            assert_eq!(
                config.retention,
                Some(RetentionConfig::Fixed(10_737_418_240))
            );
            assert_eq!(config.segment_max_bytes, 33_554_432);
            assert_eq!(config.journal_max_bytes, 536_870_912);
            let config = parse(&[
                "--retention",
                "10G",
                "--segment-max-bytes",
                "64M",
                "--journal-max-bytes",
                "1GiB",
            ])
            .expect("CLI overrides env sizes");
            assert_eq!(
                config.retention,
                Some(RetentionConfig::Fixed(10_000_000_000))
            );
            assert_eq!(config.segment_max_bytes, 64_000_000);
            assert_eq!(config.journal_max_bytes, 1_073_741_824);
        },
    );
}

#[test]
fn storage_sizes_reject_invalid_units_overflow_and_out_of_range_budgets() {
    isolated(
        "storage_sizes_reject_invalid_units_overflow_and_out_of_range_budgets",
        &[],
        || {
            for flag in ["--retention", "--segment-max-bytes", "--journal-max-bytes"] {
                for raw in [
                    "10XB",
                    "-1GiB",
                    "NaN",
                    "inf",
                    "18446744073709551616",
                    "18446744073709551615.5",
                    "18446744073709551615.6B",
                    "184467440737095516155e-1",
                    "16EiB",
                ] {
                    let argument = format!("{flag}={raw}");
                    let error = parse(&[&argument]).err().expect("reject invalid size");
                    assert!(error.to_string().contains(flag), "{error}");
                }
            }
            for args in [
                vec!["--segment-max-bytes", "0MiB"],
                vec!["--journal-max-bytes", "2GiB"],
                vec!["--journal-max-bytes", "35B"],
                vec!["--segment-max-bytes", "128MiB", "--retention", "255MiB"],
            ] {
                assert!(parse(&args).is_err(), "reject out-of-range {args:?}");
            }
            let config =
                parse(&["--retention", "128MiB"]).expect("two default segments fit exactly");
            assert_eq!(config.retention, Some(RetentionConfig::Fixed(134_217_728)));
            let config =
                parse(&["--retention", "auto:75"]).expect("automatic retention still works");
            assert_eq!(config.retention, Some(RetentionConfig::Auto(75)));
        },
    );
}

#[test]
fn cli_numbers_are_normalized_and_keep_the_existing_bounds() {
    isolated(
        "cli_numbers_are_normalized_and_keep_the_existing_bounds",
        &[],
        || {
            let config = parse(&[
                "--interval-s",
                " 30 ",
                "--pg-dsn",
                "host=example.invalid",
                "--postgres-effective-cpus",
                " 2 ",
                "--pg-activity-interval-s",
                "0",
                "--pg-statements-interval-s",
                "600",
            ])
            .expect("trim numbers; zero activity reads at the base tick");
            assert_eq!(config.tick_secs, 30);
            assert_eq!(config.postgres_effective_cpus, Some(2));
            assert_eq!(config.intervals.pg_activity, 0);
            assert_eq!(config.intervals.pg_statements_and_plans, 600);
            for args in [
                ["--interval-s", "often"],
                ["--segment-max-age-s", " -1 "],
                ["--segment-max-bytes", "0"],
                ["--journal-max-bytes", "0"],
                ["--postgres-effective-cpus", "0"],
                ["--postgres-effective-cpus", "two"],
                ["--pg-log-max-lag-s", "0"],
                ["--pg-statements-interval-s", "299"],
                ["--pg-statements-interval-s", "0"],
            ] {
                let error = parse(&args).err().expect("reject invalid value");
                assert!(error.to_string().contains(args[0]), "{error}");
            }
        },
    );
}

#[test]
fn modes_and_dependent_options_are_validated() {
    isolated("modes_and_dependent_options_are_validated", &[], || {
        let error = parse(&["--mode", "remote"]).err().expect("unknown mode");
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
        for choice in ["local", "postgresql"] {
            assert!(error.to_string().contains(choice));
        }
        for args in [
            vec!["--retention", "1"],
            vec!["--mode", "postgresql"],
            vec!["--postgres-effective-cpus", "2"],
            vec![
                "--mode",
                "postgresql",
                "--pg-dsn",
                "host=example.invalid",
                "--pgbouncer-log",
                "/pool.log",
            ],
        ] {
            assert!(parse(&args).is_err(), "reject incompatible {args:?}");
        }
        let config = parse(&[
            "--mode",
            " postgresql ",
            "--pg-dsn",
            "host=example.invalid",
            "--log-level",
            " WARNING ",
        ])
        .expect("preserve normalized modes and log-level aliases");
        assert_eq!(config.mode, CollectorMode::Postgresql);
        assert_eq!(config.log_level, LogLevel::Warn);
    });
}

#[test]
fn help_and_version_ignore_invalid_env_without_exposing_secrets() {
    isolated(
        "help_and_version_ignore_invalid_env_without_exposing_secrets",
        &[
            ("KRONIKA_PG_DSN", "password='RAW_SECRET".into()),
            ("KRONIKA_PGBOUNCER_DSNS", "password='POOL_SECRET".into()),
            ("KRONIKA_PG_SSL_ROOT_CERT", "/PRIVATE_CA_PATH".into()),
            ("KRONIKA_COLLECTOR_MODE", "invalid".into()),
            ("KRONIKA_INTERVAL_S", "invalid".into()),
        ],
        || {
            for (flag, kind) in [
                ("--help", ErrorKind::DisplayHelp),
                ("--version", ErrorKind::DisplayVersion),
            ] {
                let error = config::parse_from(["kronika-collector", flag])
                    .err()
                    .expect("display and exit");
                assert_eq!(error.kind(), kind);
                let message = error.to_string();
                for secret in ["RAW_SECRET", "POOL_SECRET", "PRIVATE_CA_PATH"] {
                    assert!(!message.contains(secret));
                }
            }
        },
    );
}

#[test]
fn invalid_dsn_errors_never_include_credentials_or_database_names() {
    isolated(
        "invalid_dsn_errors_never_include_credentials_or_database_names",
        &[],
        || {
            for flag in ["--pg-dsn", "--pgbouncer-dsn"] {
                let error = parse(&[
                    flag,
                    "host='broken password=RAW_SECRET dbname=PRIVATE_DATABASE",
                ])
                .err()
                .expect("invalid DSN");
                let message = format!("{:#}", anyhow::Error::new(error));
                for secret in ["RAW_SECRET", "PRIVATE_DATABASE"] {
                    assert!(!message.contains(secret), "{message}");
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn non_unicode_paths_are_accepted_but_dsn_errors_do_not_echo_bytes() {
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

    isolated(
        "non_unicode_paths_are_accepted_but_dsn_errors_do_not_echo_bytes",
        &[(
            "KRONIKA_PG_DSN",
            OsString::from_vec(b"host=db password=RAW_SECRET\xff".to_vec()),
        )],
        || {
            let error = parse(&[]).err().expect("reject non-Unicode DSN");
            assert!(!format!("{error:#}").contains("RAW_SECRET"));
            let path = OsString::from_vec(b"/recording/\xff".to_vec());
            let config = config::parse_from([
                OsString::from("kronika-collector"),
                "--storage-dir".into(),
                path.clone(),
                "--proc-root".into(),
                path.clone(),
                "--pg-dsn".into(),
                "host=example.invalid".into(),
            ])
            .expect("OS paths do not require Unicode");
            assert_eq!(config.storage_dir.as_os_str().as_bytes(), path.as_bytes());
            assert_eq!(
                config
                    .proc_root
                    .as_deref()
                    .expect("explicit root")
                    .as_os_str()
                    .as_bytes(),
                path.as_bytes()
            );
        },
    );
}

#[test]
fn configuration_is_installed_once_without_affecting_independent_parsing() {
    isolated(
        "configuration_is_installed_once_without_affecting_independent_parsing",
        &[],
        || {
            config::install(parse(&["--interval-s", "17"]).expect("first config"))
                .expect("install once");
            assert_eq!(config::get().tick_secs, 17);
            let second =
                parse(&["--interval-s", "23"]).expect("parsing does not access the global");
            assert!(config::install(second).is_err());
            assert_eq!(config::get().tick_secs, 17);
        },
    );
}
