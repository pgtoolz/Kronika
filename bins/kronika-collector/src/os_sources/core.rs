//! Core CPU, system, memory, load, paging, and pressure metrics.

use std::time::Instant;

use kronika_registry::Section;
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::os_stat::OsStat;
use kronika_source_os::proc::loadavg::parse_loadavg;
use kronika_source_os::proc::meminfo::parse_meminfo;
use kronika_source_os::proc::pressure::parse_pressure;
use kronika_source_os::proc::stat::{ParseError, parse_cpu, parse_stat_misc};
use kronika_source_os::proc::vmstat::parse_vmstat;
use kronika_source_os::{OsScope, ProcFs, SysFs, cgroup};

use super::OsSources;
use super::io::{log_degraded, read_optional_os_file};
use crate::logging::log_collection_finish;

/// Collect the core metrics scheduled by an `OsCore` tick.
pub(super) fn collect_core_metrics(
    fs: &ProcFs,
    sys: &SysFs,
    scope: u8,
    ts: i64,
    in_container: bool,
    selected: Option<&cgroup::AncestorContext>,
    os: &mut OsSources,
) {
    collect_cpu_and_stat(fs, scope, ts, os);
    collect_procfs_row(fs, "meminfo", &mut os.meminfo, |content| {
        parse_meminfo(content, ts).map(|row| row.to_section(scope))
    });
    collect_procfs_row(fs, "loadavg", &mut os.loadavg, |content| {
        parse_loadavg(content, ts).map(|row| row.to_section(scope))
    });
    collect_procfs_row(fs, "vmstat", &mut os.vmstat, |content| {
        parse_vmstat(content, ts).map(|row| row.to_section(scope))
    });
    collect_pressure_rows(fs, sys, scope, ts, in_container, selected, os);
}

/// Read `/proc/stat` once for CPU rows and system counters.
fn collect_cpu_and_stat(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    let cpu_type_id = OsCpu::CONTRACT.type_id.get();
    let stat_type_id = OsStat::CONTRACT.type_id.get();
    let started = Instant::now();
    let content = match fs.read("stat") {
        Ok(content) => content,
        Err(error) => {
            log_degraded(cpu_type_id, "stat", &error);
            log_degraded(stat_type_id, "stat", &error);
            return;
        }
    };

    match parse_cpu(&content, ts) {
        Ok(rows) => {
            let count = rows.len();
            os.cpu = rows.into_iter().map(|row| row.to_section(scope)).collect();
            log_collection_finish(cpu_type_id, "procfs", count, started.elapsed());
        }
        Err(error) => log_degraded(cpu_type_id, "stat", &error),
    }

    // Stat counters are independent of CPU parsing; their timing excludes that work.
    let started = Instant::now();
    match parse_stat_misc(&content, ts) {
        Ok(row) => {
            os.stat = Some(row.to_section(scope));
            log_collection_finish(stat_type_id, "procfs", 1, started.elapsed());
        }
        Err(error) => log_degraded(stat_type_id, "stat", &error),
    }
}

/// Read one required procfs file, replacing its section row only on success.
fn collect_procfs_row<S: Section>(
    fs: &ProcFs,
    source: &'static str,
    output: &mut Option<S>,
    parse: impl FnOnce(&str) -> Result<S, ParseError>,
) {
    let type_id = S::CONTRACT.type_id.get();
    let started = Instant::now();
    let content = match fs.read(source) {
        Ok(content) => content,
        Err(error) => {
            log_degraded(type_id, source, &error);
            return;
        }
    };
    match parse(&content) {
        Ok(row) => {
            *output = Some(row);
            log_collection_finish(type_id, "procfs", 1, started.elapsed());
        }
        Err(error) => log_degraded(type_id, source, &error),
    }
}

/// Host `/proc/pressure` or the selected ancestor cgroup v2 pressure files.
pub(super) fn collect_pressure_rows(
    fs: &ProcFs,
    sys: &SysFs,
    scope: u8,
    ts: i64,
    in_container: bool,
    selected: Option<&cgroup::AncestorContext>,
    os: &mut OsSources,
) {
    let type_id = OsPsi::CONTRACT.type_id.get();
    let started = Instant::now();
    let (source, timing_source, pressure_scope, rows) = if in_container {
        (
            "cgroup/{cpu,memory,io}.pressure",
            "cgroup",
            OsScope::Unknown.as_u8(),
            selected.map_or_else(
                || Ok(Vec::new()),
                |selected| cgroup::collect_ancestor_pressure(sys, selected, ts),
            ),
        )
    } else {
        let psi_cpu = read_optional_os_file(fs, "pressure/cpu", type_id);
        let psi_memory = read_optional_os_file(fs, "pressure/memory", type_id);
        let psi_io = read_optional_os_file(fs, "pressure/io", type_id);
        (
            "pressure/{cpu,memory,io}",
            "procfs",
            scope,
            parse_pressure(
                psi_cpu.as_deref(),
                psi_memory.as_deref(),
                psi_io.as_deref(),
                ts,
            ),
        )
    };

    match rows {
        Ok(rows) if rows.is_empty() => {
            log_degraded(type_id, source, &"no pressure files available");
        }
        Ok(rows) => {
            let count = rows.len();
            os.psi = rows
                .into_iter()
                .map(|row| row.to_section(pressure_scope))
                .collect();
            log_collection_finish(type_id, timing_source, count, started.elapsed());
        }
        Err(error) => log_degraded(type_id, source, &error),
    }
}

#[cfg(test)]
#[path = "../tests/os_sources/core.rs"]
mod tests;
