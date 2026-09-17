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
mod tests {
    use super::{
        Environment, InstanceMetadata, InstanceMetadataV1, InstanceMetadataV3, InstanceMetadataV4,
    };
    use crate::{ColumnClass, ColumnType, Section, StrId, Ts, Unit, lint};

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
            lint(&[
                InstanceMetadataV1::CONTRACT,
                InstanceMetadata::CONTRACT,
                InstanceMetadataV3::CONTRACT,
                InstanceMetadataV4::CONTRACT,
            ]),
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

    #[test]
    fn v4_adds_family_cadences_without_changing_v3_columns() {
        let previous = InstanceMetadataV3::CONTRACT.columns;
        let current = InstanceMetadataV4::CONTRACT.columns;
        assert_eq!(InstanceMetadataV3::CONTRACT.type_id.get(), 1_021_003);
        assert_eq!(InstanceMetadataV4::CONTRACT.type_id.get(), 1_021_004);
        assert_eq!(&current[..previous.len()], previous);
        let added = &current[previous.len()..];
        assert_eq!(
            added.iter().map(|column| column.name).collect::<Vec<_>>(),
            [
                "postgresql_instance_interval_seconds",
                "postgresql_relations_interval_seconds",
                "postgresql_statements_interval_seconds",
            ]
        );
        for column in added {
            assert_eq!(column.ty, ColumnType::U64);
            assert_eq!(column.class, ColumnClass::Label);
            assert_eq!(column.unit, Some(Unit::Seconds));
            assert!(!column.nullable);
        }
    }

    #[test]
    fn v4_roundtrip_preserves_distinct_cadences_and_optional_identity() {
        let remote = InstanceMetadataV4 {
            ts: Ts(1_000_000),
            hostname: None,
            kernel_version: None,
            environment: None,
            clock_ticks_per_sec: None,
            page_size_bytes: None,
            boot_id: None,
            btime: None,
            os_enabled: false,
            postgresql_processes_shared: false,
            postgresql_enabled: true,
            postgresql_interval_seconds: 11,
            postgresql_effective_cpus: Some(4),
            postgresql_instance_interval_seconds: 37,
            postgresql_relations_interval_seconds: 401,
            postgresql_statements_interval_seconds: 601,
        };
        let local = InstanceMetadataV4 {
            ts: Ts(2_000_000),
            hostname: Some(StrId(1)),
            kernel_version: Some(StrId(2)),
            environment: Some(Environment::Machine.as_u8()),
            clock_ticks_per_sec: Some(100),
            page_size_bytes: Some(4_096),
            boot_id: Some(StrId(3)),
            btime: Some(Ts(500_000)),
            os_enabled: true,
            postgresql_processes_shared: true,
            postgresql_effective_cpus: None,
            ..remote
        };
        crate::assert_roundtrips(&[remote, local]);
    }
}
