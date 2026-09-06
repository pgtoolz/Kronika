//! Content-coding preferences for runtime API responses.

use hyper::HeaderMap;
use hyper::header::ACCEPT_ENCODING;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AcceptedEncodings {
    gzip: u16,
    identity: u16,
    header_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentCoding {
    Identity,
    Gzip,
}

impl Default for AcceptedEncodings {
    fn default() -> Self {
        Self {
            gzip: 1_000,
            identity: 1_000,
            header_present: false,
        }
    }
}

impl AcceptedEncodings {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let mut saw_header = false;
        let mut gzip = None;
        let mut identity = None;
        let mut wildcard = None;

        for value in headers.get_all(ACCEPT_ENCODING) {
            saw_header = true;
            let value = value.to_str().ok()?;
            for item in value.split(',') {
                let coding = item.split(';').next().unwrap_or_default().trim();
                if coding.is_empty() {
                    continue;
                }
                let quality = quality(item).unwrap_or(0);
                if coding.eq_ignore_ascii_case("gzip") {
                    keep_highest(&mut gzip, quality);
                } else if coding.eq_ignore_ascii_case("identity") {
                    keep_highest(&mut identity, quality);
                } else if coding == "*" {
                    keep_highest(&mut wildcard, quality);
                }
            }
        }

        if !saw_header {
            return Some(Self::default());
        }

        let gzip = gzip.or(wildcard).unwrap_or(0);
        let identity = identity.unwrap_or_else(|| if wildcard == Some(0) { 0 } else { 1_000 });
        (gzip > 0 || identity > 0).then_some(Self {
            gzip,
            identity,
            header_present: true,
        })
    }

    pub(super) const fn for_large(self) -> ContentCoding {
        if self.gzip > 0 {
            ContentCoding::Gzip
        } else {
            ContentCoding::Identity
        }
    }

    pub(super) const fn for_small(self) -> ContentCoding {
        if self.identity > 0 {
            ContentCoding::Identity
        } else {
            ContentCoding::Gzip
        }
    }

    pub(super) const fn for_ui(self) -> ContentCoding {
        if !self.header_present || self.identity > self.gzip {
            ContentCoding::Identity
        } else {
            ContentCoding::Gzip
        }
    }

    pub(crate) const fn accepts_identity(self) -> bool {
        self.identity > 0
    }
}

impl ContentCoding {
    pub(crate) const fn header(self) -> Option<&'static str> {
        match self {
            Self::Identity => None,
            Self::Gzip => Some("gzip"),
        }
    }
}

fn keep_highest(slot: &mut Option<u16>, quality: u16) {
    *slot = Some(slot.map_or(quality, |present| present.max(quality)));
}

fn quality(item: &str) -> Option<u16> {
    let mut quality = 1_000;
    let mut saw_quality = false;
    for parameter in item.split(';').skip(1) {
        let (name, value) = parameter.split_once('=')?;
        if saw_quality || !name.trim().eq_ignore_ascii_case("q") {
            return None;
        }
        quality = qvalue(value.trim())?;
        saw_quality = true;
    }
    Some(quality)
}

fn qvalue(value: &str) -> Option<u16> {
    let (whole, fraction) = value.split_once('.').map_or((value, ""), |parts| parts);
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match whole {
        "0" => {
            let mut thousandths = 0_u16;
            for (index, byte) in fraction.bytes().enumerate() {
                let digit = u16::from(byte - b'0');
                thousandths += digit * [100, 10, 1][index];
            }
            Some(thousandths)
        }
        "1" if fraction.bytes().all(|byte| byte == b'0') => Some(1_000),
        _ => None,
    }
}

/// Compare cache validators using the weak comparison required by
/// `If-None-Match` on GET and HEAD requests.
pub(crate) fn etag_matches(offered: &str, current: &str) -> bool {
    let current = current.strip_prefix("W/").unwrap_or(current).trim();
    offered.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate).trim() == current
    })
}

#[cfg(test)]
mod tests;
