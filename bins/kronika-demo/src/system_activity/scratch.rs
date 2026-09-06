//! Fixed-size positional I/O in one owned scratch file.

use super::config::{DIRECTORY_ENV, SystemActivityConfig, paths_are_separate};
use super::{wait_for, waveform};
use anyhow::{Context, Result};
use rustix::fs::{Advice, fadvise};
use std::fs::{File, OpenOptions};
use std::hint::black_box;
use std::io::ErrorKind;
use std::num::NonZeroU64;
use std::os::unix::fs::{FileExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub(super) const SCRATCH_FILE_NAME: &str = "kronika-demo-system-activity.bin";

pub(super) struct ScratchCleanup {
    path: Option<PathBuf>,
}

impl ScratchCleanup {
    pub(super) fn cleanup(&mut self) -> Result<()> {
        let Some(path) = self.path.take() else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        self.path.as_deref().unwrap_or_else(|| Path::new(""))
    }
}

impl Drop for ScratchCleanup {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("kronika-demo: could not remove the system workload scratch file: {error:#}");
        }
    }
}

pub(super) struct ScratchRing {
    file: File,
    length: u64,
    cursor: u64,
    pending_start: u64,
    pending_bytes: u64,
    byte_carry: u64,
    write_buffer: Vec<u8>,
    read_buffer: Vec<u8>,
    sequence: u8,
}

impl ScratchRing {
    fn write_budgeted(&mut self, budget_bytes: u64) -> Result<()> {
        self.byte_carry = self
            .byte_carry
            .checked_add(budget_bytes)
            .context("the scratch write budget overflowed u64")?;
        let page_bytes = u64::try_from(self.write_buffer.len()).context("page size exceeds u64")?;
        let write_bytes = self.byte_carry / page_bytes * page_bytes;
        self.byte_carry %= page_bytes;
        self.write_pages(write_bytes)
    }

    fn write_pages(&mut self, bytes: u64) -> Result<()> {
        if bytes == 0 {
            return Ok(());
        }
        anyhow::ensure!(
            self.pending_bytes.saturating_add(bytes) <= self.length,
            "writes between file flushes exceed the scratch ring size"
        );
        if self.pending_bytes == 0 {
            self.pending_start = self.cursor;
        }
        let page_bytes = u64::try_from(self.write_buffer.len()).context("page size exceeds u64")?;
        let mut remaining = bytes;
        while remaining > 0 {
            self.write_buffer.fill(self.sequence);
            self.file
                .write_all_at(&self.write_buffer, self.cursor)
                .context("write the system workload scratch ring")?;
            self.sequence = self.sequence.wrapping_add(1);
            self.cursor = (self.cursor + page_bytes) % self.length;
            self.pending_bytes += page_bytes;
            remaining -= page_bytes;
        }
        Ok(())
    }

    fn sync_and_read(&mut self) -> Result<()> {
        if self.pending_bytes == 0 {
            return Ok(());
        }
        self.file
            .sync_data()
            .context("flush the system workload scratch file")?;
        let first = self.pending_bytes.min(self.length - self.pending_start);
        let second = self.pending_bytes - first;
        for (offset, length) in [(self.pending_start, first), (0, second)] {
            let Some(length) = NonZeroU64::new(length) else {
                continue;
            };
            fadvise(&self.file, offset, Some(length), Advice::DontNeed)
                .context("drop flushed scratch pages before reading")?;
        }
        let mut checksum = 0_u8;
        for (offset, length) in [(self.pending_start, first), (0, second)] {
            let mut read_at = offset;
            let mut remaining = length;
            while remaining > 0 {
                let buffer_len = u64::try_from(self.read_buffer.len())
                    .context("page size exceeds u64")?
                    .min(remaining);
                let buffer_len = usize::try_from(buffer_len).context("read size exceeds usize")?;
                self.file
                    .read_exact_at(&mut self.read_buffer[..buffer_len], read_at)
                    .context("read the flushed system workload scratch ring")?;
                if let Some(first) = self.read_buffer.first() {
                    checksum ^= *first;
                }
                let advanced = u64::try_from(buffer_len).context("read size exceeds u64")?;
                read_at += advanced;
                remaining -= advanced;
            }
        }
        black_box(checksum);
        self.pending_bytes = 0;
        Ok(())
    }
}

pub(super) fn prepare(config: &SystemActivityConfig) -> Result<(ScratchRing, ScratchCleanup)> {
    std::fs::create_dir_all(&config.directory)
        .with_context(|| format!("create {DIRECTORY_ENV} {}", config.directory.display()))?;
    let directory = std::fs::canonicalize(&config.directory)
        .with_context(|| format!("resolve {DIRECTORY_ENV} {}", config.directory.display()))?;
    let storage = std::fs::canonicalize(&config.storage_directory).with_context(|| {
        format!(
            "resolve KRONIKA_STORAGE_DIR {}",
            config.storage_directory.display()
        )
    })?;
    anyhow::ensure!(
        paths_are_separate(&directory, &storage),
        "{DIRECTORY_ENV} must resolve outside KRONIKA_STORAGE_DIR"
    );

    let path = directory.join(SCRATCH_FILE_NAME);
    reclaim_stale_file(&path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("create {}", path.display()))?;
    let length = config.file_bytes()?;
    let page_size = rustix::param::page_size().max(1);
    let page_bytes = u64::try_from(page_size).context("page size exceeds u64")?;
    anyhow::ensure!(
        length >= page_bytes && length.is_multiple_of(page_bytes),
        "the scratch ring size is not a multiple of the operating-system page size"
    );
    file.set_len(length)
        .with_context(|| format!("size {}", path.display()))?;

    Ok((
        ScratchRing {
            file,
            length,
            cursor: 0,
            pending_start: 0,
            pending_bytes: 0,
            byte_carry: 0,
            write_buffer: vec![0_u8; page_size],
            read_buffer: vec![0_u8; page_size],
            sequence: 1,
        },
        ScratchCleanup { path: Some(path) },
    ))
}

fn reclaim_stale_file(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_file(),
                "the system workload scratch path {} is not a regular file",
                path.display()
            );
            std::fs::remove_file(path)
                .with_context(|| format!("remove stale scratch file {}", path.display()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

pub(super) fn run(
    mut ring: ScratchRing,
    disk_kib_per_s: u64,
    flush_interval_s: u64,
    stop: &Arc<AtomicBool>,
) -> Result<()> {
    let started = Instant::now();
    let ticks_per_flush = flush_interval_s.saturating_mul(4);
    let mut ticks = 0_u64;
    while !stop.load(Ordering::Relaxed) {
        let tick_started = Instant::now();
        let bytes = waveform::payload_bytes_for_tick(disk_kib_per_s, started.elapsed());
        ring.write_budgeted(bytes)?;
        ticks += 1;
        if ticks.is_multiple_of(ticks_per_flush) {
            ring.sync_and_read()?;
        }
        let rest = waveform::WORKER_TICK.saturating_sub(tick_started.elapsed());
        if wait_for(stop, rest) {
            break;
        }
    }
    ring.sync_and_read()
}

#[cfg(test)]
mod tests;
