//! Assertions against the files the collector published.
//!
//! Everything a segment is asked comes back through `kronika-dump`. A scenario
//! that checked a segment some other way would be checking a path nobody runs.

use super::dump::{Line, dump, lines, strict_lines};
use super::{count, table_rows, type_id};
use crate::BddWorld;
use crate::collector::files_under;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;
use std::path::Path;

/// `type_id` of `instance_metadata`.
const INSTANCE_METADATA: u32 = 1_021_002;
/// `type_id` of `os_cgroup_cpu`.
const OS_CGROUP_CPU: u32 = 1_201_001;

/// A `YYYY/MM/DD` prefix, as the layout writes it.
fn is_utc_calendar_path(relative: &Path) -> bool {
    let parts: Vec<&str> = relative
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect();
    let [year, month, day, _file] = parts.as_slice() else {
        return false;
    };
    year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && [year, month, day]
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// A listing allowed to contain no segment or scan warnings.
///
/// Range and set-aside scenarios exercise exactly those outcomes. Ordinary
/// listing assertions go through `listing` instead.
fn permissive_listing(world: &BddWorld, range: &[&str]) -> Result<Vec<Line>> {
    let mut flags = vec!["--json"];
    flags.extend_from_slice(range);
    lines(&dump(world, &flags)?)
}

/// A complete listing with at least one admitted segment and no scan warning.
fn listing(world: &BddWorld) -> Result<Vec<Line>> {
    let printed = dump(world, &["--json"])?;
    let listed = strict_lines(&printed, "the full listing")?;
    anyhow::ensure!(
        listed.iter().any(|line| line.holds("kind", "segment")),
        "the full listing admitted no segments"
    );
    Ok(listed)
}

/// One line per published segment: its window, its span, what it cost.
fn segments(world: &BddWorld) -> Result<Vec<Line>> {
    Ok(listing(world)?
        .into_iter()
        .filter(|line| line.holds("kind", "segment"))
        .collect())
}

/// Segments selected by a range, including an intentionally empty result.
fn segments_in_range(world: &BddWorld, range: &[&str]) -> Result<Vec<Line>> {
    Ok(permissive_listing(world, range)?
        .into_iter()
        .filter(|line| line.holds("kind", "segment"))
        .collect())
}

/// One line per section of every published segment.
fn sections(world: &BddWorld) -> Result<Vec<Line>> {
    Ok(listing(world)?
        .into_iter()
        .filter(|line| line.holds("kind", "section"))
        .collect())
}

/// The rows of one section across every published segment.
pub(super) fn rows(world: &BddWorld, type_id: u32) -> Result<Vec<Line>> {
    let printed = dump(
        world,
        &["--json", "--limit", "0", "--section", &type_id.to_string()],
    )?;
    let listed = strict_lines(&printed, "the row listing")?;
    Ok(listed
        .into_iter()
        .filter(|line| {
            line.holds("kind", "row") && line.number("type_id") == Some(i64::from(type_id))
        })
        .collect())
}

/// The window every published segment covers, oldest first.
fn windows(world: &BddWorld) -> Result<Vec<(i64, i64)>> {
    segments(world)?
        .iter()
        .map(|line| {
            let min_ts = line.number("min_ts").context("a segment without min_ts")?;
            let max_ts = line.number("max_ts").context("a segment without max_ts")?;
            Ok((min_ts, max_ts))
        })
        .collect()
}

#[then(regex = r"^at least (\d+) segments were published$")]
fn several_segments(world: &mut BddWorld, least: usize) -> Result<()> {
    let published = segments(world)?.len();
    anyhow::ensure!(
        published >= least,
        "the run published {published} segments, fewer than {least}"
    );
    Ok(())
}

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

#[then("a segment exists under a YYYY/MM/DD directory")]
fn segment_on_calendar_path(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    let files = files_under(&run.storage_dir());
    anyhow::ensure!(
        files
            .iter()
            .any(|path| path.extension().is_some_and(|ext| ext == "zms")
                && is_utc_calendar_path(path)),
        "no segment on a UTC calendar path in {files:?}"
    );
    Ok(())
}

#[then("every published segment file ends in .zms")]
fn segments_are_zms(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    for path in files_under(&run.storage_dir()) {
        if is_utc_calendar_path(&path) {
            anyhow::ensure!(
                path.extension().and_then(std::ffi::OsStr::to_str) == Some("zms"),
                "{} is on the calendar tree but is not a segment",
                path.display()
            );
        }
    }
    Ok(())
}

#[then("the raw journal is named active.wal")]
fn journal_is_active_wal(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    anyhow::ensure!(
        run.storage_dir().join("active.wal").exists(),
        "active.wal is missing from {:?}",
        files_under(&run.storage_dir())
    );
    Ok(())
}

#[then("no segment exists under a YYYY/MM/DD directory")]
fn no_segment_on_calendar_path(world: &mut BddWorld) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    let files = files_under(&run.storage_dir());
    anyhow::ensure!(
        files
            .iter()
            .all(|path| path.extension().is_none_or(|ext| ext != "zms")),
        "unexpected segment under the data root: {files:?}"
    );
    Ok(())
}

#[then("every segment holds these sections")]
fn every_segment_holds_sections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "section", "min rows"])?;
    let sections = sections(world)?;
    for path in paths(&segments(world)?) {
        for row in &wanted {
            let [id, name, min] = row.as_slice() else {
                anyhow::bail!("a section row needs a type_id, a name and a row floor, got {row:?}");
            };
            let id = type_id(id)?;
            let held = sections
                .iter()
                .find(|line| {
                    line.holds("path", &path) && line.number("type_id") == Some(i64::from(id))
                })
                .with_context(|| format!("{path} carries no {name} ({id})"))?;
            let rows = held.number("rows").unwrap_or_default();
            anyhow::ensure!(
                rows >= i64::from(count(min)?),
                "{path} holds {rows} rows of {name} ({id}), fewer than {min}"
            );
        }
    }
    Ok(())
}

#[then("no segment holds these sections")]
fn no_segment_holds_sections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let unwanted = table_rows(step, &["type_id", "section"])?;
    let sections = sections(world)?;
    for row in &unwanted {
        let [id, name] = row.as_slice() else {
            anyhow::bail!("a section row needs a type_id and a name, got {row:?}");
        };
        let id = type_id(id)?;
        anyhow::ensure!(
            !sections
                .iter()
                .any(|line| line.number("type_id") == Some(i64::from(id))),
            "a segment carries {name} ({id}); the source was unreadable, so the section \
             belongs nowhere in the segment"
        );
    }
    Ok(())
}

#[then("some segment holds these sections")]
fn some_segment_holds_sections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let wanted = table_rows(step, &["type_id", "section", "min rows"])?;
    let sections = sections(world)?;
    for row in &wanted {
        let [id, name, min] = row.as_slice() else {
            anyhow::bail!("a section row needs a type_id, a name and a row floor, got {row:?}");
        };
        let id = type_id(id)?;
        let floor = i64::from(count(min)?);
        anyhow::ensure!(
            sections.iter().any(|line| {
                line.number("type_id") == Some(i64::from(id))
                    && line.number("rows").unwrap_or_default() >= floor
            }),
            "no segment holds {min} rows of {name} ({id})"
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

#[then(regex = r"^every segment covers at least (\d+) windows$")]
fn window_count(world: &mut BddWorld, least: i64) -> Result<()> {
    for line in segments(world)? {
        let windows = line
            .number("windows")
            .context("a segment without windows")?;
        anyhow::ensure!(
            windows >= least,
            "a segment coalesced {windows} windows, fewer than {least}"
        );
    }
    Ok(())
}

#[then("every segment ends later than it starts")]
fn segment_time_span(world: &mut BddWorld) -> Result<()> {
    for (min_ts, max_ts) in windows(world)? {
        anyhow::ensure!(
            max_ts > min_ts,
            "a segment spans {min_ts}..{max_ts}, which is not a span"
        );
    }
    Ok(())
}

#[then(regex = r"^its peak RSS stays under (\d+) MiB$")]
fn peak_rss(world: &mut BddWorld, maximum_mib: u64) -> Result<()> {
    let run = world.run.as_ref().context("a collector was started")?;
    let peak = run
        .peak_rss_kib()
        .context("the run recorded no peak RSS; it may not have been stopped")?;
    let limit_kib = maximum_mib * 1024;
    anyhow::ensure!(
        peak <= limit_kib,
        "peak RSS was {peak} KiB, above the {limit_kib} KiB the scenario allows"
    );
    Ok(())
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

/// The path of every segment a listing named.
fn paths(segments: &[Line]) -> Vec<String> {
    segments
        .iter()
        .filter_map(|line| line.get("path"))
        .collect()
}
