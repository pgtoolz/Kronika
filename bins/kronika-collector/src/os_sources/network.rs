//! Interface facts and protocol counters in the selected network scope.

use std::time::Instant;

use kronika_registry::Section;
use kronika_registry::os_netdev::OsNetdev;
use kronika_source_os::proc::stat::ParseError;
use kronika_source_os::proc::{net_dev, net_netstat, net_snmp, net_snmp6, nfs};
use kronika_source_os::{ProcFs, SysFs};
use kronika_writer::Interner;

use super::OsSources;
use super::io::{intern_str, log_degraded, read_optional_os_file};
use crate::logging::log_collection_finish;

/// Read and parse `/proc/net/dev`, interning interface names into rows.
pub(super) fn collect_netdev(
    fs: &ProcFs,
    sys: &SysFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
) -> Vec<OsNetdev> {
    let type_id = OsNetdev::CONTRACT.type_id.get();
    let started = Instant::now();
    let Some(content) = read_optional_os_file(fs, "net/dev", type_id) else {
        return Vec::new();
    };
    let mut rows = match net_dev::parse(&content) {
        Ok(rows) => rows,
        Err(error) => {
            log_degraded(type_id, "net/dev", &error);
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

/// Read IPv4, extended TCP, IPv6, and NFS counters for one network scope.
pub(super) fn collect_protocol_counters(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    collect_counter(fs, "net/snmp", &mut os.snmp, |content| {
        net_snmp::parse(content).map(|row| Some(row.to_section(scope, ts)))
    });
    collect_counter(fs, "net/netstat", &mut os.netstat, |content| {
        net_netstat::parse(content).map(|row| Some(row.to_section(scope, ts)))
    });
    collect_counter(fs, "net/snmp6", &mut os.snmp6, |content| {
        Ok(Some(net_snmp6::parse(content, ts, scope)))
    });
    collect_counter(fs, "net/rpc/nfs", &mut os.nfs_client, |content| {
        Ok(nfs::parse_client(content, ts, scope))
    });
    collect_counter(fs, "net/rpc/nfsd", &mut os.nfs_server, |content| {
        Ok(nfs::parse_server(content, ts, scope))
    });
}

/// Replace a counter only after a successful read and parse; NFS may return no row.
fn collect_counter<S: Section>(
    fs: &ProcFs,
    source: &'static str,
    output: &mut Option<S>,
    parse: impl FnOnce(&str) -> Result<Option<S>, ParseError>,
) {
    let type_id = S::CONTRACT.type_id.get();
    let started = Instant::now();
    let Some(content) = read_optional_os_file(fs, source, type_id) else {
        return;
    };
    match parse(&content) {
        Ok(row) => {
            *output = row;
            log_collection_finish(
                type_id,
                "procfs",
                usize::from(output.is_some()),
                started.elapsed(),
            );
        }
        Err(error) => log_degraded(type_id, source, &error),
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

#[cfg(test)]
#[path = "../tests/os_sources/network.rs"]
mod tests;
