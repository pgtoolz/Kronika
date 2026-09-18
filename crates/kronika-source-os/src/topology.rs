//! CPU topology and NUMA memory.

use crate::proc::cpuinfo;
use crate::{CollectionError, ProcFs, SysFs, node_id_from_dir, parse_node_meminfo};
use kronika_registry::{StrId, os_numa::OsNuma, os_topology::OsTopology};

/// Read per-NUMA-node memory from sysfs.
///
/// A machine with one node still gets one row, so a reader never has to guess
/// whether the node breakdown was collected.
#[must_use]
pub fn collect_numa(sys: &SysFs, scope: u8, ts: i64) -> Vec<OsNuma> {
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
    rows
}

/// The NUMA node of one logical CPU, or `-1` when sysfs exposes none.
#[must_use]
pub fn cpu_numa_node(sys: &SysFs, cpu_id: i32) -> i32 {
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
/// Missing enrichment leaves nullable frequency or the unknown NUMA sentinel.
/// A rejected string skips only that CPU's row, preserving input order.
///
/// # Errors
/// Returns a bounded procfs read or parse failure.
pub fn collect_topology(
    fs: &ProcFs,
    sys: &SysFs,
    scope: u8,
    ts: i64,
    mut intern: impl FnMut(&str) -> Option<StrId>,
) -> Result<Vec<OsTopology>, CollectionError> {
    let content = fs.read_raw("cpuinfo")?;
    let mut rows = cpuinfo::parse(&content)?;
    for row in &mut rows {
        row.mhz_max = cpu_max_mhz(sys, row.cpu_id);
        row.numa_node = cpu_numa_node(sys, row.cpu_id);
    }
    let built: Vec<OsTopology> = rows
        .iter()
        .filter_map(|row| {
            let model_name_id = intern(&row.model_name)?;
            Some(row.to_section(scope, ts, model_name_id))
        })
        .collect();
    Ok(built)
}

/// Maximum hardware frequency in MHz, absent for missing or invalid sysfs values.
#[must_use]
pub fn cpu_max_mhz(sys: &SysFs, cpu_id: i32) -> Option<f64> {
    let rel = format!("devices/system/cpu/cpu{cpu_id}/cpufreq/cpuinfo_max_freq");
    let khz = sys.read(&rel).ok()?.parse::<f64>().ok()?;
    (khz.is_finite() && khz >= 0.0).then_some(khz / 1000.0)
}

#[cfg(test)]
#[path = "tests/topology.rs"]
mod tests;
