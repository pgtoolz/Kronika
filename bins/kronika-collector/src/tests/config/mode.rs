use super::CollectorMode;

#[test]
fn only_explicit_postgresql_mode_disables_linux() {
    assert!(
        CollectorMode::parse("local")
            .expect("local mode")
            .collect_os()
    );
    assert_eq!(
        CollectorMode::parse("postgresql").expect("PostgreSQL mode"),
        CollectorMode::Postgresql
    );
    assert!(!CollectorMode::Postgresql.collect_os());
    for invalid in ["", "remote", "2", "auto"] {
        assert!(
            CollectorMode::parse(invalid).is_err(),
            "reject unsupported {invalid:?}"
        );
    }
}
