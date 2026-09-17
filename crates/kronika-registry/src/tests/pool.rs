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
