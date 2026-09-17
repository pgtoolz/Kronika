use super::{CollectorLog, Config, command};
use std::path::Path;

fn parse(args: &[&str]) -> anyhow::Result<Config> {
    let matches = command()
        .mut_args(|arg| arg.env(None::<&str>))
        .try_get_matches_from(std::iter::once("kronika-demo").chain(args.iter().copied()))?;
    Config::from_matches(&matches)
}

#[test]
fn default_configuration_preserves_existing_profile() {
    let config = parse(&[]).unwrap();
    assert_eq!(config.root, Path::new("demo-data"));
    assert_eq!(config.storage_dir, Path::new("demo-data/segments"));
    assert_eq!(config.duration_s, 60);
    assert_eq!(config.collector_log, CollectorLog::File);
    assert!(config.system_activity.is_some());
    assert!(config.workload.is_none());
}

#[test]
fn paths_log_and_human_duration_are_configured_without_startup() {
    let config = parse(&[
        "--dir",
        "/demo",
        "--storage-dir",
        "/recordings",
        "--collector-bin",
        "/tools/collector",
        "--collector-log",
        "stderr",
        "--duration-s",
        "1m 30s",
    ])
    .unwrap();
    assert_eq!(config.root, Path::new("/demo"));
    assert_eq!(config.storage_dir, Path::new("/recordings"));
    assert_eq!(config.collector_bin, Path::new("/tools/collector"));
    assert_eq!(config.collector_log, CollectorLog::Stderr);
    assert_eq!(config.duration_s, 90);
    assert_eq!(parse(&["--duration-s", "0"]).unwrap().duration_s, 0);
    assert!(parse(&["--duration-s", "500ms"]).is_err());
    assert!(parse(&["--collector-log", "stdout"]).is_err());
}

#[test]
fn inactive_workloads_ignore_their_other_controls() {
    let config = parse(&[
        "--system-workload-enabled",
        "false",
        "--system-cpu-percent",
        "invalid",
        "--workload-sessions",
        "invalid",
        "--workload-direct-dsn",
        "invalid",
    ])
    .unwrap();
    assert!(config.system_activity.is_none());
    assert!(config.workload.is_none());
}

#[test]
fn workload_requires_direct_dsn_and_keeps_invalid_dsns_private() {
    assert!(parse(&["--workload-dsn", "host=localhost"]).is_err());
    let error = parse(&[
        "--workload-dsn",
        "host='SECRET",
        "--workload-direct-dsn",
        "host=localhost",
    ])
    .err()
    .unwrap();
    assert!(!format!("{error:#}").contains("SECRET"));
    let config = parse(&[
        "--workload-dsn",
        "host=localhost",
        "--workload-direct-dsn",
        "host=localhost",
        "--workload-lock-hold-ms",
        "4s",
    ])
    .unwrap();
    let workload = config.workload.unwrap();
    assert_eq!(workload.lock_hold_ms, 4000);
    assert_eq!(workload.schemas, 1);
    assert_eq!(workload.sessions, 4);
    assert_eq!(workload.transactions_per_second, 20);
}

#[test]
fn byte_sizes_preserve_the_named_units_and_hard_bounds() {
    assert!(
        parse(&[
            "--system-memory-mib",
            "32MiB",
            "--system-file-mib",
            "1MiB",
            "--system-disk-kib-per-s",
            "32KiB",
            "--system-flush-interval-s",
            "1s"
        ])
        .is_ok()
    );
    assert!(parse(&["--system-memory-mib", "1GiB"]).is_err());
    assert!(parse(&["--system-memory-mib", "33MB"]).is_err());
}

#[test]
fn help_and_version_are_generated_without_configuration_validation() {
    for (flag, kind) in [
        ("--help", clap::error::ErrorKind::DisplayHelp),
        ("--version", clap::error::ErrorKind::DisplayVersion),
    ] {
        let error = command()
            .try_get_matches_from(["kronika-demo", flag])
            .unwrap_err();
        assert_eq!(error.kind(), kind);
    }
    let help = command().render_long_help().to_string();
    assert!(help.contains("--workload-direct-dsn"));
    assert!(help.contains("KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED"));
    assert!(help.contains("1m"));
}

#[test]
fn environment_fallback_and_cli_precedence_are_isolated() {
    const CASE: &str = "KRONIKA_TEST_DEMO_CLI_CASE";
    if let Ok(case) = std::env::var(CASE) {
        if case == "help" {
            let error = super::parse_from(["kronika-demo", "--help"]).err().unwrap();
            assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
            assert!(!error.to_string().contains("PRIVATE_PASSWORD"));
        } else {
            let args = if case == "override" {
                vec![
                    "kronika-demo",
                    "--duration-s",
                    "2m",
                    "--collector-log",
                    "file",
                ]
            } else {
                vec!["kronika-demo"]
            };
            let config = super::parse_from(args).unwrap();
            assert_eq!(config.duration_s, if case == "override" { 120 } else { 90 });
            assert_eq!(
                config.collector_log,
                if case == "override" {
                    CollectorLog::File
                } else {
                    CollectorLog::Stderr
                }
            );
            assert!(config.system_activity.is_none());
        }
        return;
    }
    // Set only child-process environments; concurrent tests never share mutation.
    for case in ["environment", "override", "help", "inactive-non-unicode"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .env_clear()
            .env(CASE, case)
            .env(
                "KRONIKA_DEMO_DURATION_S",
                if matches!(case, "environment" | "inactive-non-unicode") {
                    "90"
                } else {
                    "invalid"
                },
            )
            .env("KRONIKA_DEMO_COLLECTOR_LOG", "stderr")
            .env("KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED", "false")
            .env("KRONIKA_DEMO_SYSTEM_CPU_PERCENT", "invalid")
            .env("KRONIKA_DEMO_WORKLOAD_SESSIONS", "invalid")
            .args([
                "--exact",
                "config::tests::environment_fallback_and_cli_precedence_are_isolated",
            ]);
        if case == "help" {
            child.env(
                "KRONIKA_DEMO_WORKLOAD_DSN",
                "host=localhost password=PRIVATE_PASSWORD",
            );
        }
        if case == "inactive-non-unicode" {
            use std::os::unix::ffi::OsStringExt as _;
            for key in [
                "KRONIKA_DEMO_SYSTEM_CPU_PERCENT",
                "KRONIKA_DEMO_WORKLOAD_SESSIONS",
                "KRONIKA_DEMO_WORKLOAD_DIRECT_DSN",
            ] {
                child.env(key, std::ffi::OsString::from_vec(vec![0xff]));
            }
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{case}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
