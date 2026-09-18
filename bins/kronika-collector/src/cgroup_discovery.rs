//! Walk visible cgroups and append them to the collection journal in bounded portions.
//!
//! Row conversion lives in `buffering`; this module handles portion limits and
//! rebuilds a portion with the new segment dictionary after a full-WAL retry.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use kronika_layout::WriterOwner;
use kronika_source_os::cgroup::discovery::{
    DiscoveredGroup, DiscoveredIo, DiscoveryRow, DiscoveryStats, walk_visible_v2_with_primary,
};
use kronika_source_os::cgroup::{self, AncestorContext};
use kronika_source_os::{ProcFs, SysFs};
use kronika_source_pg::settings::SettingsRow;
use kronika_writer::{Journal, SectionBuffers};

use crate::buffering::buffer_row;
use crate::config::Config;
use crate::instance_metadata::push_instance_metadata;
use crate::logging::{LogLevel, field, log_event, peak_rss_kib, process_cpu_ticks};
use crate::scheduler::Scheduler;
use crate::segments::{SegmentState, append_window_and_maybe_close, encode_window};

mod buffering;

pub(crate) use buffering::context_section;
use buffering::{push_group, push_io};

// Maximum group observations retained before encoding a portion. Each group
// produces inventory, CPU, memory and PID rows, plus selected-container rows.
const GROUPS_PER_PORTION: usize = 64;
// Device rows have a separate bound: one group can contain many devices.
const IO_PER_PORTION: usize = 256;
// Bound owned path/identity strings while discovery waits for the next append.
// A single observation above this bound is omitted; aggregate overflow flushes
// the current portion before accepting the next observation.
const STRING_BYTES_PER_PORTION: usize = 128 * 1024;

#[derive(Default)]
pub(crate) struct CgroupPass {
    pub written: Vec<PathBuf>,
    pub selected: AncestorContext,
    pub charged_devices: HashSet<(i32, i32)>,
    pub appended: bool,
    pub stats: DiscoveryStats,
    pub peak_groups: usize,
    pub peak_io_rows: usize,
    pub peak_string_bytes: usize,
    pub omitted_rows: usize,
}

#[derive(Default)]
struct Portion {
    groups: Vec<(DiscoveredGroup, bool)>,
    io: Vec<(DiscoveredIo, bool)>,
    string_bytes: usize,
}

impl Portion {
    const fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.io.is_empty()
    }

    fn clear(&mut self) {
        self.groups.clear();
        self.io.clear();
        self.string_bytes = 0;
    }
}

struct Appender<'a> {
    config: &'a Config,
    in_container: bool,
    journal: &'a mut Journal,
    owner: &'a WriterOwner,
    segment: &'a mut SegmentState,
    sched: &'a mut Scheduler,
    opening_settings: &'a [SettingsRow],
    previous_open_ts: Option<i64>,
    portion: Portion,
}

/// Discover once, retaining only the current bounded write portion.
#[allow(
    clippy::too_many_arguments,
    reason = "discovery appends to the collector's existing journal and segment state"
)]
pub(crate) fn run(
    fs: &ProcFs,
    sys: &SysFs,
    config: &Config,
    in_container: bool,
    journal: &mut Journal,
    owner: &WriterOwner,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
    ts: i64,
    opening_settings: &[SettingsRow],
) -> Result<CgroupPass> {
    if !config.mode.collect_os() {
        return Ok(CgroupPass::default());
    }
    let started = Instant::now();
    let cpu_started = process_cpu_ticks();
    let selected = if in_container {
        cgroup::select_ancestor_context(fs, sys, ts).unwrap_or_else(|error| {
            log_error(&error);
            AncestorContext {
                context: cgroup::CgroupContextRow {
                    ts,
                    ..cgroup::CgroupContextRow::default()
                },
                ..AncestorContext::default()
            }
        })
    } else {
        AncestorContext::default()
    };
    let mut pass = CgroupPass {
        selected,
        ..CgroupPass::default()
    };
    let mut appender = Appender {
        config,
        in_container,
        previous_open_ts: journal.segment_id().map(kronika_layout::SegmentId::get),
        journal,
        owner,
        segment,
        sched,
        opening_settings,
        portion: Portion::default(),
    };
    let mut selected_discovery_identity = None::<String>;
    let requested_primary = pass.selected.clone();
    let mut context_received = false;
    let mut write_error = None;
    let result = walk_visible_v2_with_primary(fs, sys, ts, &requested_primary, |row, context| {
        if !context_received {
            pass.selected = context.clone();
            context_received = true;
        }
        let primary = match &row {
            DiscoveryRow::Group(group) => {
                let primary = pass
                    .selected
                    .group
                    .as_ref()
                    .is_some_and(|selected| selected.matches_group(group));
                if primary {
                    selected_discovery_identity = Some(group.cgroup_identity.clone());
                }
                primary
            }
            DiscoveryRow::Io(row) => {
                let primary =
                    selected_discovery_identity.as_deref() == Some(row.cgroup_identity.as_str());
                if primary
                    && let (Ok(major), Ok(minor)) =
                        (i32::try_from(row.major), i32::try_from(row.minor))
                {
                    pass.charged_devices.insert((major, minor));
                }
                primary
            }
        };
        if let Err(error) = appender.accept(&row, primary, &mut pass) {
            let message = error.to_string();
            write_error = Some(error);
            return Err(io::Error::other(message));
        }
        Ok(())
    });
    if let Some(error) = write_error {
        return Err(error);
    }
    match result {
        Ok(stats) => pass.stats = stats,
        Err(error) => log_error(&error),
    }
    appender.flush(&mut pass)?;
    let cpu_ticks = process_cpu_ticks()
        .zip(cpu_started)
        .and_then(|(end, start)| end.checked_sub(start));
    log_finish(&pass, started.elapsed().as_micros(), cpu_ticks);
    if let Some(error) = &pass.stats.first_error {
        log_error(&io::Error::other(error.clone()));
    }
    Ok(pass)
}

fn log_finish(pass: &CgroupPass, elapsed_us: u128, cpu_ticks: Option<u64>) {
    log_event(
        LogLevel::Info,
        "cgroup_discovery_finish",
        &[
            field("elapsed_us", elapsed_us),
            field("cpu_ticks", cpu_ticks),
            field("rss_kib", peak_rss_kib()),
            field("groups", pass.stats.groups),
            field("io_rows", pass.stats.io_rows),
            field("metric_files_read", pass.stats.metric_files_read),
            field("skipped_directories", pass.stats.skipped_directories),
            field("metric_errors", pass.stats.metric_errors),
            field("peak_groups", pass.peak_groups),
            field("peak_io_rows", pass.peak_io_rows),
            field("peak_string_bytes", pass.peak_string_bytes),
            field("omitted_rows", pass.omitted_rows),
        ],
    );
}

fn log_error(error: &io::Error) {
    log_event(
        LogLevel::Warn,
        "cgroup_discovery_degraded",
        &[field("error", error)],
    );
}

impl Appender<'_> {
    fn accept(
        &mut self,
        row: &DiscoveryRow<'_>,
        primary: bool,
        pass: &mut CgroupPass,
    ) -> Result<()> {
        let bytes = match &row {
            DiscoveryRow::Group(group) => {
                group.cgroup_path.len()
                    + group.cgroup_identity.len()
                    + group.mount_root.len()
                    + group.parent_identity.as_ref().map_or(0, String::len)
            }
            DiscoveryRow::Io(row) => row.cgroup_path.len() + row.cgroup_identity.len(),
        };
        if bytes > STRING_BYTES_PER_PORTION {
            pass.omitted_rows += 1;
            log_error(&io::Error::other(format!(
                "cgroup identity strings exceed the per-row encoding bound: {bytes} bytes"
            )));
            return Ok(());
        }
        if self.portion.groups.len() >= GROUPS_PER_PORTION
            || self.portion.io.len() >= IO_PER_PORTION
            || self.portion.string_bytes + bytes > STRING_BYTES_PER_PORTION
        {
            self.flush(pass)?;
        }
        match row {
            DiscoveryRow::Group(group) => self.portion.groups.push(((*group).clone(), primary)),
            DiscoveryRow::Io(row) => self.portion.io.push(((*row).clone(), primary)),
        }
        self.portion.string_bytes += bytes;
        pass.peak_groups = pass.peak_groups.max(self.portion.groups.len());
        pass.peak_io_rows = pass.peak_io_rows.max(self.portion.io.len());
        pass.peak_string_bytes = pass.peak_string_bytes.max(self.portion.string_bytes);
        Ok(())
    }

    fn flush(&mut self, pass: &mut CgroupPass) -> Result<()> {
        if self.portion.is_empty() {
            return Ok(());
        }
        for attempt in 0..2 {
            let open_ts = crate::clock::collection_timestamp_after(self.previous_open_ts)
                .context("read cgroup append timestamp")?;
            self.previous_open_ts = Some(open_ts);
            let mut buffers = SectionBuffers::new();
            let fresh = self.segment.is_empty();
            if fresh {
                push_instance_metadata(
                    &mut buffers,
                    &mut self.segment.interner,
                    self.in_container,
                    self.config,
                    open_ts,
                )?;
            }
            let mut pending_context = None;
            if self.in_container {
                let context = context_section(&mut self.segment.interner, &pass.selected)?;
                if self.segment.cgroup_context.as_ref() != Some(&context) {
                    buffer_row(&mut buffers, context)?;
                    pending_context = Some(context);
                }
            }
            let includes_settings =
                !self.segment.pg_settings_present && !self.opening_settings.is_empty();
            if includes_settings {
                crate::pg_sources::push_pg_settings(
                    &mut buffers,
                    &mut self.segment.interner,
                    self.opening_settings,
                )?;
            }
            for (group, primary) in &self.portion.groups {
                push_group(
                    &mut buffers,
                    &mut self.segment.interner,
                    group,
                    primary.then_some(&pass.selected),
                )?;
            }
            for (row, primary) in &self.portion.io {
                push_io(
                    &mut buffers,
                    &mut self.segment.interner,
                    row,
                    primary.then_some(&pass.selected),
                )?;
            }
            let flushed = encode_window(buffers, &self.segment.interner)?;
            let finished = append_window_and_maybe_close(
                self.journal,
                self.owner,
                self.config,
                self.segment,
                open_ts,
                false,
                &flushed,
            )
            .map_err(|failure| failure.into_parts().1)?;
            let retry = finished.iter().any(|(_, reason)| *reason == "journal-full");
            for (path, reason) in finished {
                self.sched.mark_segment_opened();
                crate::segments::report_written(&path, reason);
                pass.written.push(path);
            }
            if retry {
                anyhow::ensure!(attempt == 0, "fresh journal rejected a cgroup portion");
                continue;
            }
            if includes_settings && !self.segment.is_empty() {
                self.segment.pg_settings_present = true;
            }
            if !self.segment.is_empty()
                && let Some(context) = pending_context
            {
                self.segment.cgroup_context = Some(context);
            }
            pass.appended = true;
            self.portion.clear();
            return Ok(());
        }
        anyhow::bail!("cgroup portion exhausted append attempts")
    }
}

#[cfg(test)]
#[path = "tests/cgroup_discovery/mod.rs"]
mod tests;
