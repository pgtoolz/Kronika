use super::Transport;

#[test]
fn explicit_transport_does_not_read_the_ca_environment() {
    const CHILD: &str = "KRONIKA_TEST_EXPLICIT_TRANSPORT";
    if std::env::var_os(CHILD).is_some() {
        assert!(Transport::from_env().is_err());
        let transport = Transport::from_ca_file(None).expect("compiled public CA roots");
        let pool = crate::Pool::with_transport(
            "host=example.invalid user=monitor dbname=metrics",
            transport,
        )
        .expect("use explicitly configured transport");
        assert_eq!(pool.database_label(), "metrics");
        assert_eq!(pool.connection_label(0), "monitor@example.invalid:5432");
        assert_eq!(pool.generation(), None);
        assert_eq!(pool.on_database("other").database_label(), "other");
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "transport::tests::explicit_transport_does_not_read_the_ca_environment",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("KRONIKA_PG_SSL_ROOT_CERT", "/nonexistent/private-ca.pem")
        .output()
        .expect("run isolated transport configuration");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn explicit_ca_file_errors_do_not_disclose_the_path() {
    let path = std::path::Path::new("/nonexistent/private-ca.pem");
    let error = Transport::from_ca_file(Some(path)).expect_err("missing CA fails closed");
    assert!(error.is::<super::CaConfigError>());
    assert!(!format!("{error:#}").contains("private-ca"));
}

#[test]
fn empty_or_invalid_custom_ca_never_disables_verification() {
    assert!(Transport::from_pem(b"").is_err());
    let error = Transport::from_pem(b"not a certificate").expect_err("invalid CA fails closed");
    assert!(error.is::<super::CaConfigError>());
    assert!(error.to_string().contains("KRONIKA_PG_SSL_ROOT_CERT"));
    assert!(
        Transport::from_pem(b"-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----\n")
            .is_err()
    );
}
