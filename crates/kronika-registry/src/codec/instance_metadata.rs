//! Types `1_021_001`–`1_021_004`: per-segment instance facts and collection intervals.
//!
//! Mandatory in every segment carrying snapshots. It records the node identity
//! and the constants needed to interpret the other sections: without
//! `clock_ticks_per_sec` and `page_size_bytes` the tick and page counters in
//! the OS sections mean nothing, and `boot_id`/`btime` anchor a segment to one
//! boot of one machine. `environment` is decided at collection time so no
//! reader has to re-derive whether the numbers describe a VM or a container.

use crate::{Section, StrId, Ts};

/// Where the collector was running when it took the snapshot.
///
/// Stored as the `environment` `u8` column. The distinction says which
/// pressure and cgroup rows describe the collector itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// Bare metal or a virtual machine: the host's own resources.
    Machine,
    /// Inside a container, under a cgroup limit.
    Container,
}

impl Environment {
    /// Stable on-disk encoding.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Machine => 0,
            Self::Container => 1,
        }
    }

    /// The environment a container-detection flag describes.
    #[must_use]
    pub const fn from_container_flag(in_container: bool) -> Self {
        if in_container {
            Self::Container
        } else {
            Self::Machine
        }
    }
}

/// One row of type `1_021_002`; one row per segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_021_002,
    name = "instance_metadata",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct InstanceMetadata {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Collector hostname.
    #[column(l)]
    pub hostname: StrId,
    /// OS kernel version string.
    #[column(l)]
    pub kernel_version: StrId,
    /// `0` machine or VM, `1` container. See [`Environment`].
    #[column(l)]
    pub environment: u8,
    /// `sysconf(_SC_CLK_TCK)`; needed to convert OS tick counters.
    #[column(l)]
    pub clock_ticks_per_sec: i64,
    /// OS page size, bytes.
    #[column(l)]
    pub page_size_bytes: i64,
    /// `/proc/sys/kernel/random/boot_id`.
    #[column(l)]
    pub boot_id: StrId,
    /// Kernel boot time (`/proc/stat` btime), unix microseconds.
    #[column(l)]
    pub btime: Ts,
    /// Whether `PostgreSQL` metric collection was configured.
    #[column(l)]
    pub postgresql_enabled: bool,
    /// Effective cadence of the `PostgreSQL` snapshot source, seconds.
    #[column(l, unit = seconds)]
    pub postgresql_interval_seconds: u64,
    /// Explicit CPU capacity override for the monitored `PostgreSQL` server.
    #[column(l)]
    pub postgresql_effective_cpus: Option<u32>,
}

/// Recorded collection families and the Linux resource relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_021_003,
    name = "instance_metadata",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct InstanceMetadataV3 {
    /// Collection timestamp, Unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Linux source hostname, absent without OS collection.
    #[column(l)]
    pub hostname: Option<StrId>,
    /// Linux source kernel release.
    #[column(l)]
    pub kernel_version: Option<StrId>,
    /// Linux source environment: 0 machine, 1 container.
    #[column(l)]
    pub environment: Option<u8>,
    /// Ticks per second for recorded OS counters.
    #[column(l)]
    pub clock_ticks_per_sec: Option<i64>,
    /// Page size for recorded OS counters.
    #[column(l)]
    pub page_size_bytes: Option<i64>,
    /// Boot identity of the Linux source.
    #[column(l)]
    pub boot_id: Option<StrId>,
    /// Linux source boot time, Unix microseconds.
    #[column(l)]
    pub btime: Option<Ts>,
    /// Whether Linux collection was configured.
    #[column(l)]
    pub os_enabled: bool,
    /// The deployment uses one machine resource and process namespace for PG and Linux.
    #[column(l)]
    pub postgresql_processes_shared: bool,
    /// Whether `PostgreSQL` collection was configured.
    #[column(l)]
    pub postgresql_enabled: bool,
    /// Effective `PostgreSQL` collection cadence, seconds.
    #[column(l, unit = seconds)]
    pub postgresql_interval_seconds: u64,
    /// Explicit capacity of the `PostgreSQL` server, CPU cores.
    #[column(l)]
    pub postgresql_effective_cpus: Option<u32>,
}

/// Per-family collection intervals and the Linux resource relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_021_004,
    name = "instance_metadata",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct InstanceMetadataV4 {
    /// Collection timestamp, Unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Linux source hostname, absent without OS collection.
    #[column(l)]
    pub hostname: Option<StrId>,
    /// Linux source kernel release.
    #[column(l)]
    pub kernel_version: Option<StrId>,
    /// Linux source environment: 0 machine, 1 container.
    #[column(l)]
    pub environment: Option<u8>,
    /// Ticks per second for recorded OS counters.
    #[column(l)]
    pub clock_ticks_per_sec: Option<i64>,
    /// Page size for recorded OS counters.
    #[column(l)]
    pub page_size_bytes: Option<i64>,
    /// Boot identity of the Linux source.
    #[column(l)]
    pub boot_id: Option<StrId>,
    /// Linux source boot time, Unix microseconds.
    #[column(l)]
    pub btime: Option<Ts>,
    /// Whether Linux collection was configured.
    #[column(l)]
    pub os_enabled: bool,
    /// The deployment uses one machine resource and process namespace for PG and Linux.
    #[column(l)]
    pub postgresql_processes_shared: bool,
    /// Whether `PostgreSQL` collection was configured.
    #[column(l)]
    pub postgresql_enabled: bool,
    /// Normal activity and VACUUM-progress cadence, excluding temporary acceleration.
    #[column(l, unit = seconds)]
    pub postgresql_interval_seconds: u64,
    /// Explicit capacity of the `PostgreSQL` server, CPU cores.
    #[column(l)]
    pub postgresql_effective_cpus: Option<u32>,
    /// Server counter and settings collection cadence, seconds.
    #[column(l, unit = seconds)]
    pub postgresql_instance_interval_seconds: u64,
    /// Per-database table and index collection cadence, seconds.
    #[column(l, unit = seconds)]
    pub postgresql_relations_interval_seconds: u64,
    /// Statement and plan extension collection cadence, seconds.
    #[column(l, unit = seconds)]
    pub postgresql_statements_interval_seconds: u64,
}

/// Previous type `1_021_001`, retained so existing ZMS remains readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_021_001,
    name = "instance_metadata",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct InstanceMetadataV1 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Collector hostname.
    #[column(l)]
    pub hostname: StrId,
    /// OS kernel version string.
    #[column(l)]
    pub kernel_version: StrId,
    /// `0` machine or VM, `1` container. See [`Environment`].
    #[column(l)]
    pub environment: u8,
    /// `sysconf(_SC_CLK_TCK)`; needed to convert OS tick counters.
    #[column(l)]
    pub clock_ticks_per_sec: i64,
    /// OS page size, bytes.
    #[column(l)]
    pub page_size_bytes: i64,
    /// `/proc/sys/kernel/random/boot_id`.
    #[column(l)]
    pub boot_id: StrId,
    /// Kernel boot time (`/proc/stat` btime), unix microseconds.
    #[column(l)]
    pub btime: Ts,
}

#[cfg(test)]
#[path = "../tests/codec/instance_metadata.rs"]
mod tests;
