use super::{account, source_set, synthetic_demo};

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
    assert_eq!(source_set(Some("0".to_owned())).expect("no sources"), 0);
    assert_eq!(source_set(Some("1".to_owned())).expect("OS"), 1);
    assert_eq!(source_set(Some("2".to_owned())).expect("PostgreSQL"), 2);
    assert_eq!(source_set(Some("3".to_owned())).expect("all sources"), 3);
    assert!(source_set(None).is_err());
    assert!(source_set(Some("postgres".to_owned())).is_err());
    assert!(source_set(Some("4".to_owned())).is_err());
}

#[test]
fn synthetic_demo_mode_is_explicit() {
    assert!(!synthetic_demo(None).expect("production default"));
    assert!(synthetic_demo(Some("synthetic")).expect("demo"));
    assert!(synthetic_demo(Some("true")).is_err());
    assert!(synthetic_demo(Some("")).is_err());
}
