use super::parse_meminfo;

const FULL_SAMPLE: &str = "\
MemTotal:       16777216 kB\n\
MemFree:         4096000 kB\n\
MemAvailable:    8192000 kB\n\
Buffers:          512000 kB\n\
Cached:          3145728 kB\n\
SwapCached:            0 kB\n\
Active:          6291456 kB\n\
Inactive:        2097152 kB\n\
Dirty:              1024 kB\n\
Writeback:             0 kB\n\
AnonPages:       4194304 kB\n\
Mapped:          1048576 kB\n\
Shmem:             32768 kB\n\
Slab:             524288 kB\n\
SReclaimable:     262144 kB\n\
SUnreclaim:       262144 kB\n\
PageTables:        16384 kB\n\
SwapTotal:       8388608 kB\n\
SwapFree:        8000000 kB\n\
CommitLimit:    12582912 kB\n\
Committed_AS:   10485760 kB\n\
HugePages_Total:       0\n\
HugePages_Free:        0\n\
Hugepagesize:       2048 kB\n";

const SPARSE_SAMPLE: &str = "\
MemTotal:        8388608 kB\n\
MemFree:         1000000 kB\n";

#[test]
fn parses_full_sample() {
    let row = parse_meminfo(FULL_SAMPLE, 9_999).expect("parse");
    assert_eq!(row.ts, 9_999);
    assert_eq!(row.mem_total, 16_777_216);
    assert_eq!(row.mem_free, Some(4_096_000));
    assert_eq!(row.mem_available, Some(8_192_000));
    assert_eq!(row.buffers, Some(512_000));
    assert_eq!(row.cached, Some(3_145_728));
    assert_eq!(row.swap_total, Some(8_388_608));
    assert_eq!(row.swap_free, Some(8_000_000));
    assert_eq!(row.active, Some(6_291_456));
    assert_eq!(row.inactive, Some(2_097_152));
    assert_eq!(row.dirty, Some(1024));
    assert_eq!(row.writeback, Some(0));
    assert_eq!(row.slab, Some(524_288));
    assert_eq!(row.s_reclaimable, Some(262_144));
    assert_eq!(row.s_unreclaim, Some(262_144));
    assert_eq!(row.anon_pages, Some(4_194_304));
    assert_eq!(row.mapped, Some(1_048_576));
    assert_eq!(row.shmem, Some(32_768));
    assert_eq!(row.page_tables, Some(16_384));
    assert_eq!(row.commit_limit, Some(12_582_912));
    assert_eq!(row.committed_as, Some(10_485_760));
    assert_eq!(row.huge_pages_total, Some(0));
    assert_eq!(row.huge_pages_free, Some(0));
    assert_eq!(row.hugepagesize, Some(2048));
}

#[test]
fn missing_optional_keys_yield_none() {
    let row = parse_meminfo(SPARSE_SAMPLE, 1).expect("parse");
    assert_eq!(row.mem_total, 8_388_608);
    assert_eq!(row.mem_free, Some(1_000_000));
    assert_eq!(row.mem_available, None);
    assert_eq!(row.slab, None);
    assert_eq!(row.s_reclaimable, None);
    assert_eq!(row.s_unreclaim, None);
    assert_eq!(row.dirty, None);
    assert_eq!(row.writeback, None);
    assert_eq!(row.huge_pages_total, None);
}

#[test]
fn missing_mem_total_is_an_error() {
    let no_total = "MemFree: 4096 kB\nMemAvailable: 8192 kB\n";
    assert!(parse_meminfo(no_total, 1).is_err());
}

#[test]
fn to_section_carries_all_floor_fields_and_scope() {
    let row = parse_meminfo(FULL_SAMPLE, 9_999).expect("parse");
    let section = row.to_section(1);
    assert_eq!(section.ts.0, 9_999);
    assert_eq!(section.mem_total, 16_777_216);
    assert_eq!(section.mem_free, Some(4_096_000));
    assert_eq!(section.mem_available, Some(8_192_000));
    assert_eq!(section.buffers, Some(512_000));
    assert_eq!(section.cached, Some(3_145_728));
    assert_eq!(section.slab, Some(524_288));
    assert_eq!(section.s_reclaimable, Some(262_144));
    assert_eq!(section.s_unreclaim, Some(262_144));
    assert_eq!(section.swap_total, Some(8_388_608));
    assert_eq!(section.swap_free, Some(8_000_000));
    assert_eq!(section.dirty, Some(1024));
    assert_eq!(section.writeback, Some(0));
    assert_eq!(section.scope, 1);
}
