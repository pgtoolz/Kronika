//! Watches for `SIGTERM`/`SIGINT` so a `KRONIKA_DEMO_DURATION_S=0` run can be
//! stopped from outside instead of running forever.
//!
//! `nix::sys::signal::signal` would do this with a raw handler, but
//! registering one is `unsafe`, and this workspace denies `unsafe_code`.
//! `tokio::signal::unix::signal` does the same job through a safe API, so the
//! watch runs on its own small runtime instead.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tokio::runtime::Builder;
use tokio::signal::unix::{SignalKind, signal};

/// Spawn a background thread that flips the returned flag to `true` on the
/// first `SIGTERM` or `SIGINT`.
///
/// # Errors
///
/// Returns an error when the watch thread cannot be started.
pub(crate) fn watch() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    thread::Builder::new()
        .name("kronika-demo-signals".to_owned())
        .spawn(move || run(&flag))
        .context("spawn the signal-watch thread")?;
    Ok(stop)
}

fn run(flag: &Arc<AtomicBool>) {
    let Ok(runtime) = Builder::new_current_thread().enable_io().build() else {
        eprintln!("kronika-demo: cannot build the signal-watch runtime");
        return;
    };
    runtime.block_on(async {
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            eprintln!("kronika-demo: cannot watch SIGTERM");
            return;
        };
        let Ok(mut int) = signal(SignalKind::interrupt()) else {
            eprintln!("kronika-demo: cannot watch SIGINT");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    });
    flag.store(true, Ordering::SeqCst);
}
