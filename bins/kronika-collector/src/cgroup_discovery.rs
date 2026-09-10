//! Bounded discovery portions written through the ordinary collection journal.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use kronika_layout::WriterOwner;
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cgroup_cpu::OsCgroupCpuV3;
use kronika_registry::os_cgroup_io::OsCgroupIoV2;
use kronika_registry::os_cgroup_memory::OsCgroupMemoryV3;
use kronika_registry::os_cgroup_pids::OsCgroupPids;
use kronika_registry::os_cgroup_v2_cpu::OsCgroupV2Cpu;
use kronika_registry::os_cgroup_v2_group::OsCgroupV2Group;
use kronika_registry::os_cgroup_v2_io::OsCgroupV2Io;
use kronika_registry::os_cgroup_v2_memory::OsCgroupV2Memory;
use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;
use kronika_registry::{StrId, Ts};
use kronika_source_os::cgroup::discovery::{
    DiscoveredGroup, DiscoveredIo, DiscoveryRow, DiscoveryStats, walk_visible_v2_with_primary,
};
use kronika_source_os::cgroup::{self, AncestorContext};
use kronika_source_os::{OsScope, ProcFs, SysFs};
use kronika_source_pg::settings::SettingsRow;
use kronika_writer::{Interner, Journal, SectionBuffers};

use crate::buffering::buffer_row;
use crate::config::Config;
use crate::logging::{LogLevel, field, log_event, peak_rss_kib};
use crate::scheduler::Scheduler;
use crate::segments::{SegmentState, append_window_and_maybe_close, encode_window};
use crate::service_sections::push_instance_metadata;

const GROUPS_PER_PORTION: usize = 64;
const IO_PER_PORTION: usize = 256;
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
    log_finish(&pass, started.elapsed().as_micros());
    if let Some(error) = &pass.stats.first_error {
        log_error(&io::Error::other(error.clone()));
    }
    Ok(pass)
}

fn log_finish(pass: &CgroupPass, elapsed_us: u128) {
    log_event(
        LogLevel::Info,
        "cgroup_discovery_finish",
        &[
            field("elapsed_us", elapsed_us),
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
            let open_ts = crate::collection_timestamp_after(self.previous_open_ts)
                .context("read cgroup append timestamp")?;
            self.previous_open_ts = Some(open_ts);
            let mut buffers = SectionBuffers::new();
            let fresh = self.segment.is_empty();
            if fresh {
                push_instance_metadata(
                    &mut buffers,
                    self.segment.interner_mut(),
                    self.in_container,
                    self.config,
                    open_ts,
                )?;
            }
            if self.in_container
                && (fresh || self.portion.groups.iter().any(|(_, primary)| *primary))
            {
                let context = context_section(self.segment.interner_mut(), &pass.selected)?;
                buffer_row(&mut buffers, context)?;
            }
            let includes_settings =
                self.segment.needs_pg_settings() && !self.opening_settings.is_empty();
            if includes_settings {
                crate::push_pg_settings(
                    &mut buffers,
                    self.segment.interner_mut(),
                    self.opening_settings,
                )?;
            }
            for (group, primary) in &self.portion.groups {
                push_group(
                    &mut buffers,
                    self.segment.interner_mut(),
                    group,
                    primary.then_some(&pass.selected),
                )?;
            }
            for (row, primary) in &self.portion.io {
                push_io(
                    &mut buffers,
                    self.segment.interner_mut(),
                    row,
                    primary.then_some(&pass.selected),
                )?;
            }
            let flushed = encode_window(buffers, self.segment.interner())?;
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
                crate::announce(&format!("wrote {} reason={reason}", path.display()));
                pass.written.push(path);
            }
            if retry {
                anyhow::ensure!(attempt == 0, "fresh journal rejected a cgroup portion");
                continue;
            }
            if includes_settings && !self.segment.is_empty() {
                self.segment.mark_pg_settings_present();
            }
            pass.appended = true;
            self.portion.clear();
            return Ok(());
        }
        anyhow::bail!("cgroup portion exhausted append attempts")
    }
}

fn intern(interner: &mut Interner, value: &str) -> Result<StrId> {
    interner
        .intern(value.as_bytes())
        .map(|id| StrId(id.get()))
        .map_err(|error| anyhow::anyhow!("intern discovered cgroup: {error}"))
}

pub(crate) fn context_section(
    interner: &mut Interner,
    selected: &AncestorContext,
) -> Result<OsCgroupContextV2> {
    let (path, identity, root) = match &selected.group {
        Some(group) => (
            Some(intern(interner, &group.path)?),
            Some(intern(interner, &group.identity)?),
            Some(intern(interner, &group.root)?),
        ),
        None => (None, None, None),
    };
    Ok(cgroup::to_ancestor_context_section(
        selected,
        [path; 4],
        [identity; 4],
        [root; 4],
    ))
}

fn finite_limit(value: Option<i64>) -> (Option<i64>, Option<bool>) {
    let value = value.filter(|value| *value >= -1);
    (
        value.filter(|value| *value >= 0),
        value.map(|value| value == -1),
    )
}

fn push_group(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    group: &DiscoveredGroup,
    primary: Option<&AncestorContext>,
) -> Result<()> {
    let ts = Ts(group.ts);
    let cgroup_path = intern(interner, &group.cgroup_path)?;
    let cgroup_identity = intern(interner, &group.cgroup_identity)?;
    buffer_row(
        buffers,
        OsCgroupV2Group {
            ts,
            cgroup_path,
            cgroup_identity,
            mount_root: intern(interner, &group.mount_root)?,
            parent_identity: group
                .parent_identity
                .as_deref()
                .map(|value| intern(interner, value))
                .transpose()?,
            memory_localevents: group.memory_localevents,
            pids_localevents: group.pids_localevents,
        },
    )?;
    let cpu = &group.cpu;
    buffer_row(
        buffers,
        OsCgroupV2Cpu {
            ts,
            cgroup_path,
            cgroup_identity,
            usage_usec: cpu.usage_usec,
            user_usec: cpu.user_usec,
            system_usec: cpu.system_usec,
            nr_periods: cpu.nr_periods,
            nr_throttled: cpu.nr_throttled,
            throttled_usec: cpu.throttled_usec,
            quota_usec: cpu.quota_usec,
            period_usec: cpu.period_usec,
            cpuset_cpus: cpu.cpuset_cpus,
        },
    )?;
    let memory = &group.memory;
    let (max, max_unlimited) = finite_limit(memory.max);
    let (high, high_unlimited) = finite_limit(memory.high);
    buffer_row(
        buffers,
        OsCgroupV2Memory {
            ts,
            cgroup_path,
            cgroup_identity,
            current: memory.current,
            max,
            max_unlimited,
            high,
            high_unlimited,
            anon: memory.anon,
            file: memory.file,
            kernel: memory.kernel,
            slab: memory.slab,
            low_events: memory.low_events,
            high_events: memory.high_events,
            max_events: memory.max_events,
            oom_events: memory.oom_events,
            oom_kill: memory.oom_kill,
            local_high_events: memory.local_high_events,
            local_max_events: memory.local_max_events,
            local_oom_events: memory.local_oom_events,
            local_oom_kill: memory.local_oom_kill,
            local_oom_group_kill: memory.local_oom_group_kill,
        },
    )?;
    let pids = &group.pids;
    let (max, max_unlimited) = finite_limit(pids.max);
    buffer_row(
        buffers,
        OsCgroupV2Pids {
            ts,
            cgroup_path,
            cgroup_identity,
            current: pids.current,
            max,
            max_unlimited,
            failure_max: pids.failure_max,
            events_source: pids.events_source,
        },
    )?;
    if let Some(primary) = primary {
        push_primary_group(buffers, interner, group, primary)?;
    }
    Ok(())
}

fn push_primary_group(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    group: &DiscoveredGroup,
    selected: &AncestorContext,
) -> Result<()> {
    let Some(primary) = &selected.group else {
        return Ok(());
    };
    let cgroup_path = intern(interner, &primary.path)?;
    let cgroup_identity = intern(interner, &primary.identity)?;
    let ts = Ts(group.ts);
    let scope = OsScope::Unknown.as_u8();
    let cpu = &group.cpu;
    if let (Some(usage_usec), Some(user_usec), Some(system_usec)) =
        (cpu.usage_usec, cpu.user_usec, cpu.system_usec)
    {
        buffer_row(
            buffers,
            OsCgroupCpuV3 {
                ts,
                cgroup_path,
                cgroup_identity,
                usage_usec,
                user_usec,
                system_usec,
                throttled_usec: cpu.throttled_usec,
                nr_throttled: cpu.nr_throttled,
                quota_usec: cpu.quota_usec,
                period_usec: cpu.period_usec,
                scope,
            },
        )?;
    }
    let memory = &group.memory;
    if let Some(current) = memory.current {
        let (max, max_unlimited) = finite_limit(memory.max);
        buffer_row(
            buffers,
            OsCgroupMemoryV3 {
                ts,
                cgroup_path,
                cgroup_identity,
                current,
                max,
                max_unlimited,
                anon: memory.anon,
                file: memory.file,
                kernel: memory.kernel,
                slab: memory.slab,
                low_events: memory.low_events,
                high_events: memory.high_events,
                max_events: memory.max_events,
                oom_events: memory.oom_events,
                oom_kill: memory.oom_kill,
                scope,
            },
        )?;
    }
    if let (Some(current), Some(max)) = (group.pids.current, group.pids.max) {
        buffer_row(
            buffers,
            OsCgroupPids {
                ts,
                cgroup_path,
                current,
                max: finite_limit(Some(max)).0,
                scope,
            },
        )?;
    }
    Ok(())
}

fn push_io(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    row: &DiscoveredIo,
    primary: Option<&AncestorContext>,
) -> Result<()> {
    buffer_row(
        buffers,
        OsCgroupV2Io {
            ts: Ts(row.ts),
            cgroup_path: intern(interner, &row.cgroup_path)?,
            cgroup_identity: intern(interner, &row.cgroup_identity)?,
            major: row.major,
            minor: row.minor,
            rbytes: row.rbytes,
            wbytes: row.wbytes,
            rios: row.rios,
            wios: row.wios,
        },
    )?;
    if let Some(group) = primary.and_then(|selected| selected.group.as_ref()) {
        buffer_row(
            buffers,
            OsCgroupIoV2 {
                ts: Ts(row.ts),
                cgroup_path: intern(interner, &group.path)?,
                cgroup_identity: intern(interner, &group.identity)?,
                major: row.major,
                minor: row.minor,
                rbytes: row.rbytes,
                wbytes: row.wbytes,
                rios: row.rios,
                wios: row.wios,
                scope: OsScope::Unknown.as_u8(),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
