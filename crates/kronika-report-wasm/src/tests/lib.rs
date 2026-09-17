use super::{BAD_REQUEST, INTERNAL_SERVER_ERROR, NOT_FOUND, ReportSession};

const SEGMENT_ID: &str = "1709164800000000";

#[test]
fn request_refusals_are_stable_and_bodyless() {
    let cases = [
        (
            ReportSession::new("not-decimal", Vec::new(), Vec::new(), 0, 0)
                .request("/api/catalog", ""),
            BAD_REQUEST,
            "bad_parameter",
            Some("segment_id"),
        ),
        (
            ReportSession::new(SEGMENT_ID, Vec::new(), Vec::new(), 0, 0).request("/missing", ""),
            NOT_FOUND,
            "no_such_path",
            None,
        ),
        (
            ReportSession::new(SEGMENT_ID, Vec::new(), Vec::new(), 0, 0)
                .request("/api/catalog", "unknown=value"),
            BAD_REQUEST,
            "bad_parameter",
            Some("unknown"),
        ),
        (
            ReportSession::new(SEGMENT_ID, Vec::new(), Vec::new(), 0, 0)
                .request("/api/row-detail", "detail_ref=not%2Bbase64"),
            BAD_REQUEST,
            "bad_locator",
            None,
        ),
        (
            ReportSession::new(SEGMENT_ID, b"not a ZMS".to_vec(), Vec::new(), 0, 9)
                .request("/api/catalog", ""),
            INTERNAL_SERVER_ERROR,
            "unreadable",
            None,
        ),
    ];

    for (mut response, status, code, parameter) in cases {
        assert_eq!(response.status(), status);
        assert_eq!(response.code().as_deref(), Some(code));
        assert_eq!(response.parameter().as_deref(), parameter);
        assert!(response.message().is_some());
        assert!(response.take_body().is_empty());
    }
}
