use hyper::HeaderMap;
use hyper::header::{ACCEPT_ENCODING, HeaderValue};

use super::{AcceptedEncodings, ContentCoding, etag_matches};

fn accepted(value: Option<&str>) -> Option<AcceptedEncodings> {
    let mut headers = HeaderMap::new();
    if let Some(value) = value {
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_str(value).expect("valid header value"),
        );
    }
    AcceptedEncodings::from_headers(&headers)
}

#[test]
fn absent_or_positive_gzip_keeps_api_compression_adaptive() {
    for value in [None, Some("gzip"), Some("GZip; q=0.250"), Some("*")] {
        let encodings = accepted(value).expect("an encoding is acceptable");
        assert_eq!(encodings.for_small(), ContentCoding::Identity, "{value:?}");
        assert_eq!(encodings.for_large(), ContentCoding::Gzip, "{value:?}");
    }
}

#[test]
fn ui_defaults_to_identity_and_uses_explicit_gzip() {
    assert_eq!(
        accepted(None).expect("absent header").for_ui(),
        ContentCoding::Identity
    );
    for value in ["", "identity", "gzip;q=0", "gzip;q=0, *;q=1"] {
        assert_eq!(
            accepted(Some(value))
                .expect("identity is acceptable")
                .for_ui(),
            ContentCoding::Identity,
            "{value}"
        );
    }
    for value in ["gzip", "GZip, br", "*", "identity;q=0, *;q=0.5"] {
        assert_eq!(
            accepted(Some(value)).expect("gzip is acceptable").for_ui(),
            ContentCoding::Gzip,
            "{value}"
        );
    }
}

#[test]
fn ui_uses_quality_weights_and_prefers_gzip_on_a_tie() {
    for value in ["gzip;q=0.2, identity;q=0.8", "gzip;q=0.5", "*;q=0.5"] {
        assert_eq!(
            accepted(Some(value))
                .expect("identity is acceptable")
                .for_ui(),
            ContentCoding::Identity,
            "{value}"
        );
    }
    for value in ["gzip;q=0.8, identity;q=0.2", "gzip;q=0.5, identity;q=0.5"] {
        assert_eq!(
            accepted(Some(value)).expect("gzip is acceptable").for_ui(),
            ContentCoding::Gzip,
            "{value}"
        );
    }
}

#[test]
fn identity_only_and_gzip_only_preferences_are_respected() {
    for value in ["", "identity", "gzip;q=0", "*;q=0, identity;q=1"] {
        let encodings = accepted(Some(value)).expect("identity is acceptable");
        assert_eq!(encodings.for_small(), ContentCoding::Identity, "{value}");
        assert_eq!(encodings.for_large(), ContentCoding::Identity, "{value}");
    }
    for value in [
        "identity;q=0, gzip",
        "*;q=0, gzip;q=0.001",
        "identity;q=0, *;q=0.500",
    ] {
        let encodings = accepted(Some(value)).expect("gzip is acceptable");
        assert_eq!(encodings.for_small(), ContentCoding::Gzip, "{value}");
        assert_eq!(encodings.for_large(), ContentCoding::Gzip, "{value}");
    }
}

#[test]
fn explicit_codings_override_the_wildcard() {
    let encodings = accepted(Some("gzip;q=0, *;q=1")).expect("identity remains acceptable");
    assert_eq!(encodings.for_large(), ContentCoding::Identity);

    let encodings = accepted(Some("identity;q=1, *;q=0")).expect("identity is explicit");
    assert_eq!(encodings.for_large(), ContentCoding::Identity);
}

#[test]
fn zero_or_invalid_qualities_can_refuse_every_representation() {
    for value in [
        "gzip;q=0, identity;q=0",
        "*;q=0",
        "gzip;q=1.001, identity;q=0",
        "gzip;q=.5, identity;q=0",
        "gzip;q=0.0000, identity;q=0",
    ] {
        assert_eq!(accepted(Some(value)), None, "{value}");
    }
}

#[test]
fn valid_http_qvalues_cover_zero_one_and_three_decimal_places() {
    for value in ["gzip;q=0.001", "gzip;q=0.5", "gzip;q=1", "gzip;q=1.000"] {
        assert_eq!(
            accepted(Some(value)).expect("valid qvalue").for_large(),
            ContentCoding::Gzip,
            "{value}"
        );
    }
}

#[test]
fn repeated_field_lines_are_combined() {
    let mut headers = HeaderMap::new();
    headers.append(
        ACCEPT_ENCODING,
        HeaderValue::from_static("gzip;q=0, identity;q=0"),
    );
    headers.append(ACCEPT_ENCODING, HeaderValue::from_static("gzip;q=0.5"));
    let encodings = AcceptedEncodings::from_headers(&headers).expect("second field permits gzip");
    assert_eq!(encodings.for_small(), ContentCoding::Gzip);
    assert_eq!(encodings.for_large(), ContentCoding::Gzip);
}

#[test]
fn entity_tags_use_weak_comparison() {
    for offered in [
        "\"1234abcd\"",
        "W/\"1234abcd\"",
        "\"other\", W/\"1234abcd\"",
        "*",
    ] {
        assert!(etag_matches(offered, "W/\"1234abcd\""), "{offered}");
    }
    assert!(!etag_matches("\"1234abce\"", "W/\"1234abcd\""));
}
