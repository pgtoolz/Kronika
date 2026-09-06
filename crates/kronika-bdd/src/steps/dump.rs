//! Running the dumper and reading what it printed.
//!
//! Assertions reach a segment through the shipped binary rather than through
//! the reader, so a scenario checks the thing an operator runs.

use crate::BddWorld;
use crate::services::run;
use anyhow::{Context as _, Result};
use std::path::PathBuf;
use std::process::Command;

/// The dumper under test, found the way the collector is.
fn binary() -> Result<PathBuf> {
    let here = std::env::current_exe().context("locate the BDD binary")?;
    Ok(here.with_file_name("kronika-dump"))
}

/// Run the dumper over the run's data root and return what it printed.
pub(crate) fn dump(world: &BddWorld, flags: &[&str]) -> Result<String> {
    let collector = world.run.as_ref().context("a collector was started")?;
    let root = collector.storage_dir();
    let mut command = Command::new(binary()?);
    command.arg(&root).args(flags);
    run(&mut command).with_context(|| format!("run the dumper over {}", root.display()))
}

/// One line of `--json` output.
#[derive(Debug, Clone)]
pub(crate) struct Line(serde_json::Value);

impl Line {
    /// The value of `field` as text, or `None` when the line lacks it.
    ///
    /// A number reads as the digits it was printed with, so a scenario can
    /// compare against what a `.feature` table holds without knowing the type.
    pub(crate) fn get(&self, field: &str) -> Option<String> {
        self.0.get(field).map(value_as_text)
    }

    /// The value of `field` inside a section row's `row` object.
    pub(crate) fn row_get(&self, field: &str) -> Option<String> {
        self.0.get("row")?.get(field).map(value_as_text)
    }

    /// Whether the line carries top-level `field` equal to `value`.
    pub(crate) fn holds(&self, field: &str, value: &str) -> bool {
        self.get(field).as_deref() == Some(value)
    }

    /// Whether a section row carries `field` equal to `value`.
    pub(crate) fn row_holds(&self, field: &str, value: &str) -> bool {
        self.row_get(field).as_deref() == Some(value)
    }

    /// A top-level numeric field.
    pub(crate) fn number(&self, field: &str) -> Option<i64> {
        self.0.get(field)?.as_i64()
    }

    /// A numeric field inside a section row's `row` object.
    pub(crate) fn row_number(&self, field: &str) -> Option<i64> {
        self.0.get("row")?.get(field)?.as_i64()
    }
}

fn value_as_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

/// Parse the dumper's `--json` output, one object per line.
pub(crate) fn lines(printed: &str) -> Result<Vec<Line>> {
    printed
        .lines()
        .enumerate()
        .map(|(line_number, line)| {
            serde_json::from_str(line)
                .map(Line)
                .with_context(|| format!("parse dumper JSON line {}", line_number + 1))
        })
        .collect()
}

/// Parse a listing and reject every scan warning.
pub(crate) fn strict_lines(printed: &str, description: &str) -> Result<Vec<Line>> {
    let listed = lines(printed)?;
    let warnings = listed
        .iter()
        .filter(|line| line.holds("kind", "warning"))
        .count();
    anyhow::ensure!(warnings == 0, "{description} reported {warnings} warnings");
    Ok(listed)
}

#[cfg(test)]
mod tests;
