//! Time-window selection and corrupt-segment rejection through kronika-dump.

use super::dump::{permissive_listing, segments_in_range, windows};
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::then;

#[then("reading each segment's own window returns that segment")]
fn window_returns_its_segment(world: &mut BddWorld) -> Result<()> {
    for (min_ts, max_ts) in windows(world)? {
        let found = segments_in_range(
            world,
            &["--from", &min_ts.to_string(), "--to", &max_ts.to_string()],
        )?;
        anyhow::ensure!(
            found
                .iter()
                .any(|line| line.number("min_ts") == Some(min_ts)
                    && line.number("max_ts") == Some(max_ts)),
            "reading {min_ts}..{max_ts} returned {} segments, none of them that one",
            found.len()
        );
    }
    Ok(())
}

#[then("reading the first segment's window leaves out the last segment")]
fn window_excludes_the_others(world: &mut BddWorld) -> Result<()> {
    let windows = windows(world)?;
    let first = *windows.first().context("no segment was published")?;
    let last = *windows.last().context("no segment was published")?;
    let found = segments_in_range(
        world,
        &["--from", &first.0.to_string(), "--to", &first.1.to_string()],
    )?;
    anyhow::ensure!(
        !found
            .iter()
            .any(|line| line.number("min_ts") == Some(last.0)),
        "reading {}..{} returned the segment starting at {}",
        first.0,
        first.1,
        last.0
    );
    Ok(())
}

#[then("reading the time before the first segment returns nothing")]
fn nothing_before_the_first(world: &mut BddWorld) -> Result<()> {
    let first = windows(world)?
        .first()
        .copied()
        .context("no segment was published")?
        .0;
    let found = segments_in_range(world, &["--to", &(first - 1).to_string()])?;
    anyhow::ensure!(
        found.is_empty(),
        "reading up to {} returned {} segments",
        first - 1,
        found.len()
    );
    Ok(())
}

#[then("reading the time after the last segment returns nothing")]
fn nothing_after_the_last(world: &mut BddWorld) -> Result<()> {
    let last = windows(world)?
        .last()
        .copied()
        .context("no segment was published")?
        .1;
    let found = segments_in_range(world, &["--from", &(last + 1).to_string()])?;
    anyhow::ensure!(
        found.is_empty(),
        "reading from {} returned {} segments",
        last + 1,
        found.len()
    );
    Ok(())
}

#[then(regex = r"^the reader sets aside (\d+) files?$")]
fn reader_sets_aside(world: &mut BddWorld, expected: usize) -> Result<()> {
    let listed = permissive_listing(world, &[])?;
    let warnings = listed
        .iter()
        .filter(|line| line.holds("kind", "warning"))
        .count();
    anyhow::ensure!(
        warnings == expected,
        "the scan set aside {warnings} files, not {expected}",
    );
    Ok(())
}
