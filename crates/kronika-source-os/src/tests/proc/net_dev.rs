use kronika_registry::StrId;

use super::parse;

#[test]
fn parses_all_sixteen_columns_and_strips_iface_colon() {
    let c = "\
Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n\
  eth0:1000 10 1 2 3 4 5 6 2000 20 7 8 9 10 11 12\n";
    let rows = parse(c).unwrap();
    assert_eq!(rows.len(), 2);
    let eth = rows.iter().find(|r| r.iface == "eth0").unwrap();
    assert_eq!(eth.rx_bytes, 1000);
    assert_eq!(eth.rx_multicast, 6);
    assert_eq!(eth.tx_bytes, 2000);
    assert_eq!(eth.tx_compressed, 12);
}

#[test]
fn includes_loopback_and_skips_short_lines() {
    let c = "\
Inter-|   Receive\n\
    lo: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n\
  bad: 1 2 3\n";
    let rows = parse(c).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].iface, "lo");
}

#[test]
fn garbled_counter_is_an_error() {
    let c = "    lo: notanumber 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n";
    assert!(parse(c).is_err());
}

#[test]
fn to_section_carries_every_field_and_scope() {
    let c = "\
Inter-|header\n\
    lo: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n";
    let row = &parse(c).unwrap()[0];
    let section = row.to_section(2, 9_999, StrId(7));
    assert_eq!(section.ts.0, 9_999);
    assert_eq!(section.iface, StrId(7));
    assert_eq!(section.rx_bytes, 100);
    assert_eq!(section.tx_bytes, 200);
    assert_eq!(section.scope, 2);
}
