//! Fixed anonymous working set kept resident by deterministic page touches.

use super::wait_for;
use anyhow::{Context, Result};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const TOUCH_INTERVAL: Duration = Duration::from_secs(1);

pub(super) fn run(bytes: usize, stop: &Arc<AtomicBool>) -> Result<()> {
    let mut memory = Vec::new();
    memory
        .try_reserve_exact(bytes)
        .context("reserve the anonymous working set")?;
    memory.resize(bytes, 0_u8);
    let page_size = rustix::param::page_size().max(1);
    let mut generation = 1_u8;
    while !stop.load(Ordering::Relaxed) {
        let mut checksum = 0_u8;
        for page in memory.chunks_mut(page_size) {
            if let Some(first) = page.first_mut() {
                *first = first.wrapping_add(generation);
                checksum ^= *first;
            }
        }
        black_box(checksum);
        generation = generation.wrapping_add(1);
        if wait_for(stop, TOUCH_INTERVAL) {
            break;
        }
    }
    Ok(())
}
