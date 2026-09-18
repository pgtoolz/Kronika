//! Published file layout, section inventory, and segment rotation assertions.

use super::dump::{paths, sections, segments};
use super::{count, table_rows, type_id};
use crate::BddWorld;
use crate::collector::files_under;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;
use std::path::Path;

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

#[then(regex = r"^at least (\d+) segments were published$")]
fn several_segments(world: &mut BddWorld, least: usize) -> Result<()> {
    let published = segments(world)?.len();
    anyhow::ensure!(
        published >= least,
        "the run published {published} segments, fewer than {least}"
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

#[then("age publications preserve their windows and coalesce after the first")]
fn age_window_count(world: &mut BddWorld) -> Result<()> {
    use crate::collector::field_value;
    let listed = segments(world)?;
    for segment in &listed {
        let count = segment
            .number("windows")
            .context("a segment without windows")?;
        let min = segment
            .number("min_ts")
            .context("a segment without min_ts")?;
        let max = segment
            .number("max_ts")
            .context("a segment without max_ts")?;
        anyhow::ensure!(count >= 1 && max >= min, "invalid segment: {segment:?}");
    }
    let log = world
        .run
        .as_ref()
        .context("a collector was started")?
        .log()?;
    let closes: Vec<_> = log
        .lines()
        .filter(|line| {
            line.split_whitespace()
                .any(|field| field == "action=segment_write_finish")
        })
        .collect();
    anyhow::ensure!(
        closes.len() >= 2,
        "need two age publications to observe a complete cycle; log:\n{log}"
    );
    let published = listed
        .iter()
        .filter(|segment| {
            segment
                .get("path")
                .is_some_and(|path| Path::new(&path).extension().is_some_and(|ext| ext == "zms"))
        })
        .count();
    anyhow::ensure!(
        published == closes.len(),
        "{published} ZMS files but {} close records",
        closes.len()
    );
    let mut coalesced = false;
    for (index, close) in closes.iter().enumerate() {
        anyhow::ensure!(
            field_value(close, "reason")? == "age",
            "unexpected close: {close}"
        );
        let filename = format!("{}.zms", field_value(close, "segment_id")?);
        let segment = listed
            .iter()
            .find(|segment| {
                segment.get("path").is_some_and(|path| {
                    Path::new(&path).file_name() == Some(std::ffi::OsStr::new(&filename))
                })
            })
            .with_context(|| format!("no listed ZMS for {close}"))?;
        let count = segment.number("windows").context("missing windows")?;
        let min = segment.number("min_ts").context("missing min_ts")?;
        let max = segment.number("max_ts").context("missing max_ts")?;
        anyhow::ensure!(
            count == field_value(close, "journal_parts")?.parse::<i64>()?,
            "window count differs from WAL at close: {close}"
        );
        anyhow::ensure!(
            min == field_value(close, "min_ts")?.parse::<i64>()?
                && max == field_value(close, "max_ts")?.parse::<i64>()?,
            "timestamp bounds differ from close: {close}"
        );
        coalesced |= index > 0 && count > 1 && max > min;
    }
    anyhow::ensure!(
        coalesced,
        "no post-first age publication coalesced distinct windows"
    );
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
