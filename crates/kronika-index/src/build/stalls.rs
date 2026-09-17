//! OS pressure snapshots and continuity-aware health folding.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use kronika_reader::{Cell, ReaderError, Resolved, Segment};
use kronika_registry::instance_metadata::Environment;

use super::{
    CONTAINER, CPU, HOST, INSTANCE_METADATA_TYPE_ID, INSTANCE_METADATA_V1_TYPE_ID, IO, MEMORY,
    MetadataProjection, OS_PSI_TYPE_ID, POD, SnapshotIdentity, StallSnapshot,
};

use crate::build::metadata::metadata_projection;
use crate::health::{Stall, health};
use crate::series::HealthPoint;

#[derive(Debug, Default)]
struct PartialStall {
    cpu: Option<i64>,
    memory: Option<i64>,
    io: Option<i64>,
}

/// Visit full-resolution derived OS health points in timestamp order.
///
/// # Errors
///
/// Returns a production-reader failure for either input section.
pub fn visit_health_points(
    segment: &Segment,
    keep_going: impl FnMut() -> bool,
    visitor: impl FnMut(HealthPoint) -> bool,
) -> Result<(), ReaderError> {
    let metadata = metadata_projection(segment)?;
    visit_health_points_with_seed(segment, None, metadata.as_ref(), keep_going, visitor)
}

fn visit_health_points_with_seed(
    segment: &Segment,
    mut seed: Option<StallSnapshot>,
    metadata: Option<&MetadataProjection>,
    keep_going: impl FnMut() -> bool,
    mut visitor: impl FnMut(HealthPoint) -> bool,
) -> Result<(), ReaderError> {
    if metadata.is_some_and(|projection| projection.is_ambiguous()) {
        return visit_unknown_health_points(segment, keep_going, visitor);
    }
    let mut previous = seed.take();
    visit_stall_snapshots(segment, metadata, keep_going, |snapshot| {
        let value = previous
            .as_ref()
            .filter(|before| before.identity == snapshot.identity)
            .and_then(|before| {
                before
                    .stall
                    .zip(snapshot.stall)
                    .and_then(|(before_stall, after)| {
                        health(before_stall, before.timestamp, after, snapshot.timestamp)
                    })
            });
        let timestamp = snapshot.timestamp;
        previous = Some(snapshot);
        visitor(HealthPoint { timestamp, value })
    })
}

fn visit_unknown_health_points(
    segment: &Segment,
    mut keep_going: impl FnMut() -> bool,
    mut visitor: impl FnMut(HealthPoint) -> bool,
) -> Result<(), ReaderError> {
    if segment.rows_of(OS_PSI_TYPE_ID).is_none() || !keep_going() {
        return Ok(());
    }
    let mut timestamps = BTreeSet::new();
    let mut running = true;
    segment.visit_rows(OS_PSI_TYPE_ID, &["ts"], 0, usize::MAX, |_ordinal, row| {
        running = keep_going();
        if !running {
            return false;
        }
        if let Some(Cell::Ts(timestamp)) = row.get("ts") {
            timestamps.insert(*timestamp);
        }
        true
    })?;
    if !running {
        return Ok(());
    }
    for timestamp in timestamps {
        if !keep_going()
            || !visitor(HealthPoint {
                timestamp,
                value: None,
            })
        {
            break;
        }
    }
    Ok(())
}

fn visit_stall_snapshots(
    segment: &Segment,
    metadata: Option<&MetadataProjection>,
    mut keep_going: impl FnMut() -> bool,
    mut visitor: impl FnMut(StallSnapshot) -> bool,
) -> Result<(), ReaderError> {
    if segment.rows_of(OS_PSI_TYPE_ID).is_none()
        || metadata.is_some_and(|projection| projection.is_ambiguous())
        || !keep_going()
    {
        return Ok(());
    }
    let mut running = true;
    let Some(identity) = stall_snapshot_identity(segment, metadata, &mut keep_going)? else {
        return Ok(());
    };
    let environment = identity.environment;
    let groups = cgroup_identities(segment, ["cpu_identity", "memory_identity", "io_identity"])?;
    let selected_aggregate = segment.rows_of(1_205_002).is_some();
    let mut snapshots: BTreeMap<i64, PartialStall> = BTreeMap::new();
    segment.visit_rows(
        OS_PSI_TYPE_ID,
        &["ts", "resource", "some_total", "scope"],
        0,
        usize::MAX,
        |_ordinal, row| {
            running = keep_going();
            if !running {
                return false;
            }
            let Some(Cell::Ts(ts)) = row.get("ts") else {
                return true;
            };
            let snapshot = snapshots.entry(*ts).or_default();
            let (Some(Cell::U32(resource)), Some(Cell::I64(total)), Some(Cell::U32(scope))) =
                (row.get("resource"), row.get("some_total"), row.get("scope"))
            else {
                return true;
            };
            let matching_scope = match environment {
                Some(value) if value == u32::from(Environment::Machine.as_u8()) => *scope == HOST,
                Some(value) if value == u32::from(Environment::Container.as_u8()) => {
                    if selected_aggregate {
                        *scope == 4
                    } else {
                        matches!(*scope, POD | CONTAINER)
                    }
                }
                _ => false,
            };
            if matching_scope {
                match *resource {
                    CPU => snapshot.cpu = Some(*total),
                    MEMORY => snapshot.memory = Some(*total),
                    IO => snapshot.io = Some(*total),
                    _ => {}
                }
            }
            true
        },
    )?;
    if !running {
        return Ok(());
    }

    for (timestamp, snapshot) in snapshots {
        if !keep_going() {
            break;
        }
        let current = match (snapshot.cpu, snapshot.memory, snapshot.io) {
            (Some(cpu), Some(memory), Some(io)) => Some(Stall { cpu, memory, io }),
            _ => None,
        };
        if !visitor(StallSnapshot {
            timestamp,
            identity: SnapshotIdentity {
                cgroup: groups
                    .range(..=timestamp)
                    .next_back()
                    .map(|(_, value)| *value),
                ..identity
            },
            stall: current,
        }) {
            break;
        }
    }
    Ok(())
}

pub(crate) fn cgroup_identities<const N: usize>(
    segment: &Segment,
    fields: [&'static str; N],
) -> Result<BTreeMap<i64, [Option<u64>; N]>, ReaderError> {
    if segment.rows_of(1_205_002).is_none() {
        return Ok(BTreeMap::new());
    }
    let mut rows = BTreeMap::new();
    let mut ids = HashSet::new();
    let mut projection = vec!["ts"];
    projection.extend(fields);
    segment.visit_rows(1_205_002, &projection, 0, usize::MAX, |_, row| {
        if let Some(Cell::Ts(ts)) = row.get("ts") {
            let identities = fields.map(|name| match row.get(name) {
                Some(Cell::StrId(id)) => Some(*id),
                _ => None,
            });
            ids.extend(identities.iter().flatten().copied());
            rows.insert(*ts, identities);
        }
        true
    })?;
    let dictionary = segment.dictionary_for(&ids)?;
    for identities in rows.values_mut() {
        for id in identities {
            *id = id.filter(|id| matches!(dictionary.resolve(*id), Some(Resolved::Str(_))));
        }
    }
    Ok(rows)
}

fn stall_snapshot_identity(
    segment: &Segment,
    metadata: Option<&MetadataProjection>,
    mut keep_going: impl FnMut() -> bool,
) -> Result<Option<SnapshotIdentity>, ReaderError> {
    if let Some(metadata) = metadata {
        return Ok(Some(metadata.identity()));
    }
    let mut running = true;
    let mut identity = SnapshotIdentity {
        environment: None,
        boot_id: None,
        boot_time: None,
        cgroup: None,
    };
    if let Some(metadata_type_id) = segment
        .rows_of(INSTANCE_METADATA_TYPE_ID)
        .map(|_| INSTANCE_METADATA_TYPE_ID)
        .or_else(|| {
            segment
                .rows_of(INSTANCE_METADATA_V1_TYPE_ID)
                .map(|_| INSTANCE_METADATA_V1_TYPE_ID)
        })
    {
        segment.visit_rows(
            metadata_type_id,
            &["environment", "boot_id", "btime"],
            0,
            usize::MAX,
            |_ordinal, row| {
                running = keep_going();
                if !running {
                    return false;
                }
                if let Some(Cell::U32(value)) = row.get("environment") {
                    identity.environment = Some(*value);
                }
                if let Some(Cell::StrId(value)) = row.get("boot_id") {
                    identity.boot_id = Some(*value);
                }
                if let Some(Cell::Ts(value)) = row.get("btime") {
                    identity.boot_time = Some(*value);
                }
                true
            },
        )?;
    }
    Ok(running.then_some(identity))
}

#[cfg(feature = "posix")]
pub(super) fn last_stall_snapshot(
    segment: &Segment,
    metadata: Option<&MetadataProjection>,
) -> Result<Option<StallSnapshot>, ReaderError> {
    let mut last = None;
    visit_stall_snapshots(
        segment,
        metadata,
        || true,
        |snapshot| {
            last = Some(snapshot);
            true
        },
    )?;
    Ok(last)
}

pub(super) fn health_points(
    segment: &Segment,
    seed: Option<StallSnapshot>,
    metadata: Option<&MetadataProjection>,
) -> Result<Vec<HealthPoint>, ReaderError> {
    let mut points = Vec::new();
    visit_health_points_with_seed(
        segment,
        seed,
        metadata,
        || true,
        |point| {
            points.push(point);
            true
        },
    )?;
    Ok(points)
}
