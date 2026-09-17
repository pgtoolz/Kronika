//! Write `instance_metadata` when a segment opens.
//!
//! Readers use its Linux counter units, boot identity, collection mode and
//! `PostgreSQL` intervals to interpret the recorded rows. PostgreSQL-only mode
//! leaves all Linux fields null.

use std::time::Instant;

use anyhow::{Context, Result};
use kronika_registry::instance_metadata::{Environment, InstanceMetadataV4};
use kronika_registry::{Section, StrId, Ts};
use kronika_source_os::{OsInstanceFacts, collect_os_instance_facts};
use kronika_writer::{Interner, SectionBuffers};

use crate::buffering::buffer_row;
use crate::config::Config;
use crate::logging::{log_collection_failure, log_collection_finish, log_collection_start};

/// Buffer the new segment's metadata using its own string dictionary.
///
/// # Errors
///
/// Returns an error if host facts cannot be read in local mode, a string cannot
/// be interned, or the section buffer is full.
pub(crate) fn push_instance_metadata(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    in_container: bool,
    config: &Config,
    ts: i64,
) -> Result<()> {
    let os_enabled = config.mode.collect_os();
    let mut row = InstanceMetadataV4 {
        ts: Ts(ts),
        hostname: None,
        kernel_version: None,
        environment: None,
        clock_ticks_per_sec: None,
        page_size_bytes: None,
        boot_id: None,
        btime: None,
        os_enabled,
        postgresql_processes_shared: os_enabled && !in_container,
        postgresql_enabled: config.pg_dsn.is_some(),
        // Health freshness and VACUUM episodes use the normal activity cadence.
        postgresql_interval_seconds: recorded_interval_seconds(
            config.intervals.pg_activity,
            config.tick_secs,
        ),
        postgresql_effective_cpus: config.postgres_effective_cpus,
        postgresql_instance_interval_seconds: recorded_interval_seconds(
            config.intervals.pg_instance,
            config.tick_secs,
        ),
        postgresql_relations_interval_seconds: recorded_interval_seconds(
            config.intervals.pg_tables_and_indexes,
            config.tick_secs,
        ),
        postgresql_statements_interval_seconds: recorded_interval_seconds(
            config.intervals.pg_statements_and_plans,
            config.tick_secs,
        ),
    };
    if os_enabled {
        let facts = read_linux_facts()?;
        let mut intern = |value: &str| -> Result<StrId> {
            interner
                .intern(value.as_bytes())
                .map(|id| StrId(id.get()))
                .map_err(|err| anyhow::anyhow!("intern instance metadata string: {err}"))
        };
        row.hostname = Some(intern(&facts.hostname)?);
        row.kernel_version = Some(intern(&facts.kernel_version)?);
        row.boot_id = Some(intern(&facts.boot_id)?);
        row.environment = Some(Environment::from_container_flag(in_container).as_u8());
        row.clock_ticks_per_sec = Some(facts.clock_ticks_per_sec);
        row.page_size_bytes = Some(facts.page_size_bytes);
        row.btime = Some(Ts(facts.btime));
    }
    buffer_row(buffers, row)
}

/// Read Linux identity and counter units; failure prevents opening the segment.
fn read_linux_facts() -> Result<OsInstanceFacts> {
    let type_id = InstanceMetadataV4::CONTRACT.type_id.get();
    let started = Instant::now();
    log_collection_start(type_id, "procfs");
    match collect_os_instance_facts() {
        Ok(facts) => {
            log_collection_finish(type_id, "procfs", 1, started.elapsed());
            Ok(facts)
        }
        Err(err) => {
            log_collection_failure(type_id, "procfs", &err, started.elapsed());
            Err(err).context("collect OS instance facts")
        }
    }
}

/// A zero source interval reads on each timer tick; zero for both means signals only.
const fn recorded_interval_seconds(configured: u64, base_tick: u64) -> u64 {
    if configured == 0 {
        base_tick
    } else {
        configured
    }
}

#[cfg(test)]
#[path = "tests/instance_metadata.rs"]
mod tests;
