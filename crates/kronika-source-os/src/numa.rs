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
mod tests {
    use super::{node_id_from_dir, parse_node_meminfo};

    const NODE0: &str = "\
Node 0 MemTotal:       33554432 kB
Node 0 MemFree:         1048576 kB
Node 0 MemUsed:        32505856 kB
Node 0 FilePages:      16777216 kB
Node 0 Dirty:               128 kB
Node 0 AnonPages:       8388608 kB
Node 0 Slab:             524288 kB
Node 0 SReclaimable:     262144 kB
Node 0 SUnreclaim:       262144 kB
Node 0 HugePages_Total:      0
Node 0 HugePages_Free:       0
";

    #[test]
    fn reads_the_documented_keys() {
        let row = parse_node_meminfo(NODE0, 0, 5, 0).expect("MemTotal is present");
        assert_eq!(row.ts.0, 5);
        assert_eq!(row.node_id, 0);
        assert_eq!(row.mem_total, 33_554_432);
        assert_eq!(row.mem_free, Some(1_048_576));
        assert_eq!(row.file_pages, Some(16_777_216));
        assert_eq!(row.dirty, Some(128));
        assert_eq!(row.s_reclaimable, Some(262_144));
        assert_eq!(row.huge_pages_total, Some(0));
    }

    #[test]
    fn keys_this_build_does_not_read_stay_null() {
        let row = parse_node_meminfo(NODE0, 0, 1, 0).expect("row");
        assert_eq!(row.writeback, None);
        assert_eq!(row.mapped, None);
        assert_eq!(row.shmem, None);
        assert_eq!(row.anon_huge_pages, None);
    }

    #[test]
    fn a_node_without_memtotal_is_not_a_row() {
        assert!(parse_node_meminfo("Node 0 MemFree: 100 kB\n", 0, 1, 0).is_none());
        assert!(parse_node_meminfo("", 0, 1, 0).is_none());
    }

    #[test]
    fn node_directories_resolve_to_their_index() {
        assert_eq!(node_id_from_dir("node0"), Some(0));
        assert_eq!(node_id_from_dir("node13"), Some(13));
        assert_eq!(node_id_from_dir("nodelist"), None);
        assert_eq!(node_id_from_dir("cpu0"), None);
    }
}
