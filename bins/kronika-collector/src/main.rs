//! Linux system collector daemon.
//!
//! CLI arguments override environment settings; --storage-dir (or
//! `KRONIKA_STORAGE_DIR`) is required. The process snapshots the OS sources on their own
//! intervals, appends each synchronized window to `<storage>/active.wal`, and
//! publishes immutable `<storage>/YYYY/MM/DD/<segment-id>.zms` segments by size,
//! age, journal pressure, or `SIGUSR2`.
//!
//! `SIGTERM` and `SIGINT` stop the loop without discarding the journal.
//! Startup recovery writes valid frames left by the preceding process. A failed
//! source is logged and retried on its next interval; bad startup
//! configuration, journal-open failures, and any persistence failure that
//! poisons the journal terminate the process.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(target_env = "musl")]
#[allow(
    unsafe_code,
    reason = "jemalloc reads this C configuration pointer before main"
)]
// This replaces jemalloc's unprefixed weak malloc_conf pointer.
#[unsafe(export_name = "malloc_conf")]
static JEMALLOC_CONF: Option<&std::ffi::c_char> =
    // SAFETY: The C literal is non-null, NUL-terminated, and has static storage.
    Some(unsafe { &*c"thp:never".as_ptr() });

mod buffering;
mod cgroup_discovery;
mod clock;
mod collector;
mod config;
mod filesystem_capacity;
mod help;
mod instance_metadata;
mod log_sources;
mod logging;
mod os_sources;
mod pg_sources;
mod prometheus;
mod rotation;
mod scheduler;
mod segments;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    if filesystem_capacity::is_helper_invocation() {
        anyhow::ensure!(
            std::env::args_os().len() == 2,
            "unexpected statvfs helper arguments"
        );
        return filesystem_capacity::run_helper();
    }
    let config = config::parse_from(std::env::args_os()).unwrap_or_else(|error| error.exit());
    config::install(config)?;
    let config = config::get();
    logging::configure(config.log_level);
    logging::configure_process_diagnostics(config.mode.collect_os());
    config.log_storage_settings();
    // Tokio defaults to one worker per available CPU. Keep the pool small even
    // on large hosts, such as 96-core Kubernetes nodes.
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("initialize collector runtime")?
        .block_on(collector::run())
}

#[cfg(test)]
mod tests;
