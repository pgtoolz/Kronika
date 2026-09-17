use super::parse;

const SAMPLE: &str = "\
processor\t: 0
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz
cpu MHz\t\t: 3600.000
physical id\t: 0
core id\t\t: 0

processor\t: 1
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz
cpu MHz\t\t: 3200.500
physical id\t: 0
core id\t\t: 1

";

const SAMPLE_MISSING_CORE_ID: &str = "\
processor\t: 0
model name\t: AMD EPYC 7742
cpu MHz\t\t: 2245.000
physical id\t: 0

processor\t: 1
model name\t: AMD EPYC 7742
cpu MHz\t\t: 2245.000

";

#[test]
fn parses_two_processor_blocks() {
    let rows = parse(SAMPLE).expect("parse");
    assert_eq!(rows.len(), 2);

    assert_eq!(rows[0].cpu_id, 0);
    assert_eq!(
        rows[0].model_name,
        "Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz"
    );
    assert_eq!(rows[0].mhz_max, None);
    assert_eq!(rows[0].core_id, 0);
    assert_eq!(rows[0].socket_id, 0);

    assert_eq!(rows[1].cpu_id, 1);
    assert_eq!(rows[1].mhz_max, None);
    assert_eq!(rows[1].core_id, 1);
    assert_eq!(rows[1].socket_id, 0);
}

#[test]
fn missing_core_id_defaults_to_sentinel() {
    let rows = parse(SAMPLE_MISSING_CORE_ID).expect("parse");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].core_id, -1);
    assert_eq!(rows[0].socket_id, 0);
    // cpu1 has no physical id either
    assert_eq!(rows[1].core_id, -1);
    assert_eq!(rows[1].socket_id, -1);
}

#[test]
fn cpu_mhz_is_not_treated_as_max_frequency() {
    let content = "processor\t: 0\nmodel name\t: Test CPU\n\n";
    let rows = parse(content).expect("parse");
    assert_eq!(rows[0].mhz_max, None);
}

#[test]
fn no_processor_blocks_is_an_error() {
    assert!(parse("vendor_id\t: GenuineIntel\n").is_err());
    assert!(parse("").is_err());
}
