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
