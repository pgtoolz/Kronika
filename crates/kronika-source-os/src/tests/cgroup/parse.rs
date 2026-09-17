use super::*;

#[test]
fn cpu_max_handles_unlimited() {
    assert_eq!(parse_cpu_max("max 100000\n"), (-1, 100_000));
    assert_eq!(parse_cpu_max("200000 100000\n"), (200_000, 100_000));
}

#[test]
fn io_stat_parses_per_device_counters() {
    let rows = parse_io_stat("8:0 rbytes=1 wbytes=2 rios=3 wios=4 dbytes=9\n", 5, "/x");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].major, rows[0].minor), (8, 0));
    assert_eq!(rows[0].rbytes, Some(1));
    assert_eq!(rows[0].wios, Some(4));
}

#[test]
fn io_stat_keeps_a_row_with_partial_counters() {
    let rows = parse_io_stat("8:0 rbytes=1 wbytes=2 rios=broken\n", 5, "/x");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].rbytes, Some(1));
    assert_eq!(rows[0].wbytes, Some(2));
    assert_eq!(rows[0].rios, None);
    assert_eq!(rows[0].wios, None);
}
