//! Construction of the small presentation-series allowlist.

mod metadata;
mod postgres;
mod stalls;

pub use metadata::{CollectionFacts, collection_facts};
pub(crate) use stalls::cgroup_identities;
pub use stalls::visit_health_points;

use std::collections::BTreeMap;

#[cfg(feature = "posix")]
use kronika_reader::{Reader, SegmentKind, SegmentRef};
use kronika_reader::{ReaderError, Segment};
use metadata::{health_metadata, metadata_projection};
use postgres::{
    active_backend_points, active_backend_samples, combined_active_points, overall_points,
    postgres_health_points, transaction_points,
};
use stalls::health_points;
#[cfg(feature = "posix")]
use stalls::last_stall_snapshot;

use crate::cpu_capacity::RecordedCpuCapacity;
use crate::detect::{FindingBuilder, finding_layout};
use crate::file::Index;
use crate::health::Stall;
use crate::series::{
    ActiveBackendPoint, HealthPoint, SeriesBlock, SeriesKey, SeriesKind, pg_activity_layout,
    pg_database_layout,
};

/// Reserved input-free layout for the derived OS health gauge.
pub const DERIVED_HEALTH_TYPE_ID: u32 = 0;
/// `type_id` of `instance_metadata`.
pub const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_002;
/// Collection families and optional Linux identity.
pub const INSTANCE_METADATA_V3_TYPE_ID: u32 = 1_021_003;
/// Per-family `PostgreSQL` collection intervals.
pub const INSTANCE_METADATA_V4_TYPE_ID: u32 = 1_021_004;
/// Previous `instance_metadata`, used only to retain OS-health readability.
pub const INSTANCE_METADATA_V1_TYPE_ID: u32 = 1_021_001;
/// `type_id` of `os_psi`.
pub const OS_PSI_TYPE_ID: u32 = 1_107_001;

// Resource IDs recorded by os_psi.
const CPU: u32 = 0;
const MEMORY: u32 = 1;
const IO: u32 = 2;
// Recorded OS scope IDs used to select matching pressure samples.
const HOST: u32 = 0;
const POD: u32 = 1;
const CONTAINER: u32 = 3;

#[derive(Debug, Clone, Copy, Default)]
struct HealthSeed {
    os: Option<StallSnapshot>,
    postgres: Option<HealthPoint>,
    capacity: Option<CapacitySeed>,
}

#[derive(Debug, Clone, Copy)]
struct CapacitySeed {
    identity: SnapshotIdentity,
    timestamp: i64,
    cpus: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotIdentity {
    environment: Option<u32>,
    boot_id: Option<u64>,
    boot_time: Option<i64>,
    cgroup: Option<[Option<u64>; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StallSnapshot {
    timestamp: i64,
    identity: SnapshotIdentity,
    stall: Option<Stall>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActiveBackendSample {
    pub(crate) timestamp: i64,
    pub(crate) first_active_ordinal: Option<u32>,
    pub(crate) count: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct MetadataProjection {
    ambiguous: bool,
    timestamp: Option<i64>,
    environment: Option<u32>,
    boot_id: Option<u64>,
    boot_time: Option<i64>,
    os_enabled: Option<bool>,
    postgresql_processes_shared: bool,
    postgresql_enabled: Option<bool>,
    postgresql_effective_cpus: Option<u32>,
    postgresql_interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct HealthMetadata {
    timestamp: i64,
    os_enabled: Option<bool>,
    postgresql_enabled: Option<bool>,
    postgresql_interval_seconds: u64,
}

/// Why an allowlisted series could not be derived.
#[derive(Debug)]
pub enum BuildError {
    /// The production reader rejected an input body.
    Reader(ReaderError),
    /// A `PostgreSQL` state dictionary id had no value.
    UnresolvedState(u64),
    /// No usable current metadata row was present.
    InvalidMetadata,
    /// A `pg_log_errors.category` value was absent or outside its registry range.
    InvalidLogErrorCategory,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reader(error) => error.fmt(f),
            Self::UnresolvedState(id) => {
                write!(
                    f,
                    "pg_stat_activity state has unresolved dictionary id {id}"
                )
            }
            Self::InvalidMetadata => write!(f, "instance_metadata has no usable row"),
            Self::InvalidLogErrorCategory => {
                write!(f, "pg_log_errors.category must be between 0 and 10")
            }
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reader(error) => Some(error),
            Self::UnresolvedState(_) | Self::InvalidMetadata | Self::InvalidLogErrorCategory => {
                None
            }
        }
    }
}

impl From<ReaderError> for BuildError {
    fn from(error: ReaderError) -> Self {
        Self::Reader(error)
    }
}

#[cfg(feature = "posix")]
fn predecessor_health_seed(
    reader: &Reader,
    predecessor: Option<&SegmentRef>,
    requested: &[SeriesKey],
) -> Result<HealthSeed, BuildError> {
    let needs_os = requested
        .iter()
        .any(|key| matches!(key.kind, SeriesKind::OsHealth | SeriesKind::OverallHealth));
    let needs_postgres = requested.contains(&SeriesKey::OVERALL_HEALTH);
    let needs_capacity = requested.iter().copied().any(needs_postgres_capacity);
    let Some(predecessor) = predecessor.filter(|_| needs_os || needs_postgres || needs_capacity)
    else {
        return Ok(HealthSeed::default());
    };
    let has_psi = predecessor
        .sections()
        .iter()
        .any(|section| section.type_id == OS_PSI_TYPE_ID);
    let has_metadata = predecessor.sections().iter().any(|section| {
        matches!(
            section.type_id,
            INSTANCE_METADATA_TYPE_ID | INSTANCE_METADATA_V3_TYPE_ID | INSTANCE_METADATA_V4_TYPE_ID
        )
    });
    if !(needs_os && has_psi || (needs_postgres || needs_capacity) && has_metadata) {
        return Ok(HealthSeed::default());
    }

    let segment = reader.open_segment(predecessor)?;
    let metadata_projection = metadata_projection(&segment)?;
    let capacity = postgres_capacity(
        &segment,
        metadata_projection.filter(|_| needs_capacity),
        None,
    )?;
    let capacity_seed = metadata_projection.and_then(|projection| {
        capacity
            .last_snapshot()
            .map(|(timestamp, cpus)| CapacitySeed {
                identity: projection.identity(),
                timestamp,
                cpus,
            })
    });
    let os = if needs_os && has_psi {
        last_stall_snapshot(&segment, metadata_projection.as_ref())?
    } else {
        None
    };
    let postgres = if needs_postgres && has_metadata {
        let metadata = health_metadata(&segment, metadata_projection.as_ref())?;
        if metadata.postgresql_enabled == Some(true) {
            let mut activity = BTreeMap::<u32, Vec<ActiveBackendPoint>>::new();
            for type_id in segment
                .type_ids()
                .filter(|type_id| pg_activity_layout(*type_id))
            {
                activity.insert(
                    type_id,
                    active_backend_points(&active_backend_samples(&segment, type_id)?),
                );
            }
            postgres_health_points(&metadata, &combined_active_points(&activity), &capacity)
                .and_then(|points| points.last().copied())
        } else {
            None
        }
    } else {
        None
    };
    Ok(HealthSeed {
        os,
        postgres,
        capacity: capacity_seed,
    })
}

/// Return the complete current allowlist for a captured segment.
#[must_use]
pub fn keys(segment: &Segment) -> Vec<SeriesKey> {
    let mut keys = vec![
        SeriesKey::OS_HEALTH,
        SeriesKey::OVERALL_HEALTH,
        SeriesKey::POSTGRES_HEALTH,
        SeriesKey {
            kind: SeriesKind::Findings,
            type_id: DERIVED_HEALTH_TYPE_ID,
        },
    ];
    for type_id in segment.type_ids() {
        if pg_database_layout(type_id) {
            keys.push(SeriesKey {
                kind: SeriesKind::PgTransactionsPerSecond,
                type_id,
            });
        } else if pg_activity_layout(type_id) {
            keys.push(SeriesKey {
                kind: SeriesKind::PgActiveBackends,
                type_id,
            });
        }
        if finding_layout(type_id) {
            keys.push(SeriesKey {
                kind: SeriesKind::Findings,
                type_id,
            });
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Build every allowlisted presentation series present in a segment.
///
/// # Errors
///
/// Returns a production-reader or dictionary-resolution failure.
pub fn build(segment: &Segment) -> Result<Index, BuildError> {
    build_selected(segment, &keys(segment))
}

/// Build selected allowlisted series for a captured active response.
///
/// # Errors
///
/// Returns a production-reader or dictionary-resolution failure.
pub fn build_selected(segment: &Segment, requested: &[SeriesKey]) -> Result<Index, BuildError> {
    let finder = FindingBuilder::new(segment, requested);
    let mut active_samples = BTreeMap::new();
    let mut postgres_cpus = RecordedCpuCapacity::default();
    let mut index = build_selected_series(
        segment,
        requested,
        HealthSeed::default(),
        &mut active_samples,
        &mut postgres_cpus,
    )?;
    index
        .blocks
        .extend(finder.finish(segment, &index, &active_samples, &postgres_cpus)?);
    index.blocks.sort_by_key(SeriesBlock::key);
    Ok(index)
}

/// Build the complete index while reading earlier finished ZMS through the
/// same production reader when a comparison needs prior values.
///
/// # Errors
///
/// Returns a production-reader, dictionary-resolution, or build failure.
#[cfg(feature = "posix")]
pub fn build_from_reader(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segment: &Segment,
) -> Result<Index, BuildError> {
    let requested = keys(segment);
    build_selected_from_reader(reader, segment_ref, segment, &requested)
}

#[cfg(feature = "posix")]
pub(crate) fn build_selected_from_reader(
    reader: &Reader,
    segment_ref: &SegmentRef,
    segment: &Segment,
    requested: &[SeriesKey],
) -> Result<Index, BuildError> {
    let mut finder = FindingBuilder::new(segment, requested);
    let listing = reader
        .catalog_discovery()?
        .segments_with_predecessor(finder.window_start()..segment_ref.min_ts())?;
    let predecessor = listing
        .segments
        .iter()
        .filter(|prior| prior.kind() == SegmentKind::Finished)
        .max_by_key(|prior| (prior.max_ts(), prior.id()));
    let health_seed = predecessor_health_seed(reader, predecessor, requested)?;
    // Include one segment before the 15-minute comparison window.
    let mut priors: Vec<_> = listing
        .segments
        .into_iter()
        .filter(|prior| prior.kind() == SegmentKind::Finished)
        .filter(|prior| finder.needs(prior))
        .collect();
    priors.sort_by_key(SegmentRef::min_ts);
    let inside = priors
        .iter()
        .position(|prior| prior.max_ts() >= finder.window_start())
        .unwrap_or(priors.len());
    let from = inside.saturating_sub(1);
    for prior_ref in priors.drain(from..) {
        let prior = reader.open_segment(&prior_ref)?;
        finder.observe_prior(&prior)?;
    }
    let mut active_samples = BTreeMap::new();
    let mut postgres_cpus = RecordedCpuCapacity::default();
    let mut index = build_selected_series(
        segment,
        requested,
        health_seed,
        &mut active_samples,
        &mut postgres_cpus,
    )?;
    index
        .blocks
        .extend(finder.finish(segment, &index, &active_samples, &postgres_cpus)?);
    index.blocks.sort_by_key(SeriesBlock::key);
    Ok(index)
}

fn postgres_capacity(
    segment: &Segment,
    projection: Option<MetadataProjection>,
    seed: Option<CapacitySeed>,
) -> Result<RecordedCpuCapacity, ReaderError> {
    let Some(projection) = projection
        .filter(|projection| !projection.ambiguous && projection.postgresql_enabled == Some(true))
    else {
        return Ok(RecordedCpuCapacity::default());
    };
    let mut capacity = RecordedCpuCapacity::read(
        segment,
        projection
            .postgresql_processes_shared
            .then_some(projection.environment)
            .flatten(),
        projection.postgres_cpus(),
    )?;
    if let Some(seed) = seed.filter(|seed| {
        projection.postgresql_processes_shared && seed.identity == projection.identity()
    }) {
        capacity.seed(seed.timestamp, seed.cpus);
    }
    Ok(capacity)
}

fn needs_postgres_capacity(key: SeriesKey) -> bool {
    matches!(
        key.kind,
        SeriesKind::PostgresHealth | SeriesKind::OverallHealth
    ) || key.kind == SeriesKind::Findings && pg_activity_layout(key.type_id)
}

fn build_selected_series(
    segment: &Segment,
    requested: &[SeriesKey],
    health_seed: HealthSeed,
    active_samples: &mut BTreeMap<u32, Vec<ActiveBackendSample>>,
    postgres_cpus: &mut RecordedCpuCapacity,
) -> Result<Index, BuildError> {
    let mut requested = requested.to_vec();
    requested.sort_unstable();
    requested.dedup();
    let mut blocks = Vec::with_capacity(requested.len());
    let wants_health = requested.iter().any(|key| {
        matches!(
            key.kind,
            SeriesKind::OsHealth | SeriesKind::OverallHealth | SeriesKind::PostgresHealth
        )
    });
    let needs_metadata = wants_health
        || requested
            .iter()
            .any(|key| key.kind == SeriesKind::Findings && pg_activity_layout(key.type_id));
    let metadata_projection = needs_metadata
        .then(|| metadata_projection(segment))
        .transpose()?
        .flatten();
    *postgres_cpus = postgres_capacity(
        segment,
        metadata_projection.filter(|_| requested.iter().copied().any(needs_postgres_capacity)),
        health_seed.capacity,
    )?;
    let metadata = wants_health
        .then(|| health_metadata(segment, metadata_projection.as_ref()))
        .transpose()?;
    let os_enabled = metadata.as_ref().and_then(|metadata| metadata.os_enabled) != Some(false);
    let os_points = (wants_health && os_enabled)
        .then(|| health_points(segment, health_seed.os, metadata_projection.as_ref()))
        .transpose()?
        .unwrap_or_default();
    let needs_pg_health = requested.contains(&SeriesKey::POSTGRES_HEALTH)
        || requested.contains(&SeriesKey::OVERALL_HEALTH);
    let mut activity = BTreeMap::<u32, Vec<ActiveBackendPoint>>::new();
    for type_id in segment
        .type_ids()
        .filter(|type_id| pg_activity_layout(*type_id))
    {
        let raw_requested = requested.contains(&SeriesKey {
            kind: SeriesKind::PgActiveBackends,
            type_id,
        });
        let finding_requested = requested.contains(&SeriesKey {
            kind: SeriesKind::Findings,
            type_id,
        });
        if raw_requested || needs_pg_health || finding_requested {
            let samples = active_backend_samples(segment, type_id)?;
            if raw_requested || needs_pg_health {
                activity.insert(type_id, active_backend_points(&samples));
            }
            if finding_requested {
                active_samples.insert(type_id, samples);
            }
        }
    }
    let combined_active = combined_active_points(&activity);
    let postgres_points = metadata
        .as_ref()
        .and_then(|metadata| postgres_health_points(metadata, &combined_active, postgres_cpus));

    for key in requested {
        match key.kind {
            SeriesKind::OsHealth if key == SeriesKey::OS_HEALTH && os_enabled => {
                blocks.push(SeriesBlock::OsHealth(os_points.clone()));
            }
            SeriesKind::OverallHealth if key == SeriesKey::OVERALL_HEALTH => {
                let metadata = metadata.as_ref().ok_or(BuildError::InvalidMetadata)?;
                blocks.push(SeriesBlock::OverallHealth(overall_points(
                    &os_points,
                    postgres_points.as_deref(),
                    health_seed.postgres,
                    metadata,
                )));
            }
            SeriesKind::PostgresHealth if key == SeriesKey::POSTGRES_HEALTH => {
                if let Some(points) = &postgres_points {
                    blocks.push(SeriesBlock::PostgresHealth(points.clone()));
                }
            }
            SeriesKind::PgTransactionsPerSecond
                if pg_database_layout(key.type_id) && segment.rows_of(key.type_id).is_some() =>
            {
                blocks.push(SeriesBlock::PgTransactions {
                    type_id: key.type_id,
                    points: transaction_points(segment, key.type_id)?,
                });
            }
            SeriesKind::PgActiveBackends
                if pg_activity_layout(key.type_id) && segment.rows_of(key.type_id).is_some() =>
            {
                blocks.push(SeriesBlock::PgActiveBackends {
                    type_id: key.type_id,
                    points: activity.remove(&key.type_id).unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    blocks.sort_by_key(SeriesBlock::key);
    Ok(Index { blocks })
}

#[cfg(test)]
use postgres::transaction_rate;
#[cfg(test)]
#[path = "tests/build.rs"]
mod tests;
