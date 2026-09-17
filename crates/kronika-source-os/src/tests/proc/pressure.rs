use super::parse_pressure;

const CPU_SAMPLE: &str = "\
some avg10=0.10 avg60=0.05 avg300=0.02 total=10000\n";

const MEMORY_SAMPLE: &str = "\
some avg10=1.50 avg60=0.80 avg300=0.30 total=500000\n\
full avg10=0.20 avg60=0.10 avg300=0.05 total=100000\n";

const IO_SAMPLE: &str = "\
some avg10=0.50 avg60=0.25 avg300=0.10 total=200000\n\
full avg10=0.05 avg60=0.02 avg300=0.01 total=20000\n";

#[test]
fn three_resources_yield_three_rows_cpu_full_is_none() {
    let rows = parse_pressure(
        Some(CPU_SAMPLE),
        Some(MEMORY_SAMPLE),
        Some(IO_SAMPLE),
        1_000,
    )
    .expect("parse");
    assert_eq!(rows.len(), 3);

    let cpu = &rows[0];
    assert_eq!(cpu.resource, 0);
    assert_eq!(cpu.ts, 1_000);
    assert!((cpu.some_avg10 - 0.10).abs() < 1e-9);
    assert!((cpu.some_avg60 - 0.05).abs() < 1e-9);
    assert!((cpu.some_avg300 - 0.02).abs() < 1e-9);
    assert_eq!(cpu.some_total, 10_000);
    assert_eq!(cpu.full_avg10, None);
    assert_eq!(cpu.full_avg60, None);
    assert_eq!(cpu.full_avg300, None);
    assert_eq!(cpu.full_total, None);

    let mem = &rows[1];
    assert_eq!(mem.resource, 1);
    assert!((mem.some_avg10 - 1.50).abs() < 1e-9);
    assert_eq!(mem.some_total, 500_000);
    assert!((mem.full_avg10.unwrap() - 0.20).abs() < 1e-9);
    assert_eq!(mem.full_total, Some(100_000));

    let io = &rows[2];
    assert_eq!(io.resource, 2);
    assert!((io.some_avg10 - 0.50).abs() < 1e-9);
    assert_eq!(io.some_total, 200_000);
    assert!((io.full_avg10.unwrap() - 0.05).abs() < 1e-9);
    assert_eq!(io.full_total, Some(20_000));
}

#[test]
fn only_cpu_yields_one_row() {
    let rows = parse_pressure(Some(CPU_SAMPLE), None, None, 2_000).expect("parse");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].resource, 0);
    assert_eq!(rows[0].full_avg10, None);
    assert_eq!(rows[0].full_total, None);
}

#[test]
fn all_absent_yields_empty_vec() {
    let rows = parse_pressure(None, None, None, 3_000).expect("parse");
    assert!(rows.is_empty());
}

#[test]
fn to_section_carries_all_floor_fields_and_scope() {
    let rows = parse_pressure(
        Some(CPU_SAMPLE),
        Some(MEMORY_SAMPLE),
        Some(IO_SAMPLE),
        9_999,
    )
    .expect("parse");

    let cpu_section = rows[0].to_section(1);
    assert_eq!(cpu_section.ts.0, 9_999);
    assert_eq!(cpu_section.resource, 0);
    assert!((cpu_section.some_avg10 - 0.10).abs() < 1e-9);
    assert!((cpu_section.some_avg60 - 0.05).abs() < 1e-9);
    assert!((cpu_section.some_avg300 - 0.02).abs() < 1e-9);
    assert_eq!(cpu_section.some_total, 10_000);
    assert_eq!(cpu_section.full_avg10, None);
    assert_eq!(cpu_section.full_avg60, None);
    assert_eq!(cpu_section.full_avg300, None);
    assert_eq!(cpu_section.full_total, None);
    assert_eq!(cpu_section.scope, 1);

    let mem_section = rows[1].to_section(0);
    assert_eq!(mem_section.resource, 1);
    assert!((mem_section.full_avg10.unwrap() - 0.20).abs() < 1e-9);
    assert_eq!(mem_section.full_total, Some(100_000));
    assert_eq!(mem_section.scope, 0);
}
