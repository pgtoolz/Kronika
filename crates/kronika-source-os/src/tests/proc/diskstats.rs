use kronika_registry::StrId;

use super::{is_pseudo_device, parse};

#[test]
fn skips_loop_and_ram_devices() {
    let content = "\
   7       0 loop0 10 0 80 4 0 0 0 0 0 4 4\n\
   1       0 ram0 1 0 8 0 0 0 0 0 0 0 0\n\
   8       0 sda 100 2 3000 40 200 5 6000 70 1 800 900\n";
    let rows = parse(content).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].device, "sda");
    assert!(is_pseudo_device(7) && is_pseudo_device(1) && !is_pseudo_device(8));
}

#[test]
fn parses_modern_and_legacy_lines() {
    // modern: 20 fields (with discard+flush); legacy: 14 fields
    let c = "\
   8       0 sda 100 2 3000 40 200 5 6000 70 1 800 900 10 11 12 13 14 15\n\
 259       0 nvme0n1 1 0 8 2 3 0 24 4 0 6 6\n";
    let rows = parse(c).unwrap();
    assert_eq!(rows.len(), 2);
    let sda = &rows[0];
    assert_eq!((sda.major, sda.minor, sda.device.as_str()), (8, 0, "sda"));
    assert_eq!(sda.reads, 100);
    assert_eq!(sda.read_sectors, 3000);
    assert_eq!(sda.io_in_progress, 1);
    assert_eq!(sda.io_weighted_time_ms, 900);
    assert_eq!(sda.discards, Some(10));
    assert_eq!(sda.flushes, Some(14));
    let nvme = &rows[1];
    assert_eq!(nvme.reads, 1);
    assert_eq!(nvme.discards, None); // legacy 14-field line
}

#[test]
fn short_line_is_silently_skipped() {
    // A line with fewer than 14 fields is skipped, not an error.
    let c = "8 0 sda 100 2 3000 40 200 5 6000\n\
                 8 1 sda1 10 0 80 5 20 0 160 3 0 40 45\n";
    let rows = parse(c).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].device, "sda1");
}

#[test]
fn garbled_integer_field_is_an_error() {
    let c = "8 0 sda notanumber 2 3000 40 200 5 6000 70 1 800 900\n";
    assert!(parse(c).is_err());
}

#[test]
fn to_section_carries_every_floor_field_and_scope() {
    let c = "8 0 sda 100 2 3000 40 200 5 6000 70 1 800 900\n";
    let row = &parse(c).unwrap()[0];
    let section = row.to_section(3, 9_999, StrId(5));
    assert_eq!(section.ts.0, 9_999);
    assert_eq!(section.major, 8);
    assert_eq!(section.minor, 0);
    assert_eq!(section.device, StrId(5));
    assert_eq!(section.reads, 100);
    assert_eq!(section.read_sectors, 3000);
    assert_eq!(section.io_in_progress, 1);
    assert_eq!(section.io_weighted_time_ms, 900);
    assert_eq!(section.discards, None);
    assert_eq!(section.flushes, None);
    assert_eq!(section.scope, 3);
}
