//! Recorded collection metadata and continuity identity.

use kronika_reader::{Cell, ReaderError, Segment};

use super::{
    BuildError, HealthMetadata, INSTANCE_METADATA_TYPE_ID, INSTANCE_METADATA_V1_TYPE_ID,
    INSTANCE_METADATA_V3_TYPE_ID, INSTANCE_METADATA_V4_TYPE_ID, MetadataProjection,
    SnapshotIdentity,
};

impl MetadataProjection {
    pub(super) const fn identity(self) -> SnapshotIdentity {
        SnapshotIdentity {
            environment: self.environment,
            boot_id: self.boot_id,
            boot_time: self.boot_time,
            cgroup: None,
        }
    }

    pub(super) fn postgres_cpus(self) -> Option<u32> {
        if !self.ambiguous && self.postgresql_enabled == Some(true) {
            match self.postgresql_effective_cpus {
                Some(cpus) if cpus > 0 => Some(cpus),
                _ => None,
            }
        } else {
            None
        }
    }

    pub(super) const fn is_ambiguous(self) -> bool {
        self.ambiguous
    }

    fn same_health_facts(self, other: Self) -> bool {
        self.environment == other.environment
            && self.boot_id == other.boot_id
            && self.boot_time == other.boot_time
            && self.os_enabled == other.os_enabled
            && self.postgresql_processes_shared == other.postgresql_processes_shared
            && self.postgresql_enabled == other.postgresql_enabled
            && self.postgresql_effective_cpus == other.postgresql_effective_cpus
            && self.postgresql_interval_seconds == other.postgresql_interval_seconds
    }
}

pub(super) const fn health_metadata(
    segment: &Segment,
    projection: Option<&MetadataProjection>,
) -> Result<HealthMetadata, BuildError> {
    let Some(projection) = projection else {
        return Ok(HealthMetadata {
            timestamp: segment.min_ts(),
            os_enabled: None,
            postgresql_enabled: None,
            postgresql_interval_seconds: 0,
        });
    };
    if projection.is_ambiguous() {
        return Ok(HealthMetadata {
            timestamp: segment.min_ts(),
            os_enabled: None,
            postgresql_enabled: None,
            postgresql_interval_seconds: 0,
        });
    }
    let (Some(timestamp), Some(postgresql_enabled), Some(postgresql_interval_seconds)) = (
        projection.timestamp,
        projection.postgresql_enabled,
        projection.postgresql_interval_seconds,
    ) else {
        return Err(BuildError::InvalidMetadata);
    };
    Ok(HealthMetadata {
        timestamp,
        os_enabled: projection.os_enabled,
        postgresql_enabled: Some(postgresql_enabled),
        postgresql_interval_seconds,
    })
}

pub(super) fn metadata_projection(
    segment: &Segment,
) -> Result<Option<MetadataProjection>, ReaderError> {
    let mut layouts = [
        INSTANCE_METADATA_V4_TYPE_ID,
        INSTANCE_METADATA_V3_TYPE_ID,
        INSTANCE_METADATA_TYPE_ID,
    ]
    .into_iter()
    .filter(|type_id| segment.rows_of(*type_id).is_some());
    let Some(type_id) = layouts.next() else {
        return Ok(None);
    };
    let mut selected = None::<(i64, u64, MetadataProjection)>;
    let mut first_usable = None::<MetadataProjection>;
    let mut ambiguous =
        segment.rows_of(INSTANCE_METADATA_V1_TYPE_ID).is_some() || layouts.next().is_some();
    let mut fields = vec![
        "ts",
        "environment",
        "boot_id",
        "btime",
        "postgresql_enabled",
        "postgresql_effective_cpus",
        "postgresql_interval_seconds",
    ];
    if matches!(
        type_id,
        INSTANCE_METADATA_V3_TYPE_ID | INSTANCE_METADATA_V4_TYPE_ID
    ) {
        fields.extend(["os_enabled", "postgresql_processes_shared"]);
    }
    segment.visit_rows(type_id, &fields, 0, usize::MAX, |ordinal, row| {
        let timestamp = match row.get("ts") {
            Some(Cell::Ts(value)) => Some(*value),
            _ => None,
        };
        let environment = match row.get("environment") {
            Some(Cell::U32(value)) => Some(*value),
            _ => None,
        };
        let boot_id = match row.get("boot_id") {
            Some(Cell::StrId(value)) => Some(*value),
            _ => None,
        };
        let boot_time = match row.get("btime") {
            Some(Cell::Ts(value)) => Some(*value),
            _ => None,
        };
        let postgresql_enabled = match row.get("postgresql_enabled") {
            Some(Cell::Bool(value)) => Some(*value),
            _ => None,
        };
        let postgresql_effective_cpus = match row.get("postgresql_effective_cpus") {
            Some(Cell::U32(value)) => Some(*value),
            _ => None,
        };
        let postgresql_interval_seconds = match row.get("postgresql_interval_seconds") {
            Some(Cell::U64(value)) => Some(*value),
            _ => None,
        };
        let candidate = MetadataProjection {
            ambiguous: false,
            timestamp,
            environment,
            boot_id,
            boot_time,
            os_enabled: match row.get("os_enabled") {
                Some(Cell::Bool(value)) => Some(*value),
                _ => None,
            },
            postgresql_processes_shared: matches!(
                row.get("postgresql_processes_shared"),
                Some(Cell::Bool(true))
            ),
            postgresql_enabled,
            postgresql_effective_cpus,
            postgresql_interval_seconds,
        };
        if let (Some(timestamp), Some(_postgresql_enabled), Some(_postgresql_interval_seconds)) = (
            candidate.timestamp,
            candidate.postgresql_enabled,
            candidate.postgresql_interval_seconds,
        ) {
            if let Some(first) = first_usable {
                ambiguous |= !first.same_health_facts(candidate);
            } else {
                first_usable = Some(candidate);
            }
            if selected
                .as_ref()
                .is_none_or(|&(prior_timestamp, prior_ordinal, _)| {
                    (timestamp, ordinal) > (prior_timestamp, prior_ordinal)
                })
            {
                selected = Some((timestamp, ordinal, candidate));
            }
        }
        true
    })?;
    let mut projection = selected.map_or_else(MetadataProjection::default, |(_, _, row)| row);
    projection.ambiguous = ambiguous;
    Ok(Some(projection))
}

/// Collection choices stored by the collector, independent of web flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectionFacts {
    /// Explicit Linux collection choice; absent in older recordings.
    pub os_enabled: Option<bool>,
    /// Explicit `PostgreSQL` collection choice.
    pub postgresql_enabled: Option<bool>,
    /// SQL PID numbers refer to the recorded process namespace.
    pub postgresql_processes_shared: bool,
}

/// Read the per-segment collection contract.
///
/// # Errors
/// Returns a reader error for invalid recorded metadata.
pub fn collection_facts(segment: &Segment) -> Result<CollectionFacts, ReaderError> {
    let Some(metadata) = metadata_projection(segment)?.filter(|row| !row.ambiguous) else {
        return Ok(CollectionFacts::default());
    };
    Ok(CollectionFacts {
        os_enabled: metadata.os_enabled,
        postgresql_enabled: metadata.postgresql_enabled,
        postgresql_processes_shared: metadata.postgresql_processes_shared,
    })
}
