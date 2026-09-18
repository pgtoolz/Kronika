use super::{parse_interrupts, parse_softirqs};

const INTERRUPTS: &str = "\
           CPU0       CPU1
  0:         31          0  IR-IO-APIC    2-edge      timer
  9:          0          0  IR-IO-APIC    9-fasteoi   acpi
 24:      12000      13000  IR-PCI-MSIX-0000:00:1f.6    0-edge      eno1
NMI:          7          8  Non-maskable interrupts
ERR:          0
";

const SOFTIRQS: &str = "\
                    CPU0       CPU1
          HI:          0          1
       TIMER:     100000     110000
      NET_RX:       5000       6000
       BLOCK:        700        800
";

#[test]
fn sums_per_cpu_counts_and_keeps_the_device_text() {
    let rows = parse_interrupts(INTERRUPTS, 2);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].irq, "0");
    assert_eq!(rows[0].count, 31);
    assert_eq!(
        rows[0].device.as_deref(),
        Some("IR-IO-APIC    2-edge      timer")
    );
    assert_eq!(rows[2].irq, "24");
    assert_eq!(rows[2].count, 25_000);
}

#[test]
fn a_synthetic_line_keeps_its_description_and_a_bare_one_has_none() {
    let rows = parse_interrupts(INTERRUPTS, 2);
    let nmi = rows.iter().find(|row| row.irq == "NMI").expect("NMI line");
    assert_eq!(nmi.count, 15);
    assert_eq!(nmi.device.as_deref(), Some("Non-maskable interrupts"));
    let err = rows.iter().find(|row| row.irq == "ERR").expect("ERR line");
    assert_eq!(err.count, 0);
    assert_eq!(err.device, None);
}

#[test]
fn a_line_without_counters_is_skipped() {
    let rows = parse_interrupts("  CPU0\nMIS: not-a-number\n", 1);
    assert!(rows.is_empty());
}

#[test]
fn the_header_line_is_never_a_row() {
    let rows = parse_interrupts("           CPU0       CPU1\n", 2);
    assert!(rows.is_empty());
}

#[test]
fn softirq_vectors_sum_across_cpus() {
    let rows = parse_softirqs(SOFTIRQS);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].vector, "HI");
    assert_eq!(rows[0].count, 1);
    assert_eq!(rows[1].vector, "TIMER");
    assert_eq!(rows[1].count, 210_000);
}

#[test]
fn softirqs_without_a_body_yield_nothing() {
    assert!(parse_softirqs("").is_empty());
    assert!(parse_softirqs("                CPU0\n").is_empty());
}
