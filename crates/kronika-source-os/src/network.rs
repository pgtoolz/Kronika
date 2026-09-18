//! Interface counters enriched with namespace-visible sysfs link facts.

use crate::proc::net_dev;
use crate::{CollectionError, ProcFs, SysFs};
use kronika_registry::{StrId, os_netdev::OsNetdev};

/// Acquire interface counters and link facts in source order.
/// Rejected interface names omit only their own rows.
///
/// # Errors
/// Returns a bounded procfs read or counter parse failure.
pub fn collect_netdev(
    fs: &ProcFs,
    sys: &SysFs,
    scope: u8,
    ts: i64,
    mut intern: impl FnMut(&str) -> Option<StrId>,
) -> Result<Vec<OsNetdev>, CollectionError> {
    let content = fs.read_raw("net/dev")?;
    let mut rows = net_dev::parse(&content)?;
    for row in &mut rows {
        (row.speed_mbit, row.duplex) = net_link_facts(sys, &row.iface);
    }
    Ok(rows
        .iter()
        .filter_map(|row| {
            let iface = intern(&row.iface)?;
            Some(row.to_section(scope, ts, iface))
        })
        .collect())
}

/// Negotiated speed and duplex of one interface, from sysfs.
///
/// The kernel returns `EINVAL` for a virtual or down interface, so an absent
/// or unparsable value leaves the speed null and the duplex unknown rather
/// than claiming a link that is not there.
#[must_use]
pub fn net_link_facts(sys: &SysFs, iface: &str) -> (Option<i64>, u8) {
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

macro_rules! protocol_collector {
    ($name:ident, $section:ty, $path:literal, $parse:expr) => {
        /// Read and convert this protocol's counters in the caller's network scope.
        ///
        /// # Errors
        /// Returns a bounded procfs read or counter parse failure.
        pub fn $name(fs: &ProcFs, scope: u8, ts: i64) -> Result<Option<$section>, CollectionError> {
            let content = fs.read_raw($path)?;
            ($parse)(&content, scope, ts).map_err(CollectionError::from)
        }
    };
}

protocol_collector!(
    collect_snmp,
    kronika_registry::os_snmp::OsSnmp,
    "net/snmp",
    |content: &str, scope, ts| crate::proc::net_snmp::parse(content)
        .map(|row| Some(row.to_section(scope, ts)))
);
protocol_collector!(
    collect_netstat,
    kronika_registry::os_netstat::OsNetstat,
    "net/netstat",
    |content: &str, scope, ts| crate::proc::net_netstat::parse(content)
        .map(|row| Some(row.to_section(scope, ts)))
);
protocol_collector!(
    collect_snmp6,
    kronika_registry::os_snmp6::OsSnmp6,
    "net/snmp6",
    |content: &str, scope, ts| Ok::<_, crate::ParseError>(Some(crate::proc::net_snmp6::parse(
        content, ts, scope
    )))
);
protocol_collector!(
    collect_nfs_client,
    kronika_registry::os_nfs::OsNfsClient,
    "net/rpc/nfs",
    |content: &str, scope, ts| Ok::<_, crate::ParseError>(crate::proc::nfs::parse_client(
        content, ts, scope
    ))
);
protocol_collector!(
    collect_nfs_server,
    kronika_registry::os_nfs::OsNfsServer,
    "net/rpc/nfsd",
    |content: &str, scope, ts| Ok::<_, crate::ParseError>(crate::proc::nfs::parse_server(
        content, ts, scope
    ))
);

#[cfg(test)]
#[path = "tests/network.rs"]
mod tests;
