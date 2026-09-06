//! Types `1_021_001` and `1_021_002`: per-segment instance facts.
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
mod tests {
    use super::{Environment, InstanceMetadata, InstanceMetadataV1};
    use crate::{Section, StrId, Ts, lint};

    fn row() -> InstanceMetadata {
        InstanceMetadata {
            ts: Ts(1_000_000),
            hostname: StrId(1),
            kernel_version: StrId(3),
            environment: Environment::Machine.as_u8(),
            clock_ticks_per_sec: 100,
            page_size_bytes: 4096,
            boot_id: StrId(4),
            btime: Ts(1_700_000_000_000_000),
            postgresql_enabled: true,
            postgresql_interval_seconds: 30,
            postgresql_effective_cpus: Some(2),
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(
            lint(&[InstanceMetadataV1::CONTRACT, InstanceMetadata::CONTRACT]),
            Ok(())
        );
    }

    #[test]
    fn contract_contains_only_passive_factual_columns() {
        let names: Vec<&str> = InstanceMetadata::CONTRACT
            .columns
            .iter()
            .map(|column| column.name)
            .collect();
        assert_eq!(
            names,
            [
                "ts",
                "hostname",
                "kernel_version",
                "environment",
                "clock_ticks_per_sec",
                "page_size_bytes",
                "boot_id",
                "btime",
                "postgresql_enabled",
                "postgresql_interval_seconds",
                "postgresql_effective_cpus",
            ]
        );
    }

    #[test]
    fn environment_encodes_as_stable_u8() {
        assert_eq!(Environment::Machine.as_u8(), 0);
        assert_eq!(Environment::Container.as_u8(), 1);
        assert_eq!(
            Environment::from_container_flag(true),
            Environment::Container
        );
        assert_eq!(
            Environment::from_container_flag(false),
            Environment::Machine
        );
    }

    #[test]
    fn roundtrip_preserves_values() {
        let container = InstanceMetadata {
            ts: Ts(2_000_000),
            environment: Environment::Container.as_u8(),
            ..row()
        };
        crate::assert_roundtrips(&[row(), container]);
    }

    #[test]
    fn current_layout_preserves_disabled_and_unknown_sources() {
        let unknown = InstanceMetadata {
            postgresql_effective_cpus: None,
            ..row()
        };
        let disabled = InstanceMetadata {
            postgresql_enabled: false,
            postgresql_effective_cpus: None,
            ..row()
        };
        crate::assert_roundtrips(&[unknown]);
        crate::assert_roundtrips(&[disabled]);
    }
}
