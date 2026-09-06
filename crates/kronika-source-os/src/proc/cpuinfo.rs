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
mod tests {
    use super::parse;

    const SAMPLE: &str = "\
processor\t: 0
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz
cpu MHz\t\t: 3600.000
physical id\t: 0
core id\t\t: 0

processor\t: 1
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz
cpu MHz\t\t: 3200.500
physical id\t: 0
core id\t\t: 1

";

    const SAMPLE_MISSING_CORE_ID: &str = "\
processor\t: 0
model name\t: AMD EPYC 7742
cpu MHz\t\t: 2245.000
physical id\t: 0

processor\t: 1
model name\t: AMD EPYC 7742
cpu MHz\t\t: 2245.000

";

    #[test]
    fn parses_two_processor_blocks() {
        let rows = parse(SAMPLE).expect("parse");
        assert_eq!(rows.len(), 2);

        assert_eq!(rows[0].cpu_id, 0);
        assert_eq!(
            rows[0].model_name,
            "Intel(R) Core(TM) i7-9700K CPU @ 3.60GHz"
        );
        assert_eq!(rows[0].mhz_max, None);
        assert_eq!(rows[0].core_id, 0);
        assert_eq!(rows[0].socket_id, 0);

        assert_eq!(rows[1].cpu_id, 1);
        assert_eq!(rows[1].mhz_max, None);
        assert_eq!(rows[1].core_id, 1);
        assert_eq!(rows[1].socket_id, 0);
    }

    #[test]
    fn missing_core_id_defaults_to_sentinel() {
        let rows = parse(SAMPLE_MISSING_CORE_ID).expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].core_id, -1);
        assert_eq!(rows[0].socket_id, 0);
        // cpu1 has no physical id either
        assert_eq!(rows[1].core_id, -1);
        assert_eq!(rows[1].socket_id, -1);
    }

    #[test]
    fn cpu_mhz_is_not_treated_as_max_frequency() {
        let content = "processor\t: 0\nmodel name\t: Test CPU\n\n";
        let rows = parse(content).expect("parse");
        assert_eq!(rows[0].mhz_max, None);
    }

    #[test]
    fn no_processor_blocks_is_an_error() {
        assert!(parse("vendor_id\t: GenuineIntel\n").is_err());
        assert!(parse("").is_err());
    }
}
