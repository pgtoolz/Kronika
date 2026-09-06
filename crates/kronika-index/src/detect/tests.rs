use super::direct::{CpuRaw, cpu_busy_at_least_80, crosses_wraparound_age};
use super::{block, finding_layout};
use crate::{Finding, FindingKind, MAX_FINDINGS_PER_BLOCK};

#[test]
fn aggregate_cpu_busy_uses_the_exact_adjacent_counter_share() {
    let before = CpuRaw {
        timestamp: 1,
        counters: [0; 8],
    };
    assert!(cpu_busy_at_least_80(
        before,
        CpuRaw {
            timestamp: 2,
            counters: [40, 0, 40, 10, 10, 0, 0, 0],
        }
    ));
    assert!(!cpu_busy_at_least_80(
        before,
        CpuRaw {
            timestamp: 2,
            counters: [39, 0, 40, 11, 10, 0, 0, 0],
        }
    ));
}

#[test]
fn frozen_xid_age_crosses_at_the_exact_postgresql_failsafe_default() {
    assert!(crosses_wraparound_age(1_600_000_000));
    assert!(!crosses_wraparound_age(1_599_999_999));
}

#[test]
fn statistical_process_and_statement_series_are_not_findings() {
    assert!(!finding_layout(1_100_001));
    for type_id in 1_002_001..=1_002_006 {
        assert!(!finding_layout(type_id));
    }
    assert!(!finding_layout(1_004_001));
    assert!(!finding_layout(1_016_001));
}

#[test]
fn archiver_and_cgroup_memory_layouts_carry_findings() {
    for type_id in [1_008_001, 1_202_001, 1_202_002] {
        assert!(finding_layout(type_id));
    }
}

#[test]
fn temporary_file_rows_are_not_log_event_findings() {
    for type_id in [
        2_001_001, 2_002_001, 2_003_001, 2_004_001, 2_005_002, 2_006_001,
    ] {
        assert!(finding_layout(type_id));
    }
    assert!(!finding_layout(2_007_001));
}

#[test]
fn the_fixed_cap_keeps_timestamp_locator_order_and_reports_omissions() {
    let findings: Vec<_> = (0..=MAX_FINDINGS_PER_BLOCK)
        .rev()
        .map(|ordinal| Finding {
            kind: FindingKind::Event,
            category: Some(0),
            field_ordinal: 0,
            row_ordinal: u32::try_from(ordinal).expect("small test ordinal"),
            timestamp: i64::try_from(ordinal).expect("small test timestamp"),
        })
        .collect();
    let block = block(2_001_001, findings);
    assert_eq!(block.findings.len(), MAX_FINDINGS_PER_BLOCK);
    assert_eq!(block.total_hits, 4_097);
    assert!(block.truncated);
    assert_eq!(block.findings[0].timestamp, 0);
    assert_eq!(
        block.findings.last().map(|finding| finding.timestamp),
        Some(4_095)
    );
}
