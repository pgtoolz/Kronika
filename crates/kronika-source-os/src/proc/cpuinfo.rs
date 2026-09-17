//! Parse `/proc/cpuinfo` into per-logical-CPU topology rows.

use kronika_registry::os_topology::OsTopology;
use kronika_registry::{StrId, Ts};

pub use crate::proc::stat::ParseError;

/// One logical CPU's topology facts from `/proc/cpuinfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuinfoRow {
    /// Logical CPU index (`processor` field).
    pub cpu_id: i32,
    /// CPU model string (`model name` field).
    pub model_name: String,
    /// Maximum clock frequency in MHz, filled from sysfs by the collector.
    pub mhz_max: Option<f64>,
    /// Physical core within the socket (`core id`); `-1` when absent.
    pub core_id: i32,
    /// Physical socket (`physical id`); `-1` when absent.
    pub socket_id: i32,
    /// NUMA node, filled from sysfs by the collector; `-1` when unknown.
    pub numa_node: i32,
}

impl CpuinfoRow {
    /// Registry row for `1_113_001` with the given scope and interned model name.
    #[must_use]
    pub const fn to_section(&self, scope: u8, ts: i64, model_name_id: StrId) -> OsTopology {
        OsTopology {
            ts: Ts(ts),
            cpu_id: self.cpu_id,
            model_name: model_name_id,
            mhz_max: self.mhz_max,
            core_id: self.core_id,
            socket_id: self.socket_id,
            numa_node: self.numa_node,
            scope,
        }
    }
}

/// Parse the content of `/proc/cpuinfo` into one [`CpuinfoRow`] per logical CPU.
///
/// Blocks are separated by blank lines; each line is `key\t: value`.
/// Missing numeric topology fields use sentinel defaults:
/// `core_id`/`socket_id` = `-1`. Blocks without a `processor` field are
/// skipped. `cpu MHz` is intentionally ignored because it reports the current
/// clock on many systems, not a topology maximum.
///
/// # Errors
///
/// Returns [`ParseError`] when no processor blocks are found.
pub fn parse(content: &str) -> Result<Vec<CpuinfoRow>, ParseError> {
    let mut rows = Vec::new();

    for block in content.split("\n\n") {
        let mut cpu_id: Option<i32> = None;
        let mut model_name = String::new();
        let mut core_id: i32 = -1;
        let mut socket_id: i32 = -1;

        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();

            match key {
                "processor" => {
                    cpu_id = value.parse::<i32>().ok();
                }
                "model name" => {
                    value.clone_into(&mut model_name);
                }
                "core id" => {
                    core_id = value.parse::<i32>().unwrap_or(-1);
                }
                "physical id" => {
                    socket_id = value.parse::<i32>().unwrap_or(-1);
                }
                _ => {}
            }
        }

        if let Some(id) = cpu_id {
            rows.push(CpuinfoRow {
                cpu_id: id,
                model_name,
                mhz_max: None,
                core_id,
                socket_id,
                numa_node: -1,
            });
        }
    }

    if rows.is_empty() {
        return Err(ParseError(
            "/proc/cpuinfo: no processor blocks found".to_owned(),
        ));
    }
    Ok(rows)
}

#[cfg(test)]
#[path = "../tests/proc/cpuinfo.rs"]
mod tests;
