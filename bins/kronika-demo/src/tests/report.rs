use super::Report;
use crate::sections::SectionRows;

fn report() -> Report {
    Report {
        duration_s: 60,
        segments: 4,
        segment_bytes: 400_000,
        journal_bytes: 1_024,
        peak_rss_bytes: 12_000_000,
        cpu_ms: 1_200,
        sections: vec![SectionRows {
            type_id: 2_001_001,
            name: "pg_log_errors",
            rows: 7,
        }],
    }
}

#[test]
fn the_mean_segment_is_the_total_over_the_count() {
    assert_eq!(report().mean_segment_bytes(), 100_000);
}

#[test]
fn a_run_that_finished_nothing_reports_a_zero_mean_rather_than_dividing() {
    let empty = Report {
        segments: 0,
        segment_bytes: 0,
        ..report()
    };
    assert_eq!(empty.mean_segment_bytes(), 0);
}

#[test]
fn cpu_percent_is_of_one_core_over_the_wall_clock() {
    // 1.2 s of CPU over 60 s is 2.00 % of one core.
    assert_eq!(report().cpu_centipercent_of_one_core(), 200);
    let instant = Report {
        duration_s: 0,
        ..report()
    };
    assert_eq!(instant.cpu_centipercent_of_one_core(), 0);
}

#[test]
fn json_carries_every_measured_field() {
    let json = report().to_json();
    for key in [
        "duration_s",
        "segments",
        "segment_bytes",
        "mean_segment_bytes",
        "journal_bytes",
        "peak_rss_bytes",
        "cpu_ms",
    ] {
        assert!(json.contains(key), "{key} missing from {json}");
    }
    assert!(
        json.contains(r#"{"type_id":2001001,"name":"pg_log_errors","rows":7}"#),
        "the section totals are missing from {json}"
    );
}

#[test]
fn the_summary_names_every_measured_field() {
    let text = report().render();
    assert!(text.contains("segments        4"));
    assert!(text.contains("peak_rss_bytes  12000000"));
    assert!(text.contains("cpu_percent     2.00"));
    assert!(text.contains("section         2001001 pg_log_errors 7 rows"));
}
