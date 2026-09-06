//! The single authentication check every request passes.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};

use crate::config::Account;

const AUTHORIZATION_MAX_BYTES: usize = 8 * 1024;
const COOKIE_HEADER_MAX_BYTES: usize = 8 * 1024;
const HMAC_BLOCK_BYTES: usize = 64;
const SESSION_COOKIE_NAME: &str = "kronika_session";
const SESSION_KEY_DOMAIN: &[u8] = b"kronika session key v1\0";
const SESSION_SIGNATURE_BYTES: usize = 32;
const SESSION_SIGNATURE_TEXT_BYTES: usize = 43;
/// Lifetime of an issued browser session in seconds.
pub(crate) const SESSION_MAX_AGE: u64 = 30 * 24 * 60 * 60;

/// Whether `header` carries the configured Basic credentials.
pub(crate) fn admits_basic(account: &Account, header: Option<&str>) -> bool {
    let Some(offered) = credentials(header) else {
        return false;
    };
    same(
        &credential_digest(&offered),
        &configured_credential_digest(account),
    )
}

/// Whether `header` carries a current session signed for this account.
pub(crate) fn admits_session(account: &Account, header: Option<&str>, now: u64) -> bool {
    let Some(token) = session_token(header) else {
        return false;
    };
    let mut components = token.split('.');
    let (Some(version), Some(expiry_text), Some(signature_text)) =
        (components.next(), components.next(), components.next())
    else {
        return false;
    };
    if components.next().is_some()
        || version != "v1"
        || expiry_text.is_empty()
        || expiry_text.len() > 20
        || (expiry_text.len() > 1 && expiry_text.starts_with('0'))
        || !expiry_text.bytes().all(|byte| byte.is_ascii_digit())
        || signature_text.len() != SESSION_SIGNATURE_TEXT_BYTES
    {
        return false;
    }
    let Ok(expiry) = expiry_text.parse::<u64>() else {
        return false;
    };
    if expiry <= now {
        return false;
    }
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(signature_text) else {
        return false;
    };
    let Ok(offered) = <[u8; SESSION_SIGNATURE_BYTES]>::try_from(decoded.as_slice()) else {
        return false;
    };
    if URL_SAFE_NO_PAD.encode(offered) != signature_text {
        return false;
    }
    let message = format!("v1.{expiry_text}");
    same(&offered, &signature(account, message.as_bytes()))
}

/// A persistent session cookie signed for the configured account.
pub(crate) fn issue_cookie(account: &Account, now: u64, secure: bool) -> String {
    let expiry = now.saturating_add(SESSION_MAX_AGE);
    let message = format!("v1.{expiry}");
    let signature = URL_SAFE_NO_PAD.encode(signature(account, message.as_bytes()));
    cookie(
        &format!("{SESSION_COOKIE_NAME}={message}.{signature}"),
        SESSION_MAX_AGE,
        secure,
    )
}

/// An expired cookie with the same scope as an issued session.
pub(crate) fn clear_cookie(secure: bool) -> String {
    cookie(&format!("{SESSION_COOKIE_NAME}="), 0, secure)
}

fn credentials(header: Option<&str>) -> Option<Vec<u8>> {
    let header = header?;
    if header.len() > AUTHORIZATION_MAX_BYTES {
        return None;
    }
    let (scheme, value) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    STANDARD.decode(value.trim()).ok()
}

fn configured_credential_digest(account: &Account) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"kronika basic credentials v1\0");
    digest.update(account.user.as_bytes());
    digest.update(b":");
    digest.update(account.password.as_bytes());
    digest.finalize().into()
}

fn credential_digest(credentials: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"kronika basic credentials v1\0");
    digest.update(credentials);
    digest.finalize().into()
}

fn session_token(header: Option<&str>) -> Option<&str> {
    let header = header?;
    if header.len() > COOKIE_HEADER_MAX_BYTES {
        return None;
    }
    let mut found = None;
    for pair in header.split(';').map(str::trim) {
        if pair == SESSION_COOKIE_NAME {
            return None;
        }
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name != SESSION_COOKIE_NAME {
            continue;
        }
        if found.replace(value).is_some() {
            return None;
        }
    }
    found
}

fn signature(account: &Account, message: &[u8]) -> [u8; 32] {
    let mut key_digest = Sha256::new();
    key_digest.update(SESSION_KEY_DOMAIN);
    key_digest.update(Sha256::digest(account.user.as_bytes()));
    key_digest.update(Sha256::digest(account.password.as_bytes()));
    hmac_sha256(&key_digest.finalize(), message)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0_u8; HMAC_BLOCK_BYTES];
    if key.len() > HMAC_BLOCK_BYTES {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; HMAC_BLOCK_BYTES];
    let mut outer_pad = [0x5c_u8; HMAC_BLOCK_BYTES];
    for ((inner, outer), byte) in inner_pad.iter_mut().zip(&mut outer_pad).zip(block) {
        *inner ^= byte;
        *outer ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn cookie(value: &str, max_age: u64, secure: bool) -> String {
    let secure_attribute = if secure { "; Secure" } else { "" };
    format!("{value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure_attribute}")
}

fn same(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests;
