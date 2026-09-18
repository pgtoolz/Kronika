use super::parse_cpu;

const SAMPLE: &str = "cpu  100 20 30 400 5 6 7 8 9 10\n\
                      cpu0 50 10 15 200 2 3 3 4 4 5\n\
                      cpu1 50 10 15 200 3 3 4 4 5 5\n\
                      intr 999\nctxt 12345\n";

#[test]
fn parses_aggregate_and_per_cpu() {
    let rows = parse_cpu(SAMPLE, 1_700).expect("parse");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].cpu_id, -1);
    assert_eq!(rows[0].user, 100);
    assert_eq!(rows[1].cpu_id, 0);
    assert_eq!(rows[2].cpu_id, 1);
    assert_eq!(rows[0].guest_nice, 10);
}

#[test]
fn old_kernel_missing_trailing_fields_default_to_zero() {
    let rows = parse_cpu("cpu 100 20 30 400 5 6 7\n", 1).expect("parse");
    assert_eq!(rows[0].steal, 0);
    assert_eq!(rows[0].guest, 0);
}

#[test]
fn garbled_cpu_line_errors() {
    assert!(parse_cpu("cpu notanumber 2 3\n", 1).is_err());
}

#[test]
fn non_cpu_line_starting_with_cpu_is_skipped_not_an_error() {
    // "cpufreq" must not cause a parse error; only cpu/cpuN lines are parsed.
    let input = "cpufreq 100\ncpu 10 20 30 40 5 6 7 8 9 10\n";
    let rows = parse_cpu(input, 1).expect("parse");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cpu_id, -1);
}

use super::{parse_stat_misc, parse_uptime};

const STAT_SAMPLE: &str = "cpu  100 20 30 400 5 6 7 8 9 10\n\
                           cpu0 50 10 15 200 2 3 3 4 4 5\n\
                           intr 999\n\
                           ctxt 12345\n\
                           btime 1700000000\n\
                           processes 42\n\
                           procs_running 3\n\
                           procs_blocked 1\n\
                           softirq 100 0 50 0 10 0 0 0 10 0 30\n";

#[test]
fn parses_all_five_misc_fields() {
    let row = parse_stat_misc(STAT_SAMPLE, 9_999).expect("parse");
    assert_eq!(row.ts, 9_999);
    assert_eq!(row.ctxt, 12_345);
    assert_eq!(row.btime, 1_700_000_000_000_000);
    assert_eq!(row.processes, 42);
    assert_eq!(row.procs_running, 3);
    assert_eq!(row.procs_blocked, 1);
}

#[test]
fn missing_btime_is_an_error() {
    let no_btime = "ctxt 1\nprocesses 2\nprocs_running 1\nprocs_blocked 0\n";
    assert!(parse_stat_misc(no_btime, 1).is_err());
}

#[test]
fn missing_any_required_field_is_an_error() {
    let no_ctxt = "btime 1700000000\nprocesses 2\nprocs_running 1\nprocs_blocked 0\n";
    assert!(parse_stat_misc(no_ctxt, 1).is_err());
}

#[test]
fn btime_overflow_is_an_error() {
    // i64::MAX / 1_000_000 + 1 overflows microseconds.
    let overflow =
        "ctxt 1\nbtime 9223372036854776\nprocesses 2\nprocs_running 1\nprocs_blocked 0\n";
    assert!(parse_stat_misc(overflow, 1).is_err());
}

#[test]
fn to_section_carries_every_floor_field_and_scope() {
    let section = parse_stat_misc(STAT_SAMPLE, 9_999)
        .expect("parse")
        .to_section(0);
    assert_eq!(section.ts.0, 9_999);
    assert_eq!(section.ctxt, 12_345);
    assert_eq!(section.btime.0, 1_700_000_000_000_000);
    assert_eq!(section.processes, 42);
    assert_eq!(section.procs_running, 3);
    assert_eq!(section.procs_blocked, 1);
    assert_eq!(section.scope, 0);
}

#[test]
fn interrupt_totals_come_from_the_first_token_of_their_line() {
    let content = "ctxt 1\nbtime 2\nprocesses 3\nprocs_running 0\nprocs_blocked 0\n\
intr 900 1 2 3 4\nsoftirq 800 5 6 7\n";
    let row = parse_stat_misc(content, 1).expect("parse");
    assert_eq!(row.intr_total, Some(900));
    assert_eq!(row.softirq_total, Some(800));
}

#[test]
fn a_kernel_without_interrupt_lines_leaves_the_totals_null() {
    let bare = "ctxt 1\nbtime 2\nprocesses 3\nprocs_running 0\nprocs_blocked 0\n";
    let row = parse_stat_misc(bare, 1).expect("parse");
    assert_eq!(row.intr_total, None);
    assert_eq!(row.softirq_total, None);
}

#[test]
fn the_sample_totals_match_its_intr_and_softirq_lines() {
    let row = parse_stat_misc(STAT_SAMPLE, 1).expect("parse");
    assert_eq!(row.intr_total, Some(999));
    assert_eq!(row.softirq_total, Some(100));
}

#[test]
fn uptime_is_exact_at_hundredth_second_resolution() {
    assert_eq!(
        parse_uptime("3600.25 28000.50\n"),
        Some((3_600_250_000, 28_000_500_000))
    );
    assert_eq!(parse_uptime("0.00 0.00\n"), Some((0, 0)));
    assert_eq!(parse_uptime("12 34\n"), Some((12_000_000, 34_000_000)));
}

#[test]
fn uptime_rejects_a_short_or_garbled_file() {
    assert_eq!(parse_uptime(""), None);
    assert_eq!(parse_uptime("3600.25\n"), None);
    assert_eq!(parse_uptime("nope 1.0\n"), None);
    assert_eq!(parse_uptime("-1.0 2.0\n"), None);
}
