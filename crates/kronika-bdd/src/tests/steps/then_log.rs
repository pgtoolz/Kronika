use super::{validate_integer_fields, validate_shutdown_query_summary};

const INTEGER_REQUIREMENTS: &[(&str, &str)] = &[
    ("interval_ms", "positive"),
    ("query_count", "positive"),
    ("rows", "positive"),
    ("application_payload_from_postgres_bytes", "positive"),
    ("application_payload_to_postgres_bytes", "positive"),
    ("batches", "positive"),
    ("encoded_bytes", "positive"),
    ("wal_bytes_appended", "positive"),
    ("peak_rss_kib", "positive"),
    ("query_errors", "nonnegative"),
    ("sink_errors", "nonnegative"),
    ("connect_errors", "nonnegative"),
    ("query_timeouts", "nonnegative"),
    ("connect_timeouts", "nonnegative"),
    ("errors", "nonnegative"),
    ("timeouts", "nonnegative"),
    ("slow_queries", "nonnegative"),
    ("fetch_elapsed_ms_total", "nonnegative"),
    ("fetch_elapsed_ms_max", "nonnegative"),
    ("encode_elapsed_ms_total", "nonnegative"),
    ("append_elapsed_ms_total", "nonnegative"),
];

const SUMMARY: &str = concat!(
    "kronika-collector level=info action=pg_query_summary reason=shutdown ",
    "payload_measure=logical_application_estimate interval_ms=1999 query_count=3 ",
    "query_rate_per_s=1.500000 rows=8 ",
    "application_payload_from_postgres_bytes=3145728 ",
    "application_payload_from_postgres_mib=3.000000 ",
    "application_payload_to_postgres_bytes=1572864 ",
    "application_payload_to_postgres_mib=1.500000 batches=6 query_errors=1 ",
    "sink_errors=1 connect_errors=1 query_timeouts=1 connect_timeouts=1 errors=3 ",
    "timeouts=2 slow_queries=1 fetch_elapsed_ms_total=1021 fetch_elapsed_ms_max=501 ",
    "encode_elapsed_ms_total=21 append_elapsed_ms_total=15 encoded_bytes=900 ",
    "wal_bytes_appended=1080 peak_rss_kib=1234"
);

fn validate(line: &str) -> anyhow::Result<()> {
    validate_integer_fields(line, INTEGER_REQUIREMENTS)?;
    validate_shutdown_query_summary(line, "logical_application_estimate")
}

#[test]
fn full_shutdown_summary_contract_is_accepted() {
    validate(SUMMARY).expect("the summary is internally consistent");
}

#[test]
fn query_rate_allows_only_interval_truncation_and_rounding() {
    let fastest = SUMMARY.replace("query_rate_per_s=1.500000", "query_rate_per_s=1.500750");
    validate(&fastest).expect("the exact logged millisecond is a possible interval");

    for rate in ["1.499999", "1.500751"] {
        let line = SUMMARY.replace(
            "query_rate_per_s=1.500000",
            &format!("query_rate_per_s={rate}"),
        );
        let error = validate(&line).expect_err("a rate outside the truncation interval must fail");
        assert!(
            error.to_string().contains(&line),
            "the failure must include the actual line: {error:#}"
        );
    }
}

#[test]
fn mib_fields_use_production_six_decimal_rounding() {
    let half_tie = SUMMARY
        .replace(
            "application_payload_from_postgres_bytes=3145728",
            "application_payload_from_postgres_bytes=8192",
        )
        .replace(
            "application_payload_from_postgres_mib=3.000000",
            "application_payload_from_postgres_mib=0.007813",
        );
    validate(&half_tie).expect("a half-tie rounds away from zero");

    let wrong = half_tie.replace(
        "application_payload_from_postgres_mib=0.007813",
        "application_payload_from_postgres_mib=0.007812",
    );
    let error = validate(&wrong).expect_err("a differently rounded MiB value must fail");
    assert!(
        error.to_string().contains(&wrong),
        "the failure must include the actual line: {error:#}"
    );
}

#[test]
fn zero_error_timeout_slow_and_timing_counters_are_valid() {
    let line = SUMMARY
        .replace("query_errors=1", "query_errors=0")
        .replace("sink_errors=1", "sink_errors=0")
        .replace("connect_errors=1", "connect_errors=0")
        .replace("query_timeouts=1", "query_timeouts=0")
        .replace("connect_timeouts=1", "connect_timeouts=0")
        .replace("errors=3", "errors=0")
        .replace("timeouts=2", "timeouts=0")
        .replace("slow_queries=1", "slow_queries=0")
        .replace("fetch_elapsed_ms_total=1021", "fetch_elapsed_ms_total=0")
        .replace("fetch_elapsed_ms_max=501", "fetch_elapsed_ms_max=0")
        .replace("encode_elapsed_ms_total=21", "encode_elapsed_ms_total=0")
        .replace("append_elapsed_ms_total=15", "append_elapsed_ms_total=0");

    validate(&line).expect("zero is valid for these counters");
}

#[test]
fn derived_counters_and_fetch_bounds_are_checked() {
    for line in [
        SUMMARY.replace("errors=3", "errors=2"),
        SUMMARY.replace("timeouts=2", "timeouts=1"),
        SUMMARY.replace("fetch_elapsed_ms_max=501", "fetch_elapsed_ms_max=1022"),
    ] {
        let error = validate(&line).expect_err("an inconsistent derived counter must fail");
        assert!(
            error.to_string().contains(&line),
            "the failure must include the actual line: {error:#}"
        );
    }
}

#[test]
fn required_counters_are_nonnegative_and_selected_costs_are_positive() {
    for line in [
        SUMMARY.replace("query_rate_per_s=1.500000", "query_rate_per_s=1.5"),
        SUMMARY.replace("query_errors=1", "query_errors=-1"),
        SUMMARY.replace("batches=6", "batches=0"),
        SUMMARY.replace("encoded_bytes=900", "encoded_bytes=0"),
        SUMMARY.replace("wal_bytes_appended=1080", "wal_bytes_appended=0"),
        SUMMARY.replace("peak_rss_kib=1234", "peak_rss_kib=0"),
    ] {
        let error = validate(&line).expect_err("the malformed or zero required counter must fail");
        assert!(
            error.to_string().contains(&line),
            "the failure must include the actual line: {error:#}"
        );
    }
}

#[test]
fn summary_identity_and_payload_measure_are_required() {
    for line in [
        SUMMARY.replace("action=pg_query_summary", "action=other"),
        SUMMARY.replace("reason=shutdown", "reason=interval"),
        SUMMARY.replace(
            "payload_measure=logical_application_estimate",
            "payload_measure=wire_bytes",
        ),
    ] {
        let error = validate(&line).expect_err("the summary identity contract must fail");
        assert!(
            error.to_string().contains(&line),
            "the failure must include the actual line: {error:#}"
        );
    }
}
