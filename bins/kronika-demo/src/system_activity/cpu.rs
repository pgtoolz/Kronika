//! Bounded CPU duty-cycle worker.

use super::{wait_for, waveform};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub(super) fn run(peak_percent: u64, stop: &Arc<AtomicBool>) {
    let started = Instant::now();
    let mut state = 0x4b52_4f4e_494b_4100_u64;
    while !stop.load(Ordering::Relaxed) {
        let frame_started = Instant::now();
        let busy_for = waveform::cpu_busy_time(peak_percent, started.elapsed());
        while frame_started.elapsed() < busy_for && !stop.load(Ordering::Relaxed) {
            for _ in 0..256 {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
            }
            black_box(state);
        }
        let rest = waveform::CPU_FRAME.saturating_sub(frame_started.elapsed());
        if wait_for(stop, rest) {
            break;
        }
    }
}
