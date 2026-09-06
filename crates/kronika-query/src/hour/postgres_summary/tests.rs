use std::collections::BTreeMap;

use kronika_reader::{Cell, Row};

use super::facts::{Previous, Summary};
use super::{FIELDS, fold_moments, layout};

#[test]
fn fixed_layout_keeps_surface_before_population_facts() {
    let layout = layout();
    let columns = layout["columns"].as_array().expect("summary columns");
    assert_eq!(columns.len(), FIELDS.split_ascii_whitespace().count() + 1);
    assert_eq!(columns[0]["name"], "surface");
    assert_eq!(columns.last().expect("last fact")["name"], "usable_pct");
}

#[test]
fn integer_counters_are_subtracted_before_float_conversion() {
    let base = 9_007_199_254_740_992_i64;
    let before = row(1_002_002, &[("calls", Cell::I64(base))]);
    let current = row(1_002_002, &[("calls", Cell::I64(base + 1))]);
    let mut summary = Summary::new(1);
    summary.add(&current, Some(&Previous::new(1, &before)));
    assert_eq!(summary.values(1)[0], Some(1.0));
}

#[test]
fn unavailable_identity_does_not_blank_available_population_values() {
    let before = row(
        1_002_002,
        &[
            ("calls", Cell::I64(10)),
            ("total_exec_time", Cell::F64(100.0)),
        ],
    );
    let established = row(
        1_002_002,
        &[
            ("calls", Cell::I64(12)),
            ("total_exec_time", Cell::F64(110.0)),
        ],
    );
    let new = row(
        1_002_002,
        &[("calls", Cell::I64(1)), ("total_exec_time", Cell::F64(2.0))],
    );
    let mut summary = Summary::new(1);
    summary.add(&established, Some(&Previous::new(1, &before)));
    summary.add(&new, None);
    let values = summary.values(1);
    assert_eq!(values[0], Some(1.0));
    assert_eq!(values[1], Some(100.0));
    assert_eq!(values[2], Some(5.0));
}

#[test]
fn ratios_ignore_identity_rows_missing_one_side() {
    let before = row(
        1_002_002,
        &[
            ("calls", Cell::I64(0)),
            ("shared_blks_read", Cell::I64(0)),
            ("local_blks_read", Cell::I64(0)),
        ],
    );
    let current = row(
        1_002_002,
        &[
            ("calls", Cell::I64(10)),
            ("shared_blks_read", Cell::I64(10)),
            ("local_blks_read", Cell::I64(0)),
        ],
    );
    let other_before = row(
        1_002_002,
        &[
            ("shared_blks_read", Cell::I64(0)),
            ("shared_blks_hit", Cell::I64(0)),
            ("local_blks_read", Cell::I64(0)),
            ("local_blks_hit", Cell::I64(0)),
        ],
    );
    let other = row(
        1_002_002,
        &[
            ("shared_blks_read", Cell::I64(1)),
            ("shared_blks_hit", Cell::I64(9)),
            ("local_blks_read", Cell::I64(0)),
            ("local_blks_hit", Cell::I64(0)),
            ("total_exec_time", Cell::F64(20.0)),
            ("wal_bytes", Cell::I64(30)),
        ],
    );
    let mut summary = Summary::new(1);
    summary.add(&current, Some(&Previous::new(1, &before)));
    summary.add(&other, Some(&Previous::new(1, &other_before)));
    let values = summary.values(1);
    assert_eq!(values[2], None);
    assert_eq!(values[3], Some(10.0));
    assert_eq!(values[4], None);
}

#[test]
fn relation_shares_count_only_rows_with_their_required_inputs() {
    let table_before = row(
        1_013_008,
        &[
            ("vacuum_count", Cell::I64(0)),
            ("autovacuum_count", Cell::I64(0)),
        ],
    );
    let table_current = row(
        1_013_008,
        &[
            ("vacuum_count", Cell::I64(1)),
            ("autovacuum_count", Cell::I64(0)),
        ],
    );
    let mut tables = Summary::new(4);
    tables.add(&table_current, Some(&Previous::new(4, &table_before)));
    tables.add(&row(1_013_008, &[]), None);
    assert_eq!(tables.values(4)[11], Some(100.0));

    let index_before = row(1_014_004, &[("idx_scan", Cell::I64(0))]);
    let index_current = row(
        1_014_004,
        &[
            ("idx_scan", Cell::I64(1)),
            ("indisvalid", Cell::Bool(true)),
            ("indisready", Cell::Bool(true)),
        ],
    );
    let mut indexes = Summary::new(5);
    indexes.add(&index_current, Some(&Previous::new(5, &index_before)));
    indexes.add(&row(1_014_004, &[]), None);
    let values = indexes.values(5);
    assert_eq!(values[14], Some(100.0));
    assert_eq!(values[15], Some(0.0));
    assert_eq!(values[16], Some(100.0));
}

#[test]
fn relation_population_folds_in_datid_order() {
    let moments = BTreeMap::from([
        ((100, 1), (1, table_summary(10_000_000_000_000_000, 0))),
        ((100, 2), (1, table_summary(1, 1))),
        ((100, 3), (1, table_summary(1, 1))),
    ]);
    let point = fold_moments(4, moments).pop().expect("table point");
    assert_eq!(point.3[12], Some(1.999_999_999_999_999_4e-14));
}

fn table_summary(main_bytes: i64, toast_bytes: i64) -> Summary {
    let current = row(
        1_013_008,
        &[
            ("main_fork_bytes", Cell::I64(main_bytes)),
            ("toast_bytes", Cell::I64(toast_bytes)),
        ],
    );
    let mut summary = Summary::new(4);
    summary.add(&current, None);
    summary
}

fn row(type_id: u32, values: &[(&str, Cell)]) -> Row {
    let contract = kronika_registry::contract(type_id).expect("fixture contract");
    let cells = contract
        .columns
        .iter()
        .map(|column| {
            values
                .iter()
                .find_map(|(name, value)| (*name == column.name).then(|| value.clone()))
                .unwrap_or(Cell::Null)
        })
        .collect();
    Row::new(contract, cells)
}
