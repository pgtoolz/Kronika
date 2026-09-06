mod dump;
mod fixture;
mod given;
mod pg_session;
mod then_demo;
mod then_dump;
mod then_log;
mod then_rows;
mod then_segment;
mod when;

use anyhow::{Context as _, Result, bail};
use cucumber::gherkin::Step;

fn table_rows(step: &Step, header: &[&str]) -> Result<Vec<Vec<String>>> {
    let table = step.table.as_ref().context("the step needs a table")?;
    let (first, rest) = table.rows.split_first().context("the table is empty")?;
    let trimmed: Vec<&str> = first.iter().map(|cell| cell.trim()).collect();
    if trimmed != header {
        bail!("table header is {trimmed:?}, the step reads {header:?}");
    }
    Ok(rest
        .iter()
        .map(|row| row.iter().map(|cell| cell.trim().to_owned()).collect())
        .collect())
}

fn type_id(cell: &str) -> Result<u32> {
    cell.parse()
        .with_context(|| format!("{cell:?} is not a type_id"))
}

fn count(cell: &str) -> Result<u32> {
    cell.parse()
        .with_context(|| format!("{cell:?} is not a count"))
}
