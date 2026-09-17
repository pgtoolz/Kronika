//! Parse per-NUMA-node memory from sysfs (`1_117`).

use kronika_registry::Ts;
use kronika_registry::os_numa::OsNuma;

/// Build the `1_117_001` row for one node from its `meminfo` content.
///
/// Every line is `Node <id> <Key>: <value> kB`. `MemTotal` is required: a node
/// directory without it is not a node this build understands, and the caller
/// gets `None` rather than a row of zeros.
#[must_use]
pub fn parse_node_meminfo(content: &str, node_id: i32, ts: i64, scope: u8) -> Option<OsNuma> {
    let mut row = OsNuma {
        ts: Ts(ts),
        node_id,
        mem_total: 0,
        mem_free: None,
        mem_used: None,
        file_pages: None,
        dirty: None,
        writeback: None,
        anon_pages: None,
        mapped: None,
        shmem: None,
        slab: None,
        s_reclaimable: None,
        s_unreclaim: None,
        anon_huge_pages: None,
        huge_pages_total: None,
        huge_pages_free: None,
        scope,
    };
    let mut seen_total = false;

    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        // `Node 0 MemTotal` -> `MemTotal`.
        let Some(key) = key.split_whitespace().nth(2) else {
            continue;
        };
        let Some(value) = value
            .split_whitespace()
            .next()
            .and_then(|token| token.parse::<i64>().ok())
        else {
            continue;
        };
        if key == "MemTotal" {
            row.mem_total = value;
            seen_total = true;
            continue;
        }
        let slot = match key {
            "MemFree" => &mut row.mem_free,
            "MemUsed" => &mut row.mem_used,
            "FilePages" => &mut row.file_pages,
            "Dirty" => &mut row.dirty,
            "Writeback" => &mut row.writeback,
            "AnonPages" => &mut row.anon_pages,
            "Mapped" => &mut row.mapped,
            "Shmem" => &mut row.shmem,
            "Slab" => &mut row.slab,
            "SReclaimable" => &mut row.s_reclaimable,
            "SUnreclaim" => &mut row.s_unreclaim,
            "AnonHugePages" => &mut row.anon_huge_pages,
            "HugePages_Total" => &mut row.huge_pages_total,
            "HugePages_Free" => &mut row.huge_pages_free,
            _ => continue,
        };
        *slot = Some(value);
    }

    seen_total.then_some(row)
}

/// The node index behind a `nodeN` sysfs directory name.
#[must_use]
pub fn node_id_from_dir(name: &str) -> Option<i32> {
    name.strip_prefix("node")?.parse().ok()
}

#[cfg(test)]
#[path = "tests/numa.rs"]
mod tests;
