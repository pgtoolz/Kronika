use std::ffi::OsString;

use clap::error::ErrorKind;

use super::{Config, account, parse_from, source_set};

#[test]
fn absent_credentials_disable_authentication() {
    assert_eq!(account(None, None).expect("no authentication"), None);
}

#[test]
fn both_nonempty_credentials_enable_authentication() {
    let made = account(Some("dba".to_owned()), Some("secret".to_owned()))
        .expect("valid configuration")
        .expect("account");
    assert_eq!(made.user, "dba");
    assert_eq!(made.password, "secret");
    let debug = format!("{made:?}");
    assert_eq!(debug, "Account { credentials: [redacted] }");
    assert!(!debug.contains("dba"));
    assert!(!debug.contains("secret"));
}

#[test]
fn partial_or_empty_credentials_are_configuration_errors() {
    for (user, password, variable) in [
        (Some("dba"), None, "KRONIKA_WEB_PASSWORD"),
        (None, Some("secret"), "KRONIKA_WEB_USER"),
        (Some(""), Some("secret"), "KRONIKA_WEB_USER"),
        (Some("dba"), Some(""), "KRONIKA_WEB_PASSWORD"),
        (Some(""), Some(""), "KRONIKA_WEB_USER"),
        (Some(""), None, "KRONIKA_WEB_PASSWORD"),
        (None, Some(""), "KRONIKA_WEB_USER"),
    ] {
        let error = account(user.map(str::to_owned), password.map(str::to_owned))
            .expect_err("invalid credentials");
        let message = error.to_string();
        assert!(message.contains(variable), "{message}");
        assert!(!message.contains("secret"), "{message}");
    }
}

#[test]
fn the_source_bitset_accepts_the_four_public_combinations() {
    assert_eq!(source_set("0").expect("no sources"), 0);
    assert_eq!(source_set("1").expect("OS"), 1);
    assert_eq!(source_set("2").expect("PostgreSQL"), 2);
    assert_eq!(source_set("3").expect("all sources"), 3);
    assert!(source_set("postgres").is_err());
    assert!(source_set("4").is_err());
}

#[test]
fn source_names_select_catalog_flags() {
    for (name, expected) in [("none", 0), ("os", 1), ("postgresql", 2), ("all", 3)] {
        assert_eq!(source_set(name).expect(name), expected);
    }
}

// Each child receives its own environment, so parallel tests never mutate the
// process-wide environment while clap reads it.
fn isolated(test: &str, env: &[(&str, OsString)], check: impl FnOnce()) {
    const CHILD: &str = "KRONIKA_TEST_WEB_CONFIG";
    if std::env::var_os(CHILD).is_some() {
        check();
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .env_clear()
        .env(CHILD, "1")
        .envs(env.iter().map(|(key, value)| (key, value)))
        .args(["--exact", &format!("config::tests::{test}"), "--nocapture"])
        .output()
        .expect("isolated configuration test");
    assert!(
        output.status.success(),
        "{test}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

fn parse(args: &[&str]) -> Result<Config, clap::Error> {
    parse_from(
        ["kronika-web", "--storage-dir", "/recording"]
            .into_iter()
            .chain(args.iter().copied()),
    )
}

#[test]
fn required_options_and_optional_defaults_are_preserved() {
    isolated(
        "required_options_and_optional_defaults_are_preserved",
        &[],
        || {
            let config = parse(&[]).expect("required CLI configuration");
            assert_eq!(config.data_root, std::path::Path::new("/recording"));
            assert_eq!(
                config.listen,
                "127.0.0.1:8080".parse().expect("default address")
            );
            assert_eq!(config.sources, 3);
            assert_eq!(config.account, None);
            assert!(!config.synthetic_demo);
            assert_eq!(config.export_gate.available_permits(), 1);
            for (value, expected) in [
                ("none", 0),
                ("os", 1),
                ("postgresql", 2),
                ("all", 3),
                ("0", 0),
                ("1", 1),
                ("2", 2),
                ("3", 3),
            ] {
                assert_eq!(parse(&["--sources", value]).expect(value).sources, expected);
            }
            for args in [vec!["kronika-web"], vec!["kronika-web", "--sources", "os"]] {
                assert_eq!(
                    parse_from(args)
                        .expect_err("missing required option")
                        .kind(),
                    ErrorKind::MissingRequiredArgument,
                );
            }
        },
    );
}

#[test]
fn environment_fallbacks_preserve_existing_service_configuration() {
    isolated(
        "environment_fallbacks_preserve_existing_service_configuration",
        &[
            ("KRONIKA_STORAGE_DIR", "/service-recording".into()),
            ("KRONIKA_WEB_LISTEN", "[::1]:9090".into()),
            ("KRONIKA_WEB_SOURCES", "2".into()),
            ("KRONIKA_WEB_USER", "service-user".into()),
            ("KRONIKA_WEB_PASSWORD", "service-password".into()),
            ("KRONIKA_WEB_DEMO", "synthetic".into()),
        ],
        || {
            let config = parse_from(["kronika-web"]).expect("environment-only service");
            assert_eq!(config.data_root, std::path::Path::new("/service-recording"));
            assert_eq!(config.listen, "[::1]:9090".parse().expect("IPv6 address"));
            assert_eq!(config.sources, 2);
            assert!(config.synthetic_demo);
            let account = config.account.expect("configured account");
            assert_eq!(account.user, "service-user");
            assert_eq!(account.password, "service-password");
        },
    );
}

#[test]
fn cli_overrides_even_invalid_environment_values() {
    isolated(
        "cli_overrides_even_invalid_environment_values",
        &[
            ("KRONIKA_STORAGE_DIR", "/old-recording".into()),
            ("KRONIKA_WEB_LISTEN", "not-an-address".into()),
            ("KRONIKA_WEB_SOURCES", "unsupported".into()),
            ("KRONIKA_WEB_USER", "".into()),
            ("KRONIKA_WEB_PASSWORD", "".into()),
            ("KRONIKA_WEB_DEMO", "invalid".into()),
        ],
        || {
            let config = parse(&[
                "--sources",
                "os",
                "--listen",
                "0.0.0.0:8081",
                "--user",
                "cli-user",
                "--password",
                "cli-password",
                "--demo",
                "synthetic",
            ])
            .expect("CLI values replace environment before validation");
            assert_eq!(config.data_root, std::path::Path::new("/recording"));
            assert_eq!(config.listen, "0.0.0.0:8081".parse().expect("IPv4 address"));
            assert_eq!(config.sources, 1);
            assert!(config.synthetic_demo);
            let account = config.account.expect("CLI account");
            assert_eq!(account.user, "cli-user");
            assert_eq!(account.password, "cli-password");
        },
    );
}

#[test]
fn cli_and_environment_credentials_can_be_combined() {
    isolated(
        "cli_and_environment_credentials_can_be_combined",
        &[("KRONIKA_WEB_PASSWORD", "env-password".into())],
        || {
            let config =
                parse(&["--user", "cli-user"]).expect("credential fallbacks are independent");
            let account = config.account.expect("combined account");
            assert_eq!(account.user, "cli-user");
            assert_eq!(account.password, "env-password");
        },
    );
}

#[test]
fn help_and_version_ignore_invalid_env_and_hide_values() {
    isolated(
        "help_and_version_ignore_invalid_env_and_hide_values",
        &[
            ("KRONIKA_STORAGE_DIR", "/PRIVATE_PATH".into()),
            ("KRONIKA_WEB_LISTEN", "invalid".into()),
            ("KRONIKA_WEB_SOURCES", "invalid".into()),
            ("KRONIKA_WEB_USER", "PRIVATE_USER".into()),
            ("KRONIKA_WEB_PASSWORD", "PRIVATE_PASSWORD".into()),
            ("KRONIKA_WEB_DEMO", "invalid".into()),
        ],
        || {
            for (flag, expected) in [
                ("--help", ErrorKind::DisplayHelp),
                ("-h", ErrorKind::DisplayHelp),
                ("--version", ErrorKind::DisplayVersion),
            ] {
                let error = parse_from(["kronika-web", flag]).expect_err("display then exit");
                assert_eq!(error.kind(), expected);
                let text = error.to_string();
                for private in ["PRIVATE_PATH", "PRIVATE_USER", "PRIVATE_PASSWORD"] {
                    assert!(!text.contains(private), "{text}");
                }
                if expected == ErrorKind::DisplayHelp {
                    for option in [
                        "--storage-dir",
                        "--listen",
                        "--sources",
                        "--user",
                        "--password",
                        "--demo",
                    ] {
                        assert!(text.contains(option), "{text}");
                    }
                }
            }
        },
    );
}

#[test]
fn invalid_cli_configuration_reports_options_without_credentials() {
    isolated(
        "invalid_cli_configuration_reports_options_without_credentials",
        &[],
        || {
            for (args, expected) in [
                (vec!["--user", "PRIVATE_USER"], "--password"),
                (vec!["--password", "PRIVATE_PASSWORD"], "--user"),
                (
                    vec!["--user", "", "--password", "PRIVATE_PASSWORD"],
                    "is empty",
                ),
                (vec!["--user", "PRIVATE_USER", "--password", ""], "is empty"),
                (vec!["--listen", "localhost:8080"], "--listen"),
                (vec!["--sources", "4"], "--sources"),
                (vec!["--sources", "invalid"], "--sources"),
                (vec!["--demo", "true"], "--demo"),
                (vec!["--demo", ""], "--demo"),
                (
                    vec![
                        "--user",
                        "PRIVATE_USER",
                        "--password",
                        "PRIVATE_PASSWORD",
                        "--unknown",
                    ],
                    "--unknown",
                ),
            ] {
                let error = parse(&args).expect_err("invalid configuration").to_string();
                assert!(error.contains(expected), "{error}");
                for private in ["PRIVATE_USER", "PRIVATE_PASSWORD"] {
                    assert!(!error.contains(private), "{error}");
                }
            }
        },
    );
}

#[cfg(unix)]
#[test]
fn non_unicode_credential_errors_do_not_echo_secret_bytes() {
    use std::os::unix::ffi::OsStringExt as _;

    isolated(
        "non_unicode_credential_errors_do_not_echo_secret_bytes",
        &[(
            "KRONIKA_WEB_PASSWORD",
            OsString::from_vec(b"PRIVATE_PASSWORD\xff".to_vec()),
        )],
        || {
            let error = parse(&["--user", "PRIVATE_USER"]).expect_err("invalid Unicode");
            assert_eq!(error.kind(), ErrorKind::InvalidUtf8);
            assert!(!error.to_string().contains("PRIVATE_PASSWORD"));
            assert!(!error.to_string().contains("PRIVATE_USER"));
            let config = parse(&["--user", "cli-user", "--password", "replacement"])
                .expect("CLI replaces invalid credential bytes");
            assert_eq!(config.account.expect("CLI account").password, "replacement");
            for flag in ["--user", "--password"] {
                let error = parse_from([
                    OsString::from("kronika-web"),
                    "--storage-dir".into(),
                    "/recording".into(),
                    "--sources".into(),
                    "os".into(),
                    flag.into(),
                    OsString::from_vec(b"PRIVATE_CREDENTIAL\xff".to_vec()),
                ])
                .expect_err("invalid CLI Unicode");
                assert_eq!(error.kind(), ErrorKind::InvalidUtf8);
                assert!(!error.to_string().contains("PRIVATE_CREDENTIAL"));
            }
        },
    );
}
