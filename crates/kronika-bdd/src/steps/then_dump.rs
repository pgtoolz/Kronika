//! Assertions on what the dumper prints and on the index it builds.

use super::dump::{Line, dump, strict_lines};
use super::table_rows;
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;

#[then("the dumper reports these sections with a size")]
fn dumper_reports_sections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["section"])?;
    let printed = dump(world, &[])?;
    for row in &wanted {
        let [name] = row.as_slice() else {
            anyhow::bail!("a section row needs one name, got {row:?}");
        };
        let named = printed
            .lines()
            .filter(|line| line.contains(name.as_str()))
            .any(|line| line.contains("bytes="));
        anyhow::ensure!(
            named,
            "the dumper printed no size for {name} in:\n{printed}"
        );
    }
    Ok(())
}

#[then("the dumper prints every section byte count as a share of the segment")]
fn dumper_reports_shares(world: &mut BddWorld) -> Result<()> {
    let printed = dump(world, &[])?;
    let shares = printed
        .lines()
        .filter(|line| line.trim_end().ends_with('%'));
    anyhow::ensure!(
        shares.count() > 0,
        "the dumper printed no shares in:\n{printed}"
    );
    Ok(())
}

#[then(regex = r"^the dumper prints the rows of section (\d+)$")]
fn dumper_prints_rows(world: &mut BddWorld, type_id: u32) -> Result<()> {
    let printed = dump(world, &["--section", &type_id.to_string(), "--limit", "0"])?;
    anyhow::ensure!(
        printed.lines().any(|line| line.contains("ts=")),
        "the dumper printed no rows of {type_id} in:\n{printed}"
    );
    Ok(())
}

#[then(regex = r"^column (\w+) of section (\d+) starts with (\S+) rather than an id$")]
fn dumper_resolves_dictionary(
    world: &mut BddWorld,
    column: String,
    type_id: u32,
    prefix: String,
) -> Result<()> {
    let printed = dump(world, &["--section", &type_id.to_string(), "--limit", "0"])?;
    let resolved = printed
        .lines()
        .filter_map(|line| line.split(&format!("{column}=")).nth(1))
        .filter_map(|tail| tail.split(' ').next())
        .any(|value| value.starts_with(&prefix));
    anyhow::ensure!(
        resolved && !printed.contains("<str "),
        "the dumper left {column} of {type_id} unresolved in:\n{printed}"
    );
    Ok(())
}

fn index_points(world: &BddWorld) -> Result<Vec<Line>> {
    let printed = dump(world, &["--index", "--json"])?;
    Ok(strict_lines(&printed, "the index listing")?
        .into_iter()
        .filter(|line| line.holds("kind", "point"))
        .collect())
}

#[then("the dumper builds one OS health point per pressure snapshot")]
fn dumper_builds_points(world: &mut BddWorld) -> Result<()> {
    let printed = dump(world, &["--section", "1107001", "--limit", "0", "--json"])?;
    let section_lines = strict_lines(&printed, "the PSI row listing")?;
    let snapshots: std::collections::BTreeSet<i64> = section_lines
        .iter()
        .filter(|row| row.holds("kind", "row"))
        .filter_map(|row| row.row_number("ts"))
        .collect();
    anyhow::ensure!(!snapshots.is_empty(), "the dumper printed no PSI snapshots");

    let points = index_points(world)?
        .into_iter()
        .filter(|point| point.holds("series", "os_health"))
        .count();
    anyhow::ensure!(points > 0, "the dumper built no OS health points");
    anyhow::ensure!(
        points == snapshots.len(),
        "the dumper built {points} OS health points for {} pressure snapshots",
        snapshots.len()
    );
    Ok(())
}

#[then("every health series point is null or between 0 and 100")]
fn dumper_points_are_in_range(world: &mut BddWorld) -> Result<()> {
    let points = index_points(world)?
        .into_iter()
        .filter(|point| {
            point.holds("series", "os_health")
                || point.holds("series", "overall_health")
                || point.holds("series", "postgres_health")
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(!points.is_empty(), "the dumper built no health points");
    for point in points {
        let value = point
            .get("value")
            .context("an index point without a value field")?;
        if value == "null" {
            continue;
        }
        let health: u32 = value
            .parse()
            .with_context(|| format!("{value:?} is not a health value"))?;
        anyhow::ensure!(health <= 100, "health {health} is above 100");
    }
    Ok(())
}
