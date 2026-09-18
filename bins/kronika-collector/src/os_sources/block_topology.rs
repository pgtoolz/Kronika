use std::collections::HashSet;
use std::time::Instant;

use kronika_source_os::{SysFs, block_topology};

use super::OsSources;
use super::io::log_degraded;
use crate::logging::log_collection_finish;

/// Record the exact sysfs edges; inside a container (`kept` present) only the
/// chains under the container's own devices.
pub(super) fn collect_block_topology(
    sys: &SysFs,
    scope: u8,
    ts: i64,
    kept: Option<&HashSet<(i32, i32)>>,
    os: &mut OsSources,
) {
    let started = Instant::now();
    let rows = match block_topology::collect_sections(sys, scope, ts, kept) {
        Ok(edges) => edges,
        Err(error) => {
            log_degraded(1_123_001, "sysfs/dev/block", &error);
            return;
        }
    };
    os.block_topology = rows;
    if !os.block_topology.is_empty() {
        log_collection_finish(
            1_123_001,
            "sysfs",
            os.block_topology.len(),
            started.elapsed(),
        );
    }
}
