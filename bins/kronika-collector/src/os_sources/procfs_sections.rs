use std::collections::HashSet;

use super::{
    Instant, Interner, MountEntry, MountStringIds, OsDiskstats, OsInterrupts, OsKernelLimits,
    OsMountinfo, OsNetdev, OsNuma, OsSoftirq, OsSources, OsTopology, ProcFs, SysFs, Ts, cpuinfo,
    diskstats, intern_str, interrupts, is_kernel_tree_mount, is_pseudo_filesystem, kernel_limits,
    log_collection_finish, log_degraded, mount_row, net_dev, net_netstat, net_snmp, net_snmp6, nfs,
    node_id_from_dir, parse_dev_pair, parse_mountinfo, parse_node_meminfo, read_optional_os_file,
};

/// Read and parse `/proc/diskstats`, interning device names into rows.
///
/// `/proc/diskstats` reports the whole node. Inside a container the caller
/// passes the devices the pod is charged for and only those rows are kept.
pub(super) fn collect_diskstats(
    fs: &ProcFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    kept: Option<&HashSet<(i32, i32)>>,
) -> Vec<OsDiskstats> {
    let type_id = 1_108_001_u32;
    let started = Instant::now();
    let Some(content) = read_optional_os_file(fs, "diskstats", type_id) else {
        return Vec::new();
    };
    let mut rows = match diskstats::parse(&content) {
        Ok(rows) => rows,
        Err(err) => {
            log_degraded(type_id, "diskstats", &err.0);
            return Vec::new();
        }
    };

    if let Some(kept) = kept {
        rows.retain(|row| kept.contains(&(row.major, row.minor)));
    }

    let built: Vec<OsDiskstats> = rows
        .iter()
        .filter_map(|row| {
            let device = intern_str(interner, type_id, "diskstats", &row.device)?;
            Some(row.to_section(scope, ts, device))
        })
        .collect();
    log_collection_finish(type_id, "procfs", built.len(), started.elapsed());
    built
}

/// Read and parse `/proc/net/dev`, interning interface names into rows.
pub(super) fn collect_netdev(
    fs: &ProcFs,
    sys: &SysFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
) -> Vec<OsNetdev> {
    let type_id = 1_109_001_u32;
    let started = Instant::now();
    let Some(content) = read_optional_os_file(fs, "net/dev", type_id) else {
        return Vec::new();
    };
    let mut rows = match net_dev::parse(&content) {
        Ok(rows) => rows,
        Err(err) => {
            log_degraded(type_id, "net/dev", &err.0);
            return Vec::new();
        }
    };
    for row in &mut rows {
        (row.speed_mbit, row.duplex) = net_link_facts(sys, &row.iface);
    }
    let built: Vec<OsNetdev> = rows
        .iter()
        .filter_map(|row| {
            let iface = intern_str(interner, type_id, "net/dev", &row.iface)?;
            Some(row.to_section(scope, ts, iface))
        })
        .collect();
    log_collection_finish(type_id, "procfs", built.len(), started.elapsed());
    built
}

/// Read the two singleton network counter files into `os`.
pub(super) fn collect_net_singletons(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    let snmp_type_id = 1_110_001_u32;
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "net/snmp", snmp_type_id) {
        match net_snmp::parse(&content) {
            Ok(row) => {
                os.snmp = Some(row.to_section(scope, ts));
                log_collection_finish(snmp_type_id, "procfs", 1, started.elapsed());
            }
            Err(err) => log_degraded(snmp_type_id, "net/snmp", &err.0),
        }
    }

    let netstat_type_id = 1_111_001_u32;
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "net/netstat", netstat_type_id) {
        match net_netstat::parse(&content) {
            Ok(row) => {
                os.netstat = Some(row.to_section(scope, ts));
                log_collection_finish(netstat_type_id, "procfs", 1, started.elapsed());
            }
            Err(err) => log_degraded(netstat_type_id, "net/netstat", &err.0),
        }
    }
}

/// Negotiated speed and duplex of one interface, from sysfs.
///
/// The kernel returns `EINVAL` for a virtual or down interface, so an absent
/// or unparsable value leaves the speed null and the duplex unknown rather
/// than claiming a link that is not there.
pub(crate) fn net_link_facts(sys: &SysFs, iface: &str) -> (Option<i64>, u8) {
    let speed = sys
        .read(&format!("class/net/{iface}/speed"))
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|mbit| *mbit > 0);
    let duplex = match sys
        .read(&format!("class/net/{iface}/duplex"))
        .as_deref()
        .map(str::trim)
    {
        Ok("half") => 1,
        Ok("full") => 2,
        _ => 0,
    };
    (speed, duplex)
}

/// Read the interrupt, softirq, kernel-limit, IPv6, and NFS singletons.
pub(super) fn collect_kernel_singletons(
    fs: &ProcFs,
    interner: &mut Interner,
    scope: u8,
    net_scope: u8,
    ts: i64,
    cpu_count: usize,
    os: &mut OsSources,
) {
    let irq_type_id = 1_114_001_u32;
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "interrupts", irq_type_id) {
        let parsed = interrupts::parse_interrupts(&content, cpu_count);
        os.interrupts = parsed
            .iter()
            .filter_map(|row| {
                Some(OsInterrupts {
                    ts: Ts(ts),
                    irq: intern_str(interner, irq_type_id, "interrupts", &row.irq)?,
                    device: match row.device.as_deref() {
                        Some(text) => Some(intern_str(interner, irq_type_id, "interrupts", text)?),
                        None => None,
                    },
                    count: row.count,
                    scope,
                })
            })
            .collect();
        log_collection_finish(
            irq_type_id,
            "procfs",
            os.interrupts.len(),
            started.elapsed(),
        );
    }

    let softirq_type_id = 1_115_001_u32;
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "softirqs", softirq_type_id) {
        os.softirq = interrupts::parse_softirqs(&content)
            .iter()
            .filter_map(|row| {
                Some(OsSoftirq {
                    ts: Ts(ts),
                    vector: intern_str(interner, softirq_type_id, "softirqs", &row.vector)?,
                    count: row.count,
                    scope,
                })
            })
            .collect();
        log_collection_finish(
            softirq_type_id,
            "procfs",
            os.softirq.len(),
            started.elapsed(),
        );
    }

    let limits_type_id = 1_116_001_u32;
    let started = Instant::now();
    let file_nr = read_optional_os_file(fs, "sys/fs/file-nr", limits_type_id);
    let inode_nr = read_optional_os_file(fs, "sys/fs/inode-nr", limits_type_id);
    let dentry_state = read_optional_os_file(fs, "sys/fs/dentry-state", limits_type_id);
    if file_nr.is_some() || inode_nr.is_some() || dentry_state.is_some() {
        let row = kernel_limits::parse_kernel_limits(
            file_nr.as_deref(),
            inode_nr.as_deref(),
            dentry_state.as_deref(),
        );
        os.kernel_limits = Some(OsKernelLimits {
            ts: Ts(ts),
            nr_file: row.nr_file,
            nr_free_file: row.nr_free_file,
            max_file: row.max_file,
            nr_inode: row.nr_inode,
            nr_free_inode: row.nr_free_inode,
            nr_dentry: row.nr_dentry,
            nr_unused_dentry: row.nr_unused_dentry,
            scope,
        });
        log_collection_finish(limits_type_id, "procfs", 1, started.elapsed());
    }

    let snmp6_type_id = 1_118_001_u32;
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "net/snmp6", snmp6_type_id) {
        os.snmp6 = Some(net_snmp6::parse(&content, ts, net_scope));
        log_collection_finish(snmp6_type_id, "procfs", 1, started.elapsed());
    }

    let nfs_client_type_id = 1_119_001_u32;
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "net/rpc/nfs", nfs_client_type_id) {
        os.nfs_client = nfs::parse_client(&content, ts, net_scope);
        let rows = usize::from(os.nfs_client.is_some());
        log_collection_finish(nfs_client_type_id, "procfs", rows, started.elapsed());
    }

    let nfs_server_type_id = 1_120_001_u32;
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "net/rpc/nfsd", nfs_server_type_id) {
        os.nfs_server = nfs::parse_server(&content, ts, net_scope);
        let rows = usize::from(os.nfs_server.is_some());
        log_collection_finish(nfs_server_type_id, "procfs", rows, started.elapsed());
    }
}

/// Read per-NUMA-node memory from sysfs.
///
/// A machine with one node still gets one row, so a reader never has to guess
/// whether the node breakdown was collected.
pub(super) fn collect_numa(sys: &SysFs, scope: u8, ts: i64) -> Vec<OsNuma> {
    let type_id = 1_117_001_u32;
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

/// Read and parse `/proc/self/mountinfo`, resolving `major == 0` subvolume
/// devices via `/sys`.
pub(super) fn mountinfo_entries(fs: &ProcFs) -> Vec<MountEntry> {
    let type_id = 1_112_002_u32;
    let Some(content) = read_optional_os_file(fs, "self/mountinfo", type_id) else {
        return Vec::new();
    };
    let mut entries = parse_mountinfo(&content);
    entries.retain(|entry| {
        !is_pseudo_filesystem(&entry.fstype) && !is_kernel_tree_mount(&entry.mount_point)
    });
    resolve_major_zero(&SysFs::from_env(), &mut entries);
    entries
}

/// Recover the real `(major, minor)` of `major == 0` subvolume mounts (btrfs,
/// ZFS) whose source is a `/dev/` node, by reading `class/block/<name>/dev`.
/// Entries that cannot be resolved keep `major == 0` and are dropped by
/// `device_map`/`container_device_set` downstream.
pub(crate) fn resolve_major_zero(sys: &SysFs, entries: &mut [MountEntry]) {
    for entry in entries.iter_mut().filter(|e| e.major == 0) {
        let Some(name) = entry.source.strip_prefix("/dev/") else {
            continue;
        };
        let rel = format!("class/block/{name}/dev");
        if let Ok(content) = sys.read(&rel)
            && let Some((major, minor)) = parse_dev_pair(&content)
        {
            entry.major = major;
            entry.minor = minor;
        }
    }
}

/// Build one `os_mountinfo` row per parsed mount entry.
///
/// Mount point, fstype, and source strings are interned here. Filesystem
/// capacity is collected only for the local-filesystem allowlist. It remains
/// nullable for skipped mounts, failed calls, and calls unfinished at the
/// capacity pass deadline.
pub(crate) fn collect_mountinfo(
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    entries: &[MountEntry],
) -> Vec<OsMountinfo> {
    let type_id = 1_112_002_u32;
    let started = Instant::now();
    let capacities = crate::capacity::collect(entries);
    let mut rows = Vec::new();
    for (entry, space) in entries.iter().zip(capacities) {
        let (Some(mount_point), Some(root), Some(fstype), Some(source)) = (
            intern_str(interner, type_id, "self/mountinfo", &entry.mount_point),
            intern_str(interner, type_id, "self/mountinfo", &entry.root),
            intern_str(interner, type_id, "self/mountinfo", &entry.fstype),
            intern_str(interner, type_id, "self/mountinfo", &entry.source),
        ) else {
            continue;
        };
        rows.push(mount_row(
            entry,
            space,
            scope,
            ts,
            MountStringIds {
                mount_point,
                root,
                fstype,
                source,
            },
        ));
    }
    log_collection_finish(type_id, "procfs", rows.len(), started.elapsed());
    rows
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
    let type_id = 1_113_001_u32;
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
