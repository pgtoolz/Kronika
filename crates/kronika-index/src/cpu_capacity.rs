use std::collections::{BTreeMap, BTreeSet};

use kronika_reader::{Cell, ReaderError, Segment};
use kronika_registry::instance_metadata::Environment;

const OS_CPU: u32 = 1_102_001;
const OS_CGROUP_CONTEXT: u32 = 1_205_001;

/// Effective cgroup CPU capacity in cores from the recorded cpuset and
/// hierarchical quota/period. An unknown quota remains unknown; `-1` uses the
/// positive cpuset alone.
#[must_use]
pub fn cgroup_cpu_capacity(
    cpuset: Option<i64>,
    quota: Option<i64>,
    period: Option<i64>,
) -> Option<f64> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "recorded CPU counts fit f64 capacity"
    )]
    let cpuset = cpuset.filter(|cpus| *cpus > 0).map(|cpus| cpus as f64);
    match (quota, period) {
        (Some(-1), _) => cpuset,
        (Some(quota), Some(period)) if quota > 0 && period > 0 => {
            #[expect(clippy::cast_precision_loss, reason = "microseconds of one period")]
            let cores = quota as f64 / period as f64;
            Some(cpuset.map_or(cores, |cpus| cores.min(cpus)))
        }
        _ => None,
    }
}

#[derive(Debug, Default)]
pub(crate) struct RecordedCpuCapacity {
    explicit: Option<f64>,
    snapshots: BTreeMap<i64, Option<f64>>,
}

impl RecordedCpuCapacity {
    #[cfg(test)]
    pub(crate) fn fixed(cpus: f64) -> Self {
        Self {
            explicit: Some(cpus),
            ..Self::default()
        }
    }

    pub(crate) fn read(
        segment: &Segment,
        environment: Option<u32>,
        explicit: Option<u32>,
    ) -> Result<Self, ReaderError> {
        let mut capacity = Self {
            explicit: explicit.filter(|cpus| *cpus > 0).map(f64::from),
            snapshots: BTreeMap::new(),
        };
        if capacity.explicit.is_some() {
            return Ok(capacity);
        }
        match environment {
            Some(value) if value == u32::from(Environment::Machine.as_u8()) => {
                capacity.read_machine(segment)?;
            }
            Some(value) if value == u32::from(Environment::Container.as_u8()) => {
                capacity.read_container(segment)?;
            }
            _ => {}
        }
        Ok(capacity)
    }

    pub(crate) fn at(&self, timestamp: i64) -> Option<f64> {
        self.explicit.or_else(|| {
            self.snapshots
                .range(..=timestamp)
                .next_back()
                .and_then(|(_, capacity)| *capacity)
        })
    }

    #[cfg(feature = "posix")]
    pub(crate) fn last_snapshot(&self) -> Option<(i64, Option<f64>)> {
        self.snapshots
            .last_key_value()
            .map(|(ts, cpus)| (*ts, *cpus))
    }

    pub(crate) fn seed(&mut self, timestamp: i64, cpus: Option<f64>) {
        self.snapshots.entry(timestamp).or_insert(cpus);
    }

    fn read_machine(&mut self, segment: &Segment) -> Result<(), ReaderError> {
        if segment.rows_of(OS_CPU).is_none() {
            return Ok(());
        }
        let mut snapshots = BTreeMap::<i64, BTreeSet<i32>>::new();
        segment.visit_rows(
            OS_CPU,
            &["ts", "cpu_id", "scope"],
            0,
            usize::MAX,
            |_, row| {
                if let (Some(Cell::Ts(ts)), Some(Cell::I32(cpu)), Some(Cell::U32(0))) =
                    (row.get("ts"), row.get("cpu_id"), row.get("scope"))
                {
                    let cpus = snapshots.entry(*ts).or_default();
                    if *cpu >= 0 {
                        cpus.insert(*cpu);
                    }
                }
                true
            },
        )?;
        self.snapshots = snapshots
            .into_iter()
            .map(|(ts, cpus)| {
                let capacity = u32::try_from(cpus.len()).ok().filter(|count| *count > 0);
                (ts, capacity.map(f64::from))
            })
            .collect();
        Ok(())
    }

    fn read_container(&mut self, segment: &Segment) -> Result<(), ReaderError> {
        if segment.rows_of(OS_CGROUP_CONTEXT).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            OS_CGROUP_CONTEXT,
            &[
                "ts",
                "cpuset_cpus",
                "effective_cpu_quota_usec",
                "effective_cpu_period_usec",
                "scope",
            ],
            0,
            usize::MAX,
            |_, row| {
                if let Some(Cell::Ts(ts)) = row.get("ts") {
                    let capacity = matches!(row.get("scope"), Some(Cell::U32(1 | 3)))
                        .then(|| {
                            cgroup_cpu_capacity(
                                optional_i64(row.get("cpuset_cpus")),
                                optional_i64(row.get("effective_cpu_quota_usec")),
                                optional_i64(row.get("effective_cpu_period_usec")),
                            )
                        })
                        .flatten();
                    self.snapshots.insert(*ts, capacity);
                }
                true
            },
        )?;
        Ok(())
    }
}

const fn optional_i64(cell: Option<&Cell>) -> Option<i64> {
    match cell {
        Some(Cell::I64(value)) => Some(*value),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
