//! Assertions about decoded section rows.

use super::dump::{Line, paths, rows, segments};
use super::{table_rows, type_id};
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;
use std::collections::BTreeMap;

/// Physical layouts emitted by the collector in these scenarios.
const INSTANCE_METADATA: u32 = 1_021_004;
const OS_CGROUP_CPU: u32 = 1_201_003;

#[then(regex = r"^every snapshot of section (\d+) contains exactly these rows$")]
fn exact_snapshot_rows(world: &mut BddWorld, id: u32, step: &Step) -> Result<()> {
    let table = step.table.as_ref().context("the step needs a table")?;
    let header = table.rows.first().context("the table is empty")?;
    let columns: Vec<&str> = header.iter().map(|column| column.trim()).collect();
    let mut expected = table_rows(step, &columns)?;
    anyhow::ensure!(
        !columns.is_empty() && !expected.is_empty(),
        "expected rows are empty"
    );
    for row in &expected {
        anyhow::ensure!(
            row.len() == columns.len(),
            "row {row:?} does not match {columns:?}"
        );
    }
    expected.sort();
    let mut snapshots = BTreeMap::<(String, i64), Vec<Vec<String>>>::new();
    for row in rows(world, id)? {
        let path = row.get("path").context("a stored row has a segment path")?;
        let ts = row
            .row_number("ts")
            .context("a stored row has a timestamp")?;
        let values = columns
            .iter()
            .map(|column| {
                row.row_get(column)
                    .with_context(|| format!("{path} section {id} has no column {column}"))
            })
            .collect::<Result<Vec<_>>>()?;
        snapshots.entry((path, ts)).or_default().push(values);
    }
    anyhow::ensure!(!snapshots.is_empty(), "no stored snapshots of section {id}");
    for ((path, ts), mut recorded) in snapshots {
        recorded.sort();
        anyhow::ensure!(
            recorded == expected,
            "{path} section {id} at {ts}, columns {columns:?}: expected {expected:?}, got {recorded:?}"
        );
    }
    Ok(())
}

#[then("no segment records these rows")]
fn excluded_rows(world: &mut BddWorld, step: &Step) -> Result<()> {
    for expected in table_rows(step, &["type_id", "column", "value"])? {
        let [id, column, value] = expected.as_slice() else {
            anyhow::bail!("an excluded row needs a type_id, column and value, got {expected:?}");
        };
        let recorded = rows(world, type_id(id)?)?;
        anyhow::ensure!(
            !recorded.iter().any(|row| row.row_holds(column, value)),
            "a segment records excluded {column}={value} in section {id}"
        );
    }
    Ok(())
}

#[then("every segment records these instance facts")]
fn instance_facts(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["column", "value"])?;
    let recorded = rows(world, INSTANCE_METADATA)?;
    let published = paths(&segments(world)?);
    anyhow::ensure!(
        recorded.len() == published.len(),
        "{} instance_metadata rows across {} segments, expected one each",
        recorded.len(),
        published.len()
    );
    for path in published {
        let per_segment: Vec<&Line> = recorded
            .iter()
            .filter(|row| row.holds("path", &path))
            .collect();
        anyhow::ensure!(
            per_segment.len() == 1,
            "{path} has {} instance_metadata rows, expected one",
            per_segment.len()
        );
        let row = per_segment[0];
        for expected in &wanted {
            let [column, value] = expected.as_slice() else {
                anyhow::bail!("an instance-fact row needs a column and a value, got {expected:?}");
            };
            let held = row
                .row_get(column)
                .with_context(|| format!("{path} instance_metadata has no column {column}"))?;
            anyhow::ensure!(
                held == *value,
                "{path} records {column}={held}, not {value}"
            );
        }
    }
    Ok(())
}

#[then(regex = r"^some segment records a cgroup CPU limit of (\d+) cores$")]
fn cgroup_cpu_limit(world: &mut BddWorld, cores: i64) -> Result<()> {
    let mut seen = Vec::new();
    for row in rows(world, OS_CGROUP_CPU)? {
        let (Some(quota), Some(period)) =
            (row.row_number("quota_usec"), row.row_number("period_usec"))
        else {
            continue;
        };
        if period > 0 && quota > 0 {
            seen.push(quota / period);
            if quota == cores * period {
                return Ok(());
            }
        }
    }
    anyhow::bail!("no cgroup records a {cores}-core quota; the limits found were {seen:?}")
}

#[then("some segment records these log events")]
#[then("some segment records these rows")]
fn log_events_recorded(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "column", "value"])?;
    for expected in &wanted {
        let [id, column, value] = expected.as_slice() else {
            anyhow::bail!(
                "a log-event row needs a type_id, a column and a value, got {expected:?}"
            );
        };
        let recorded = rows(world, type_id(id)?)?;
        anyhow::ensure!(
            recorded.iter().any(|row| row.row_holds(column, value)),
            "no segment records {column}={value} in {id}; the segments hold {:?}",
            seen_values(&recorded, column)
        );
    }
    Ok(())
}

#[then("some segment records these log events exactly once")]
fn log_events_recorded_once(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "column", "value"])?;
    for expected in &wanted {
        let [id, column, value] = expected.as_slice() else {
            anyhow::bail!(
                "a log-event row needs a type_id, a column and a value, got {expected:?}"
            );
        };
        let recorded = rows(world, type_id(id)?)?;
        let seen = recorded
            .iter()
            .filter(|row| row.row_holds(column, value))
            .count();
        anyhow::ensure!(
            seen == 1,
            "{id} records {column}={value} {seen} times, not once"
        );
    }
    Ok(())
}

/// Every value one column holds, for a failure message that says what is there
/// instead of only what is not.
fn seen_values(rows: &[Line], column: &str) -> Vec<String> {
    let mut seen: Vec<String> = rows.iter().filter_map(|row| row.row_get(column)).collect();
    seen.sort();
    seen.dedup();
    seen
}
