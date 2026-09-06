//! Reusable input buffers for section decode.
//!
//! The Parquet reader borrows from owned `Bytes`, so callers still need a
//! temporary buffer while reading a section body. This pool reuses those input
//! buffers after the last `Bytes` reference is dropped.
//!
//! It only addresses the input buffer. Zstd output and Arrow arrays are the
//! decoded data itself; the pool does not touch them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use bytes::Bytes;

/// A bounded pool of reusable input buffers.
///
/// The pool keeps at most `max_buffers` idle buffers and frees any returned
/// buffer whose capacity exceeds `buffer_limit`. A borrowed buffer can still
/// grow while the caller writes into it; enforce input caps before decode.
///
/// `load` and `Loan::drop` briefly take a `std::sync::Mutex`; do not hold a
/// loan across `.await` without measuring contention.
#[derive(Clone, Debug)]
pub struct BytesPool {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    stats: Counters,
}

#[derive(Debug)]
struct State {
    idle: Vec<Vec<u8>>,
    max_buffers: usize,
    buffer_limit: usize,
}

/// Lifetime counters, kept outside the mutex so a metrics reader never grows or
/// contends the decode loop's critical section. `Relaxed` is enough: each is an
/// independent monotonic total, and a metrics snapshot needs no cross-counter
/// ordering.
#[derive(Debug, Default)]
#[allow(
    clippy::struct_field_names,
    reason = "the _total suffix is the counter-metric convention and mirrors the public PoolStats fields 1:1"
)]
struct Counters {
    loans_total: AtomicU64,
    returned_total: AtomicU64,
    dropped_oversize_total: AtomicU64,
    dropped_full_total: AtomicU64,
    poisoned_total: AtomicU64,
}

/// What happened to a buffer returned to the pool, so `Loan::drop` can count it
/// after releasing the lock.
enum Returned {
    /// Retained for reuse.
    Kept,
    /// Freed: capacity grew past `buffer_limit`.
    Oversize,
    /// Freed: `max_buffers` idle buffers were already parked.
    Full,
}

/// A snapshot of pool activity for metrics.
///
/// `idle` is the live count of parked buffers (a gauge); the `_total` fields are
/// monotonic lifetime counters. The reuse rate is `returned_total / loans_total`.
/// A nonzero-and-climbing `dropped_oversize_total` means `buffer_limit` is below
/// the section size, so the pool allocates per section;
/// `dropped_full_total` means `max_buffers` is the binding limit instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Idle buffers parked right now.
    pub idle: u64,
    /// Buffers handed out over the pool's lifetime.
    pub loans_total: u64,
    /// Returned buffers retained for reuse.
    pub returned_total: u64,
    /// Returns freed because the buffer grew past `buffer_limit`.
    pub dropped_oversize_total: u64,
    /// Returns freed because `max_buffers` idle buffers were already parked.
    pub dropped_full_total: u64,
    /// Times a `load`/return found the lock poisoned by a panicking holder. The
    /// pool recovers (it has no state invariant), so this is the only trace.
    pub poisoned_total: u64,
}

/// A buffer on loan from the pool. It returns to the pool after the last
/// `Bytes` reference is gone.
struct Loan {
    data: Vec<u8>,
    shared: Arc<Shared>,
}

impl AsRef<[u8]> for Loan {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for Loan {
    fn drop(&mut self) {
        let data = std::mem::take(&mut self.data);
        let counter = match self.shared.take_back(data) {
            Returned::Kept => &self.shared.stats.returned_total,
            Returned::Oversize => &self.shared.stats.dropped_oversize_total,
            Returned::Full => &self.shared.stats.dropped_full_total,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

impl Shared {
    /// Lock the state, counting a poisoned lock. The pool has no state invariant
    /// to uphold, so recovering is safe — but the count is the only signal that a
    /// holder panicked, which `stats()` would otherwise hide.
    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            self.stats.poisoned_total.fetch_add(1, Ordering::Relaxed);
            poisoned.into_inner()
        })
    }

    /// Take a returned buffer back under the lock, retaining it for reuse if it
    /// fits, and report the outcome. A freed buffer is dropped after the lock is
    /// released, not inside the critical section.
    fn take_back(&self, data: Vec<u8>) -> Returned {
        let mut state = self.lock_state();
        if data.capacity() > state.buffer_limit {
            drop(state);
            Returned::Oversize
        } else if state.idle.len() >= state.max_buffers {
            drop(state);
            Returned::Full
        } else {
            let mut data = data;
            data.clear();
            state.idle.push(data);
            Returned::Kept
        }
    }
}

impl BytesPool {
    /// A pool that keeps at most `max_buffers` idle buffers, each of capacity up
    /// to `buffer_limit` bytes.
    #[must_use]
    pub fn new(max_buffers: usize, buffer_limit: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    idle: Vec::new(),
                    max_buffers,
                    buffer_limit,
                }),
                stats: Counters::default(),
            }),
        }
    }

    /// Take a buffer, fill it via `fill`, and freeze it to `Bytes`.
    ///
    /// When the returned `Bytes` and every clone or slice of it are dropped, the
    /// buffer returns to the pool for reuse, so a steady loop does not allocate
    /// per call. The buffer handed to `fill` is empty but may already have spare
    /// capacity from a previous loan.
    #[must_use]
    pub fn load(&self, fill: impl FnOnce(&mut Vec<u8>)) -> Bytes {
        let mut data = self.shared.lock_state().idle.pop().unwrap_or_default();
        data.clear();
        fill(&mut data);
        self.shared
            .stats
            .loans_total
            .fetch_add(1, Ordering::Relaxed);
        Bytes::from_owner(Loan {
            data,
            shared: Arc::clone(&self.shared),
        })
    }

    /// Snapshot the pool's activity counters for metrics.
    ///
    /// The `_total` counters are lock-free atomics; `idle` is read under a brief
    /// lock (it is the parked-buffer list's length). The snapshot is not a single
    /// atomic instant, which is fine for metrics — independent counters need no
    /// mutual consistency.
    #[must_use]
    pub fn stats(&self) -> PoolStats {
        let idle = {
            let state = self.shared.lock_state();
            u64::try_from(state.idle.len()).unwrap_or(u64::MAX)
        };
        let counters = &self.shared.stats;
        PoolStats {
            idle,
            loans_total: counters.loans_total.load(Ordering::Relaxed),
            returned_total: counters.returned_total.load(Ordering::Relaxed),
            dropped_oversize_total: counters.dropped_oversize_total.load(Ordering::Relaxed),
            dropped_full_total: counters.dropped_full_total.load(Ordering::Relaxed),
            poisoned_total: counters.poisoned_total.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BytesPool;

    #[test]
    fn buffer_returns_on_drop_and_is_reused() {
        let pool = BytesPool::new(2, 1 << 20);
        assert_eq!(pool.stats().idle, 0);

        let first = pool.load(|buf| buf.extend_from_slice(&[1, 2, 3]));
        assert_eq!(&first[..], &[1, 2, 3]);
        assert_eq!(pool.stats().idle, 0, "in flight, not idle");
        drop(first);
        assert_eq!(pool.stats().idle, 1, "returned on drop");

        let second = pool.load(|buf| buf.extend_from_slice(&[4, 5]));
        assert_eq!(pool.stats().idle, 0, "reused the idle buffer, no new one");
        assert_eq!(&second[..], &[4, 5]);
    }

    #[test]
    fn idle_buffers_are_capped() {
        let pool = BytesPool::new(2, 1 << 20);
        let loans: Vec<_> = (0..4).map(|_| pool.load(|buf| buf.push(0))).collect();
        drop(loans);
        assert_eq!(pool.stats().idle, 2, "kept at most max_buffers");
    }

    #[test]
    fn oversized_buffers_are_not_retained() {
        let pool = BytesPool::new(4, 16);
        let big = pool.load(|buf| buf.extend(std::iter::repeat_n(0_u8, 1024)));
        drop(big);
        assert_eq!(
            pool.stats().idle,
            0,
            "a buffer above buffer_limit is freed, not pooled"
        );
    }

    #[test]
    fn a_live_clone_keeps_the_buffer_out_of_the_pool() {
        let pool = BytesPool::new(2, 1 << 20);
        let original = pool.load(|buf| buf.extend_from_slice(&[7, 8, 9]));
        let clone = original.clone();
        drop(original);
        assert_eq!(pool.stats().idle, 0, "still referenced by the clone");
        drop(clone);
        assert_eq!(
            pool.stats().idle,
            1,
            "returns only after the last reference"
        );
    }

    #[test]
    fn stats_report_loans_returns_and_drop_reasons() {
        let pool = BytesPool::new(1, 16);

        let a = pool.load(|buf| buf.extend_from_slice(&[1, 2]));
        let b = pool.load(|buf| buf.extend_from_slice(&[3, 4]));
        drop(a); // idle was empty -> retained
        drop(b); // idle already full (max_buffers = 1) -> dropped

        // Pops the retained buffer, then grows it past buffer_limit so its
        // return is freed as oversize rather than pooled.
        let big = pool.load(|buf| buf.extend(std::iter::repeat_n(0_u8, 64)));
        drop(big);

        let stats = pool.stats();
        assert_eq!(stats.loans_total, 3, "a, b, big");
        assert_eq!(stats.returned_total, 1, "only a was retained");
        assert_eq!(stats.dropped_full_total, 1, "b hit max_buffers");
        assert_eq!(
            stats.dropped_oversize_total, 1,
            "big grew past buffer_limit"
        );
        assert_eq!(stats.poisoned_total, 0, "no lock was poisoned");
    }
}
