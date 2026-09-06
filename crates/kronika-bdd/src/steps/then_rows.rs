use super::then_segment::rows;
use super::{table_rows, type_id};
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;
use std::collections::BTreeMap;

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
