//! Interface facts and protocol counters in the selected network scope.

use std::time::Instant;

use kronika_registry::Section;
use kronika_registry::os_netdev::OsNetdev;
use kronika_source_os::{CollectionError, network};
use kronika_source_os::{ProcFs, SysFs};
use kronika_writer::Interner;

use super::OsSources;
use super::io::{intern_str, log_degraded};
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
    let rows = network::collect_netdev(fs, sys, scope, ts, |value| {
        intern_str(interner, type_id, "net/dev", value)
    });
    super::io::collected_rows(rows, "net/dev", started)
}

/// Read IPv4, extended TCP, IPv6, and NFS counters for one network scope.
pub(super) fn collect_protocol_counters(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    collect_counter("net/snmp", &mut os.snmp, || {
        network::collect_snmp(fs, scope, ts)
    });
    collect_counter("net/netstat", &mut os.netstat, || {
        network::collect_netstat(fs, scope, ts)
    });
    collect_counter("net/snmp6", &mut os.snmp6, || {
        network::collect_snmp6(fs, scope, ts)
    });
    collect_counter("net/rpc/nfs", &mut os.nfs_client, || {
        network::collect_nfs_client(fs, scope, ts)
    });
    collect_counter("net/rpc/nfsd", &mut os.nfs_server, || {
        network::collect_nfs_server(fs, scope, ts)
    });
}

/// Replace a counter only after a successful read and parse; NFS may return no row.
fn collect_counter<S: Section>(
    source: &'static str,
    output: &mut Option<S>,
    collect: impl FnOnce() -> Result<Option<S>, CollectionError>,
) {
    let type_id = S::CONTRACT.type_id.get();
    let started = Instant::now();
    match collect() {
        Ok(row) => {
            *output = row;
            log_collection_finish(
                type_id,
                "procfs",
                usize::from(output.is_some()),
                started.elapsed(),
            );
        }
        Err(error) if !error.is_missing() => log_degraded(type_id, source, &error),
        Err(_) => {}
    }
}

#[cfg(test)]
#[path = "../tests/os_sources/network.rs"]
mod tests;
