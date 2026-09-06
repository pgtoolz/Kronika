//! The per-segment identity row.

use crate::buffering::buffer_row;
use crate::config::Config;
use crate::logging::{log_collection_failure, log_collection_finish, log_collection_start};
use anyhow::{Context, Result};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::{StrId, Ts};
use kronika_source_os::{OsInstanceFacts, collect_os_instance_facts};
use kronika_writer::{Interner, SectionBuffers};
use std::time::Instant;

const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_002;

/// Read the host identity for a new segment.
///
/// # Errors
///
/// Returns an error naming the `/proc` file that could not be read. The
/// identity is what makes a finished segment self-contained, so a failure here
/// is not a degraded section.
pub(crate) fn collect_instance() -> Result<OsInstanceFacts> {
    let started = Instant::now();
    log_collection_start(INSTANCE_METADATA_TYPE_ID, "procfs");
    match collect_os_instance_facts() {
        Ok(facts) => {
            log_collection_finish(INSTANCE_METADATA_TYPE_ID, "procfs", 1, started.elapsed());
            Ok(facts)
        }
        Err(err) => {
            log_collection_failure(INSTANCE_METADATA_TYPE_ID, "procfs", &err, started.elapsed());
            Err(err).context("collect OS instance facts")
        }
    }
}

/// Intern the identity strings and buffer the current `instance_metadata` row.
///
/// `in_container` is decided at collection time and stored, so nothing
/// downstream re-derives whether these numbers describe a machine or a
/// container.
///
/// # Errors
///
/// Returns an error if a string cannot be interned or the section buffer is
/// full.
pub(crate) fn push_instance_metadata(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    facts: &OsInstanceFacts,
    in_container: bool,
    config: &Config,
    ts: i64,
) -> Result<()> {
    let mut intern = |value: &str| -> Result<StrId> {
        interner
            .intern(value.as_bytes())
            .map(|id| StrId(id.get()))
            .map_err(|err| anyhow::anyhow!("intern instance metadata string: {err}"))
    };
    let row = InstanceMetadata {
        ts: Ts(ts),
        hostname: intern(&facts.hostname)?,
        kernel_version: intern(&facts.kernel_version)?,
        environment: Environment::from_container_flag(in_container).as_u8(),
        clock_ticks_per_sec: facts.clock_ticks_per_sec,
        page_size_bytes: facts.page_size_bytes,
        boot_id: intern(&facts.boot_id)?,
        btime: Ts(facts.btime),
        postgresql_enabled: !config.pg_dsns.is_empty(),
        postgresql_interval_seconds: effective_interval(config.intervals.pg, config.tick_secs),
        postgresql_effective_cpus: config.postgres_effective_cpus,
    };
    buffer_row(buffers, row)
}

const fn effective_interval(configured: u64, base_tick: u64) -> u64 {
    if configured == 0 {
        base_tick
    } else {
        configured
    }
}

#[cfg(test)]
mod tests {
    use super::effective_interval;

    #[test]
    fn zero_source_interval_uses_the_timer_tick_as_its_freshness() {
        assert_eq!(effective_interval(30, 5), 30);
        assert_eq!(effective_interval(0, 5), 5);
        assert_eq!(effective_interval(0, 0), 0);
    }
}
