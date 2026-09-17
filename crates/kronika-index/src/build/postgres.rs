//! `PostgreSQL` activity, transaction, and combined health series.

use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, ReaderError, Resolved, Segment};

use super::{ActiveBackendSample, BuildError, HealthMetadata};

use crate::cpu_capacity::RecordedCpuCapacity;
use crate::health::{SourcePenalty, overall_health, postgres_penalty};
use crate::series::{ActiveBackendPoint, HealthPoint, TransactionPoint};

pub(super) fn combined_active_points(
    activity: &BTreeMap<u32, Vec<ActiveBackendPoint>>,
) -> Vec<(i64, Option<u32>)> {
    let mut counts = BTreeMap::<i64, Option<u32>>::new();
    for points in activity.values() {
        for point in points {
            counts
                .entry(point.timestamp)
                .and_modify(|count| *count = None)
                .or_insert(Some(point.count));
        }
    }
    counts.into_iter().collect()
}

pub(super) fn postgres_health_points(
    metadata: &HealthMetadata,
    active: &[(i64, Option<u32>)],
    capacity: &RecordedCpuCapacity,
) -> Option<Vec<HealthPoint>> {
    if metadata.postgresql_enabled != Some(true) {
        return None;
    }
    if active.is_empty() {
        return Some(vec![HealthPoint {
            timestamp: metadata.timestamp,
            value: None,
        }]);
    }
    Some(
        active
            .iter()
            .map(|(timestamp, active)| HealthPoint {
                timestamp: *timestamp,
                value: active
                    .zip(capacity.at(*timestamp))
                    .and_then(|(active, cpus)| postgres_penalty(active, cpus))
                    .map(|penalty| 100_u8.saturating_sub(penalty)),
            })
            .collect(),
    )
}

pub(super) fn overall_points(
    os: &[HealthPoint],
    postgres: Option<&[HealthPoint]>,
    predecessor_postgres: Option<HealthPoint>,
    metadata: &HealthMetadata,
) -> Vec<HealthPoint> {
    if metadata.os_enabled == Some(false) {
        return postgres.map_or_else(
            || {
                vec![HealthPoint {
                    timestamp: metadata.timestamp,
                    value: None,
                }]
            },
            <[HealthPoint]>::to_vec,
        );
    }
    let postgres_interval = metadata
        .postgresql_interval_seconds
        .checked_mul(1_000_000)
        .and_then(|value| i64::try_from(value).ok());
    os.iter()
        .map(|point| {
            let postgres_penalty = match metadata.postgresql_enabled {
                Some(false) => SourcePenalty::Disabled,
                Some(true) => {
                    latest_postgres_point(postgres, predecessor_postgres, point.timestamp)
                        .filter(|candidate| {
                            postgres_interval.is_some_and(|interval| {
                                point.timestamp.saturating_sub(candidate.timestamp) <= interval
                            })
                        })
                        .and_then(|candidate| candidate.value)
                        .map_or(SourcePenalty::Unknown, |health| {
                            SourcePenalty::Known(100_u8.saturating_sub(health))
                        })
                }
                None => SourcePenalty::Unknown,
            };
            HealthPoint {
                timestamp: point.timestamp,
                value: if metadata.os_enabled.is_none()
                    && metadata.postgresql_enabled != Some(false)
                {
                    None
                } else {
                    overall_health(point.value, postgres_penalty)
                },
            }
        })
        .collect()
}

fn latest_postgres_point(
    current: Option<&[HealthPoint]>,
    predecessor: Option<HealthPoint>,
    timestamp: i64,
) -> Option<HealthPoint> {
    current
        .and_then(|points| {
            points
                .iter()
                .rev()
                .find(|candidate| candidate.timestamp <= timestamp)
                .copied()
        })
        .or_else(|| predecessor.filter(|candidate| candidate.timestamp <= timestamp))
}

pub(super) fn transaction_points(
    segment: &Segment,
    type_id: u32,
) -> Result<Vec<TransactionPoint>, ReaderError> {
    let mut previous: BTreeMap<u32, (i64, i128)> = BTreeMap::new();
    let mut points = Vec::new();
    segment.visit_rows(
        type_id,
        &["ts", "datid", "xact_commit", "xact_rollback"],
        0,
        usize::MAX,
        |_ordinal, row| {
            let (
                Some(Cell::Ts(timestamp)),
                Some(Cell::U32(datid)),
                Some(Cell::I64(commit)),
                Some(Cell::I64(rollback)),
            ) = (
                row.get("ts"),
                row.get("datid"),
                row.get("xact_commit"),
                row.get("xact_rollback"),
            )
            else {
                return true;
            };
            let total = i128::from(*commit) + i128::from(*rollback);
            let value = previous.get(datid).and_then(|(before_ts, before)| {
                transaction_rate(*before_ts, *before, *timestamp, total)
            });
            previous.insert(*datid, (*timestamp, total));
            points.push(TransactionPoint {
                timestamp: *timestamp,
                datid: *datid,
                value,
            });
            true
        },
    )?;
    points.sort_by_key(|point| (point.datid, point.timestamp));
    Ok(points)
}

pub(super) fn transaction_rate(
    before_ts: i64,
    before: i128,
    timestamp: i64,
    total: i128,
) -> Option<f64> {
    let elapsed = timestamp.checked_sub(before_ts)?;
    let delta = total.checked_sub(before)?;
    if elapsed <= 0 || delta < 0 {
        return None;
    }
    let rate = integer_as_f64(delta)? * 1_000_000.0 / integer_as_f64(i128::from(elapsed))?;
    (rate.is_finite() && rate >= 0.0).then_some(rate)
}

fn integer_as_f64(value: i128) -> Option<f64> {
    let mut value = u128::try_from(value).ok()?;
    let mut words = [0_u32; 4];
    for word in words.iter_mut().rev() {
        *word = u32::try_from(value & u128::from(u32::MAX)).ok()?;
        value >>= 32;
    }
    Some(words.into_iter().fold(0.0, |number, word| {
        number.mul_add(4_294_967_296.0, f64::from(word))
    }))
}

pub(super) fn active_backend_points(samples: &[ActiveBackendSample]) -> Vec<ActiveBackendPoint> {
    samples
        .iter()
        .map(|sample| ActiveBackendPoint {
            timestamp: sample.timestamp,
            count: sample.count,
        })
        .collect()
}

pub(super) fn active_backend_samples(
    segment: &Segment,
    type_id: u32,
) -> Result<Vec<ActiveBackendSample>, BuildError> {
    let mut ids = HashSet::new();
    let mut samples = Vec::new();
    segment.visit_rows(type_id, &["ts", "state"], 0, usize::MAX, |ordinal, row| {
        let Some(Cell::Ts(timestamp)) = row.get("ts") else {
            return true;
        };
        let state = match row.get("state") {
            Some(Cell::StrId(id)) => {
                ids.insert(*id);
                Some(*id)
            }
            _ => None,
        };
        samples.push((*timestamp, state, u32::try_from(ordinal).ok()));
        true
    })?;
    let dictionary = segment.dictionary_for(&ids)?;
    let mut active_ids = HashSet::new();
    for id in ids {
        match dictionary.resolve(id) {
            Some(Resolved::Str(b"active")) => {
                active_ids.insert(id);
            }
            Some(Resolved::Str(_) | Resolved::Blob(_)) => {}
            None => return Err(BuildError::UnresolvedState(id)),
        }
    }

    let mut counts = BTreeMap::<i64, (Option<u32>, u32)>::new();
    for (timestamp, state, ordinal) in samples {
        let sample = counts.entry(timestamp).or_default();
        if state.is_some_and(|id| active_ids.contains(&id)) {
            sample.0 = sample.0.or(ordinal);
            sample.1 = sample.1.saturating_add(1);
        }
    }
    Ok(counts
        .into_iter()
        .map(
            |(timestamp, (first_active_ordinal, count))| ActiveBackendSample {
                timestamp,
                first_active_ordinal,
                count,
            },
        )
        .collect())
}
