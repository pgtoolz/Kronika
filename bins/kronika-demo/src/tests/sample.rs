use super::{cpu_ticks, peak_rss_bytes};

const STATUS: &str = "\
Name:\tkronika-collect
State:\tS (sleeping)
VmPeak:\t  200000 kB
VmSize:\t  180000 kB
VmHWM:\t   12000 kB
VmRSS:\t   11000 kB
";

// 52 fields; utime = 120 and stime = 30 sit at positions 14 and 15.
const STAT: &str = "\
4242 (kronika (test) x) S 1 4242 4242 0 -1 4194560 500 0 0 0 120 30 0 0 20 0 5 0 900 \
180000000 11000 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";

#[test]
fn peak_rss_comes_from_vmhwm_in_bytes() {
    assert_eq!(peak_rss_bytes(STATUS), Some(12_000 * 1024));
}

#[test]
fn a_status_without_vmhwm_reads_as_unmeasured() {
    assert_eq!(peak_rss_bytes("Name:\tsh\nState:\tS\n"), None);
    assert_eq!(peak_rss_bytes(""), None);
}

#[test]
fn cpu_ticks_sum_user_and_system_past_a_comm_with_spaces_and_parens() {
    assert_eq!(cpu_ticks(STAT), Some(150));
}

#[test]
fn a_truncated_stat_line_reads_as_unmeasured() {
    assert_eq!(cpu_ticks("4242 (sh) S 1 2 3"), None);
    assert_eq!(cpu_ticks("no parenthesis here"), None);
}
