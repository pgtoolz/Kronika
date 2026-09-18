use super::parse_vmstat;

const FULL_SAMPLE: &str = "\
pgpgin 1000000\n\
pgpgout 2000000\n\
pswpin 0\n\
pswpout 0\n\
pgfault 5000000\n\
pgmajfault 1024\n\
pgsteal_kswapd 512000\n\
pgsteal_direct 4096\n\
pgscan_kswapd 768000\n\
pgscan_direct 8192\n\
oom_kill 0\n";

const SPARSE_SAMPLE: &str = "\
pgpgin 100\n\
pgpgout 200\n\
nr_free_pages 12345\n";

#[test]
fn parses_full_sample() {
    let row = parse_vmstat(FULL_SAMPLE, 9_999).expect("parse");
    assert_eq!(row.ts, 9_999);
    assert_eq!(row.pgpgin, Some(1_000_000));
    assert_eq!(row.pgpgout, Some(2_000_000));
    assert_eq!(row.pswpin, Some(0));
    assert_eq!(row.pswpout, Some(0));
    assert_eq!(row.pgfault, Some(5_000_000));
    assert_eq!(row.pgmajfault, Some(1024));
    assert_eq!(row.pgsteal_kswapd, Some(512_000));
    assert_eq!(row.pgsteal_direct, Some(4096));
    assert_eq!(row.pgscan_kswapd, Some(768_000));
    assert_eq!(row.pgscan_direct, Some(8192));
    assert_eq!(row.oom_kill, Some(0));
}

#[test]
fn missing_oom_kill_yields_none() {
    let row = parse_vmstat(SPARSE_SAMPLE, 1).expect("parse");
    assert_eq!(row.pgpgin, Some(100));
    assert_eq!(row.pgpgout, Some(200));
    assert_eq!(row.pswpin, None);
    assert_eq!(row.pswpout, None);
    assert_eq!(row.pgfault, None);
    assert_eq!(row.pgmajfault, None);
    assert_eq!(row.pgsteal_kswapd, None);
    assert_eq!(row.pgsteal_direct, None);
    assert_eq!(row.pgscan_kswapd, None);
    assert_eq!(row.pgscan_direct, None);
    assert_eq!(row.oom_kill, None);
}

#[test]
fn to_section_carries_all_floor_fields_and_scope() {
    let row = parse_vmstat(FULL_SAMPLE, 9_999).expect("parse");
    let section = row.to_section(1);
    assert_eq!(section.ts.0, 9_999);
    assert_eq!(section.pgpgin, Some(1_000_000));
    assert_eq!(section.pgpgout, Some(2_000_000));
    assert_eq!(section.pswpin, Some(0));
    assert_eq!(section.pswpout, Some(0));
    assert_eq!(section.pgfault, Some(5_000_000));
    assert_eq!(section.pgmajfault, Some(1024));
    assert_eq!(section.pgsteal_kswapd, Some(512_000));
    assert_eq!(section.pgsteal_direct, Some(4096));
    assert_eq!(section.pgscan_kswapd, Some(768_000));
    assert_eq!(section.pgscan_direct, Some(8192));
    assert_eq!(section.oom_kill, Some(0));
    assert_eq!(section.scope, 1);
}
