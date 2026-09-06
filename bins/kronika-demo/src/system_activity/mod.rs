//! Default bounded CPU, memory, file, and loopback activity for demo histories.

mod config;
mod cpu;
mod loopback;
mod memory;
mod scratch;
mod waveform;

pub(crate) use config::SystemActivityConfig;

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Worker {
    name: &'static str,
    handle: JoinHandle<()>,
}

/// Owns every system worker and the exact scratch-file cleanup action.
pub(crate) struct SystemActivity {
    stop: Arc<AtomicBool>,
    workers: Vec<Worker>,
    scratch_cleanup: Option<scratch::ScratchCleanup>,
    stopped: bool,
}

impl SystemActivity {
    /// Start independent workers. A worker startup or runtime error is logged
    /// and does not stop the collector, `PostgreSQL` workload, or other workers.
    pub(crate) fn start(config: &SystemActivityConfig, stop: Arc<AtomicBool>) -> Self {
        println!(
            "kronika-demo: system workload started: 60s waveform, CPU peak {}% of one core, memory {} MiB, scratch {} MiB at {}, disk peak {} KiB/s read and write, loopback peak {} KiB/s, file flush {}s",
            config.cpu_percent,
            config.memory_mib,
            config.file_mib,
            config.directory.display(),
            config.disk_kib_per_s,
            config.network_kib_per_s,
            config.flush_interval_s,
        );
        println!(
            "kronika-demo: system workload hourly means: CPU {} ms, disk {} bytes each direction, loopback {} bytes each direction",
            waveform::hourly_cpu_millis(config.cpu_percent),
            waveform::hourly_payload_bytes(config.disk_kib_per_s),
            waveform::hourly_payload_bytes(config.network_kib_per_s),
        );

        let mut activity = Self {
            stop,
            workers: Vec::with_capacity(4),
            scratch_cleanup: None,
            stopped: false,
        };
        let cpu_percent = config.cpu_percent;
        let cpu_stop = Arc::clone(&activity.stop);
        spawn_worker(&mut activity.workers, "krn-demo-cpu", move || {
            cpu::run(cpu_percent, &cpu_stop);
            Ok(())
        });

        match config.memory_bytes() {
            Ok(memory_bytes) => {
                let memory_stop = Arc::clone(&activity.stop);
                spawn_worker(&mut activity.workers, "krn-demo-memory", move || {
                    memory::run(memory_bytes, &memory_stop)
                });
            }
            Err(error) => {
                eprintln!("kronika-demo: system memory worker could not start: {error:#}");
            }
        }

        match scratch::prepare(config) {
            Ok((ring, cleanup)) => {
                activity.scratch_cleanup = Some(cleanup);
                let disk_stop = Arc::clone(&activity.stop);
                let disk_kib_per_s = config.disk_kib_per_s;
                let flush_interval_s = config.flush_interval_s;
                spawn_worker(&mut activity.workers, "krn-demo-disk", move || {
                    scratch::run(ring, disk_kib_per_s, flush_interval_s, &disk_stop)
                });
            }
            Err(error) => {
                eprintln!("kronika-demo: system disk worker could not start: {error:#}");
            }
        }

        let network_kib_per_s = config.network_kib_per_s;
        let network_stop = Arc::clone(&activity.stop);
        spawn_worker(&mut activity.workers, "krn-demo-loop", move || {
            loopback::run(network_kib_per_s, &network_stop)
        });
        activity
    }

    /// Stop every worker, wait for it, and remove the owned scratch file.
    pub(crate) fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.stop.store(true, Ordering::SeqCst);
        for worker in &self.workers {
            worker.handle.thread().unpark();
        }
        for worker in self.workers.drain(..) {
            if worker.handle.join().is_err() {
                eprintln!(
                    "kronika-demo: system {} worker panicked during shutdown",
                    worker.name
                );
            }
        }
        if let Some(mut cleanup) = self.scratch_cleanup.take()
            && let Err(error) = cleanup.cleanup()
        {
            eprintln!("kronika-demo: could not remove the system workload scratch file: {error:#}");
        }
        println!("kronika-demo: system workload stopped");
    }
}

impl Drop for SystemActivity {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spawn_worker<F>(workers: &mut Vec<Worker>, name: &'static str, run: F)
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    match thread::Builder::new().name(name.to_owned()).spawn(move || {
        if let Err(error) = run() {
            eprintln!("kronika-demo: system {name} worker stopped early: {error:#}");
        }
    }) {
        Ok(handle) => workers.push(Worker { name, handle }),
        Err(error) => eprintln!("kronika-demo: system {name} worker could not start: {error}"),
    }
}

fn wait_for(stop: &AtomicBool, duration: Duration) -> bool {
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let remaining = duration.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return false;
        }
        thread::park_timeout(remaining);
    }
    true
}

#[cfg(test)]
mod tests;
