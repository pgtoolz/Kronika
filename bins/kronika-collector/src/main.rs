//! Linux system collector daemon.
//!
//! Configuration is environment-only; the one required variable is
//! `KRONIKA_STORAGE_DIR`. The process snapshots the OS sources on their own
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
mod rotation;
mod scheduler;
mod segments;

use anyhow::{Context, Result};
use config::Config;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    if let Some(argument) = arguments.next() {
        anyhow::ensure!(
            arguments.next().is_none(),
            "unexpected arguments; use kronika-collector --help"
        );
        if argument == "--help" || argument == "-h" {
            print!("{}", help::HELP);
            return Ok(());
        }
        if argument == "--version" {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        if filesystem_capacity::is_helper_invocation() {
            return filesystem_capacity::run_helper();
        }
        anyhow::bail!(
            "unexpected argument {}; use kronika-collector --help",
            argument.display()
        );
    }
    let config = Config::from_env()?;
    logging::configure_process_diagnostics(config.mode.collect_os());
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    if !config.mode.collect_os() {
        runtime.worker_threads(1);
    }
    runtime
        .enable_all()
        .build()
        .context("initialize collector runtime")?
        .block_on(collector::run(config))
}

#[cfg(test)]
mod tests;
