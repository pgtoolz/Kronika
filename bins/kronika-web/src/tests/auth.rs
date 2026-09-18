use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::{
    COOKIE_HEADER_MAX_BYTES, SESSION_COOKIE_NAME, SESSION_MAX_AGE, admits_basic, admits_session,
    clear_cookie, hmac_sha256, issue_cookie,
};
use crate::config::Account;

const CORRECT: &str = "Basic ZGJhOnNlY3JldA==";
const NOW: u64 = 1_786_579_200;

fn account() -> Account {
    Account {
        user: "dba".to_owned(),
        password: "secret".to_owned(),
    }
}

#[test]
fn basic_admits_the_configured_account() {
    assert!(admits_basic(&account(), Some(CORRECT)));
}

#[test]
fn basic_scheme_is_case_insensitive() {
    assert!(admits_basic(&account(), Some("basic ZGJhOnNlY3JldA==")));
}

#[test]
fn basic_rejects_missing_malformed_and_wrong_credentials() {
    for invalid in [
        None,
        Some("Basic ZGJhOndyb25n"),
        Some("Bearer ZGJhOnNlY3JldA=="),
        Some("Basic not-base64!!"),
    ] {
        assert!(!admits_basic(&account(), invalid), "{invalid:?}");
    }
}

#[test]
fn basic_rejects_an_oversized_authorization_value() {
    let oversized = format!("Basic {}", "A".repeat(9_000));
    assert!(!admits_basic(&account(), Some(&oversized)));
}

#[test]
fn local_hmac_matches_the_rfc_4231_sha256_vector() {
    let actual = hmac_sha256(&[0x0b; 20], b"Hi There");
    assert_eq!(
        hex(&actual),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}

#[test]
fn issued_cookie_has_the_persistent_first_party_scope() {
    let cookie = issue_cookie(&account(), NOW, false);
    assert!(cookie.starts_with(&format!("{SESSION_COOKIE_NAME}=v1.")));
    assert!(cookie.contains("; Path=/; HttpOnly; SameSite=Strict;"));
    assert!(cookie.contains(&format!("Max-Age={SESSION_MAX_AGE}")));
    assert!(!cookie.contains("; Secure"));
}

#[test]
fn issued_cookie_contains_no_configured_or_basic_credentials() {
    let cookie = issue_cookie(&account(), NOW, false);
    assert!(!cookie.contains("dba"));
    assert!(!cookie.contains("secret"));
    assert!(!cookie.contains(CORRECT));
    assert!(!cookie.contains(&STANDARD.encode("dba:secret")));
}

#[test]
fn secure_attribute_applies_to_issue_and_clear() {
    assert!(issue_cookie(&account(), NOW, true).ends_with("; Secure"));
    assert!(clear_cookie(true).ends_with("; Secure"));
}

#[test]
fn clear_cookie_matches_the_insecure_session_scope() {
    assert_eq!(
        clear_cookie(false),
        "kronika_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
    );
}

#[test]
fn session_is_valid_until_but_not_at_its_expiry() {
    let header = request_cookie(&issue_cookie(&account(), NOW, false));
    assert!(admits_session(&account(), Some(&header), NOW));
    assert!(admits_session(
        &account(),
        Some(&header),
        NOW + SESSION_MAX_AGE - 1
    ));
    assert!(!admits_session(
        &account(),
        Some(&header),
        NOW + SESSION_MAX_AGE
    ));
}

#[test]
fn password_change_invalidates_an_existing_session() {
    let header = request_cookie(&issue_cookie(&account(), NOW, false));
    let changed = Account {
        user: "dba".to_owned(),
        password: "changed".to_owned(),
    };
    assert!(!admits_session(&changed, Some(&header), NOW));
}

#[test]
fn user_change_invalidates_an_existing_session() {
    let header = request_cookie(&issue_cookie(&account(), NOW, false));
    let changed = Account {
        user: "other".to_owned(),
        password: "secret".to_owned(),
    };
    assert!(!admits_session(&changed, Some(&header), NOW));
}

#[test]
fn account_field_boundaries_cannot_reuse_a_session() {
    let original = Account {
        user: "dba".to_owned(),
        password: "part:secret".to_owned(),
    };
    let header = request_cookie(&issue_cookie(&original, NOW, false));
    let regrouped = Account {
        user: "dba:part".to_owned(),
        password: "secret".to_owned(),
    };

    assert!(!admits_session(&regrouped, Some(&header), NOW));
}

#[test]
fn session_rejects_version_expiry_and_signature_tampering() {
    let header = request_cookie(&issue_cookie(&account(), NOW, false));
    let token = header
        .strip_prefix(&format!("{SESSION_COOKIE_NAME}="))
        .expect("cookie name");
    let mut parts = token.split('.');
    assert_eq!(parts.next(), Some("v1"));
    let expiry = parts.next().expect("expiry");
    let signature = parts.next().expect("signature");
    assert_eq!(parts.next(), None);

    for invalid in [
        format!("{SESSION_COOKIE_NAME}=v2.{expiry}.{signature}"),
        format!("{SESSION_COOKIE_NAME}=v1.0{expiry}.{signature}"),
        format!("{SESSION_COOKIE_NAME}=v1.{expiry}.{signature}.extra"),
        format!("{SESSION_COOKIE_NAME}=v1.{expiry}.short"),
        format!("{SESSION_COOKIE_NAME}=v1.{}.{signature}", NOW + 1),
        format!(
            "{SESSION_COOKIE_NAME}=v1.{expiry}.{}",
            "A".repeat(signature.len())
        ),
    ] {
        assert!(
            !admits_session(&account(), Some(&invalid), NOW),
            "{invalid}"
        );
    }
}

#[test]
fn session_rejects_a_noncanonical_signature_encoding() {
    let header = request_cookie(&issue_cookie(&account(), NOW, false));
    let mut alias = header.into_bytes();
    let last = alias.last_mut().expect("signature character");
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let index = alphabet
        .iter()
        .position(|candidate| *candidate == *last)
        .expect("URL-safe signature character");
    *last = alphabet[(index & !3) | ((index + 1) & 3)];
    let alias = String::from_utf8(alias).expect("ASCII token");

    assert!(!admits_session(&account(), Some(&alias), NOW));
}

#[test]
fn session_cookie_selection_is_exact() {
    let valid = request_cookie(&issue_cookie(&account(), NOW, false));
    assert!(admits_session(
        &account(),
        Some(&format!("theme=dark; {valid}; locale=en")),
        NOW
    ));
    assert!(!admits_session(
        &account(),
        Some(&format!("not_{valid}")),
        NOW
    ));
    assert!(!admits_session(
        &account(),
        Some(&format!("{SESSION_COOKIE_NAME}; {valid}")),
        NOW
    ));
}

#[test]
fn session_cookie_must_occur_once() {
    let valid = request_cookie(&issue_cookie(&account(), NOW, false));
    assert!(!admits_session(
        &account(),
        Some(&format!("{valid}; {valid}")),
        NOW
    ));
    assert!(!admits_session(
        &account(),
        Some(&format!("{SESSION_COOKIE_NAME}=x; {valid}")),
        NOW
    ));
}

#[test]
fn session_cookie_header_is_bounded() {
    let oversized = "x".repeat(COOKIE_HEADER_MAX_BYTES + 1);
    assert!(!admits_session(&account(), Some(&oversized), NOW));
}

fn request_cookie(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .expect("cookie value")
        .to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut output, byte| {
        write!(output, "{byte:02x}").expect("write to string");
        output
    })
}
