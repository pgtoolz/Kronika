//! Parsers for `/proc` virtual filesystem files.

pub mod cpuinfo;
pub mod diskstats;
pub mod interrupts;
pub mod kernel_limits;
pub mod loadavg;
pub mod meminfo;
pub mod net_dev;
pub mod net_netstat;
pub mod net_snmp;
pub mod net_snmp6;
pub mod nfs;
pub mod pressure;
pub mod process;
pub mod stat;
pub mod vmstat;
