use super::{parse_pressure, parse_pressure_at};

#[test]
fn cpu_full_is_recorded_only_for_cgroup_and_remains_optional() {
    let some = "some avg10=1.00 avg60=2.00 avg300=3.00 total=40\n";
    let both = format!("{some}full avg10=0.50 avg60=1.50 avg300=2.50 total=20\n");
    let host = parse_pressure(Some(&both), None, None, 1).unwrap();
    assert_eq!(host[0].full_total, None);
    for (text, expected) in [(both.as_str(), Some(20)), (some, None)] {
        let rows = parse_pressure_at(
            Some(text),
            None,
            None,
            1,
            "/visible",
            ["cpu.pressure", "memory.pressure", "io.pressure"],
            true,
        )
        .unwrap();
        assert_eq!(rows[0].full_total, expected);
        assert_eq!(rows[0].some_total, 40);
    }
}
