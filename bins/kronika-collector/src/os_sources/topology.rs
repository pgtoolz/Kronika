//! CPU topology and NUMA memory.

use std::time::Instant;

use kronika_registry::{Section, os_numa::OsNuma, os_topology::OsTopology};
use kronika_source_os::proc::cpuinfo;
use kronika_source_os::{ProcFs, SysFs, node_id_from_dir, parse_node_meminfo};
use kronika_writer::Interner;

use super::io::{intern_str, log_degraded, read_optional_os_file};
use crate::logging::log_collection_finish;

/// Read per-NUMA-node memory from sysfs.
///
/// A machine with one node still gets one row, so a reader never has to guess
/// whether the node breakdown was collected.
pub(super) fn collect_numa(sys: &SysFs, scope: u8, ts: i64) -> Vec<OsNuma> {
    let type_id = OsNuma::CONTRACT.type_id.get();
    let started = Instant::now();
    let Ok(entries) = sys.read_dir("devices/system/node") else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for entry in &entries {
        let Some(node_id) = node_id_from_dir(entry.name.as_str()) else {
            continue;
        };
        let rel = format!("devices/system/node/{}/meminfo", entry.name.as_str());
        let Ok(content) = sys.read(&rel) else {
            continue;
        };
        if let Some(row) = parse_node_meminfo(&content, node_id, ts, scope) {
            rows.push(row);
        }
    }
    if !rows.is_empty() {
        log_collection_finish(type_id, "sysfs", rows.len(), started.elapsed());
    }
    rows
}

/// The NUMA node of one logical CPU, or `-1` when sysfs exposes none.
pub(crate) fn cpu_numa_node(sys: &SysFs, cpu_id: i32) -> i32 {
    let rel = format!("devices/system/cpu/cpu{cpu_id}");
    let Ok(entries) = sys.read_dir(&rel) else {
        return -1;
    };
    entries
        .iter()
        .find_map(|entry| node_id_from_dir(entry.name.as_str()))
        .unwrap_or(-1)
}

/// Read `/proc/cpuinfo` and build one `os_topology` row per logical CPU.
///
/// On read or parse failure the section is skipped and a `collection_degraded`
/// event is logged; zeros are never fabricated.
pub(super) fn collect_topology(
    fs: &ProcFs,
    sys: &SysFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
) -> Vec<OsTopology> {
    let type_id = OsTopology::CONTRACT.type_id.get();
    let started = Instant::now();
    let Some(content) = read_optional_os_file(fs, "cpuinfo", type_id) else {
        return Vec::new();
    };
    let mut rows = match cpuinfo::parse(&content) {
        Ok(rows) => rows,
        Err(err) => {
            log_degraded(type_id, "cpuinfo", &err.0);
            return Vec::new();
        }
    };
    for row in &mut rows {
        row.mhz_max = cpu_max_mhz(sys, row.cpu_id);
        row.numa_node = cpu_numa_node(sys, row.cpu_id);
    }
    let built: Vec<OsTopology> = rows
        .iter()
        .filter_map(|row| {
            let model_name_id = intern_str(interner, type_id, "cpuinfo", &row.model_name)?;
            Some(row.to_section(scope, ts, model_name_id))
        })
        .collect();
    log_collection_finish(type_id, "procfs", built.len(), started.elapsed());
    built
}

pub(crate) fn cpu_max_mhz(sys: &SysFs, cpu_id: i32) -> Option<f64> {
    let rel = format!("devices/system/cpu/cpu{cpu_id}/cpufreq/cpuinfo_max_freq");
    let khz = sys.read(&rel).ok()?.parse::<f64>().ok()?;
    (khz.is_finite() && khz >= 0.0).then_some(khz / 1000.0)
}
