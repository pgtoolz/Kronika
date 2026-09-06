use super::{account, authentication_required, source_set, synthetic_demo};

#[test]
fn both_nonempty_credentials_are_required() {
    let made = account(Some("dba".to_owned()), Some("secret".to_owned())).expect("account");
    assert_eq!(made.user, "dba");
    assert_eq!(made.password, "secret");
    assert!(account(None, None).is_err());
    assert!(account(Some(String::new()), Some("secret".to_owned())).is_err());
    assert!(account(Some("dba".to_owned()), Some(String::new())).is_err());
}

#[test]
fn account_debug_output_redacts_credentials() {
    let made = account(Some("dba".to_owned()), Some("secret".to_owned())).expect("account");
    let debug = format!("{made:?}");
    assert_eq!(debug, "Account { credentials: [redacted] }");
    assert!(!debug.contains("dba"));
    assert!(!debug.contains("secret"));
}

#[test]
fn authentication_is_disabled_only_explicitly() {
    assert!(authentication_required(None).expect("default"));
    assert!(authentication_required(Some("required")).expect("required"));
    assert!(!authentication_required(Some("disabled")).expect("disabled"));
    assert!(authentication_required(Some("false")).is_err());
    assert!(authentication_required(Some("")).is_err());
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
