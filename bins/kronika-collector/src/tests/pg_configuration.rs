use crate::pg_sources;

#[test]
fn invalid_ca_configuration_is_reported_without_dsn_or_file_secrets() {
    const CHILD: &str = "KRONIKA_TEST_CA_DIAGNOSTIC_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let config = crate::config::Config::from_env().expect("valid collector settings and DSN");
        for error in [
            pg_sources::open(&config).expect_err("invalid CA rejects metrics"),
            crate::log_sources::LogSources::open(&config)
                .expect_err("invalid CA rejects discovery"),
        ] {
            let text = format!("{error:#}");
            assert!(text.contains("KRONIKA_PG_SSL_ROOT_CERT"));
            assert!(!text.contains("not a valid connection string"));
            assert!(!text.contains("keep-secret"));
            assert!(!text.contains("private-ca-path"));
        }
        return;
    }
    let directory = tempfile::tempdir().expect("CA fixture directory");
    let ca = directory.path().join("private-ca-path.pem");
    for contents in [
        None,
        Some(""),
        Some("-----BEGIN CERTIFICATE-----\nkeep-secret\n-----END CERTIFICATE-----\n"),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&ca, contents).expect("write invalid CA fixture");
        }
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("KRONIKA_") {
                command.env_remove(name);
            }
        }
        let result = command
            .args([
                "--exact",
                "tests::pg_configuration::invalid_ca_configuration_is_reported_without_dsn_or_file_secrets",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("KRONIKA_STORAGE_DIR", directory.path().join("recording"))
            .env("KRONIKA_PG_DSN", "host=127.0.0.1 user=monitor password=keep-secret dbname=metrics sslmode=require")
            .env("KRONIKA_PG_SSL_ROOT_CERT", &ca)
            .output().expect("run isolated constructor check");
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
    }
}
