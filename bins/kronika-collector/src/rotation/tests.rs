use super::*;
use kronika_layout::{FileIdentity, SegmentArtifacts, SegmentId};

const MICROS_PER_DAY: i64 = 86_400_000_000;

fn address(id: i64) -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(id).expect("valid segment id")).expect("valid address")
}

fn segment(id: i64, zms_bytes: u64, idx_bytes: Option<u64>) -> SegmentArtifacts {
    SegmentArtifacts {
        address: address(id),
        zms_identity: FileIdentity {
            device: 1,
            inode: u64::try_from(id).unwrap_or(0),
            len: zms_bytes,
            mtime_seconds: 0,
            mtime_nanoseconds: 0,
            ctime_seconds: 0,
            ctime_nanoseconds: 0,
        },
        zms_bytes,
        idx_bytes,
    }
}

fn snapshot(segments: Vec<SegmentArtifacts>, orphans: Vec<SegmentAddress>) -> LayoutSnapshot {
    LayoutSnapshot {
        days: Vec::new(),
        segments,
        orphan_indexs: orphans,
        temporaries: Vec::new(),
        foreign_entries: Vec::new(),
        visited_entries: 0,
        metadata_bytes: 0,
    }
}

#[test]
fn deficit_is_exact_at_the_percentage_boundary() {
    // 80 of 100 is exactly the 80% threshold: no deficit, one more byte
    // creates one.
    let threshold = used_threshold_bytes(100, 80);
    assert_eq!(80_u64.saturating_sub(threshold), 0);
    assert_eq!(81_u64.saturating_sub(threshold), 1);
    assert_eq!(
        0_u64.saturating_sub(used_threshold_bytes(0, 80)),
        0,
        "an empty partition never has a deficit"
    );
    assert_eq!(
        150_u64.saturating_sub(used_threshold_bytes(100, 99)),
        51,
        "statvfs reporting used above total still yields a full deficit"
    );
}

#[test]
fn reconcile_pending_reclaim_credits_only_observed_drops() {
    assert_eq!(
        reconcile_pending_reclaim(100, None, 500),
        100,
        "the first observation has no baseline to credit against"
    );
    assert_eq!(
        reconcile_pending_reclaim(100, Some(500), 470),
        70,
        "a 30-byte physical drop shrinks the pending figure"
    );
    assert_eq!(
        reconcile_pending_reclaim(100, Some(500), 300),
        0,
        "a drop larger than pending clears it without wrapping"
    );
    assert_eq!(
        reconcile_pending_reclaim(100, Some(500), 600),
        100,
        "foreign growth never inflates pending reclaim"
    );
}

#[test]
fn used_threshold_is_the_percent_of_total() {
    assert_eq!(used_threshold_bytes(1000, 80), 800);
    assert_eq!(used_threshold_bytes(0, 80), 0);
    assert_eq!(
        used_threshold_bytes(199, 80),
        159,
        "the threshold rounds down"
    );
    let expected = u64::try_from(u128::from(u64::MAX) * 99 / 100).expect("fits u64");
    assert_eq!(
        used_threshold_bytes(u64::MAX, 99),
        expected,
        "the figure is exact in u128 even at the type limit"
    );
}

#[test]
fn countable_bytes_sums_zms_and_idx() {
    let snap = snapshot(
        vec![
            segment(1, 100, Some(30)),
            segment(MICROS_PER_DAY, 200, None),
        ],
        Vec::new(),
    );
    assert_eq!(countable_bytes(&snap), 100 + 30 + 200);
}

#[test]
fn countable_bytes_saturates_instead_of_overflowing() {
    let snap = snapshot(
        vec![
            segment(1, u64::MAX, Some(u64::MAX)),
            segment(2, u64::MAX, None),
        ],
        Vec::new(),
    );
    assert_eq!(countable_bytes(&snap), u64::MAX);
}

fn bare_rotation(config: RetentionConfig, non_journal_bytes: u64, seeded: Instant) -> Rotation {
    Rotation {
        config,
        limits: LayoutLimits::default(),
        non_journal_bytes,
        last_tick: seeded,
        last_recount: seeded,
        last_degradation: None,
        pending_reclaim: 0,
        last_observed_used: None,
    }
}

#[test]
fn tree_bytes_saturates_instead_of_overflowing() {
    let rotation = bare_rotation(RetentionConfig::Fixed(0), u64::MAX, Instant::now());
    assert_eq!(rotation.tree_bytes(1), u64::MAX);
}

#[test]
fn time_until_tick_counts_down_and_saturates_at_zero() {
    let seeded = Instant::now();
    let rotation = bare_rotation(RetentionConfig::Fixed(0), 0, seeded);
    assert_eq!(rotation.time_until_tick(seeded), TICK);
    assert_eq!(
        rotation.time_until_tick(seeded + TICK / 2),
        TICK / 2,
        "half the period elapsed leaves half the wait"
    );
    assert_eq!(rotation.time_until_tick(seeded + TICK), Duration::ZERO);
    assert_eq!(
        rotation.time_until_tick(seeded + TICK * 3),
        Duration::ZERO,
        "an overdue tick never yields a negative wait"
    );
}

#[test]
fn plan_keeps_the_newest_segment_and_orders_oldest_first() {
    let snap = snapshot(
        vec![
            segment(1, 10, None),
            segment(2, 10, None),
            segment(MICROS_PER_DAY, 10, None),
        ],
        Vec::new(),
    );
    let victims = plan_victims(&snap);
    let ids: Vec<i64> = victims
        .iter()
        .filter_map(|v| match v {
            Victim::Segment(address) => Some(address.id.get()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![1, 2], "oldest two are candidates, newest is kept");
}

#[test]
fn plan_keeps_a_lone_segment() {
    let snap = snapshot(vec![segment(1, 10, None)], Vec::new());
    assert!(
        !plan_victims(&snap)
            .iter()
            .any(|v| matches!(v, Victim::Segment(_))),
        "a single segment is the non-deletable minimum"
    );
}

#[test]
fn plan_orders_orphans_before_segments() {
    let snap = snapshot(
        vec![segment(1, 10, None), segment(2, 10, None)],
        vec![address(MICROS_PER_DAY)],
    );
    let victims = plan_victims(&snap);
    assert!(
        matches!(victims.first(), Some(Victim::OrphanIndex(_))),
        "orphan indexs are reclaimed before finished segments"
    );
}

#[test]
fn only_segments_and_temporaries_adjust_the_tree_counter() {
    assert!(
        Victim::Segment(address(1)).counted_in_tree(),
        "finished segments are seeded and their removal adjusts the counter"
    );
    assert!(
        !Victim::OrphanIndex(address(1)).counted_in_tree(),
        "orphan indexs have no scanned size, so they stay out of the counter"
    );
}

#[test]
fn enforcement_reseeds_the_counter_with_post_seed_indexs() {
    let directory = tempfile::tempdir().expect("create a tempdir");
    let root = kronika_layout::DataRoot::open(directory.path()).expect("open the root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire the writer");
    let oldest = address(1);
    for item in [oldest, address(2), address(3)] {
        let mut temp = owner.create_zms_temp(item).expect("create a temporary");
        std::io::Write::write_all(temp.file_mut(), b"ZMSBODY").expect("write the body");
        temp.publish().expect("publish the segment");
    }
    let mut rotation = Rotation::new(
        Some(RetentionConfig::Fixed(1)),
        &owner,
        LayoutLimits::default(),
        Instant::now(),
    )
    .expect("seed rotation")
    .expect("rotation is enabled");
    // A sidecar published by the web process after seeding: the
    // incremental counter has not seen it, only a rescan can.
    let sidecar = directory
        .path()
        .join(oldest.day.to_string())
        .join(oldest.idx_name());
    std::fs::write(&sidecar, b"IDX").expect("write the sidecar");

    rotation
        .enforce(&owner, 0, Instant::now())
        .expect("enforce the budget");

    let recount = countable_bytes(&root.scan(LayoutLimits::default()).expect("rescan"));
    assert_eq!(
        rotation.non_journal_bytes, recount,
        "the counter matches a full recount after deleting a segment with a post-seed sidecar"
    );
    assert_eq!(recount, b"ZMSBODY".len() as u64, "only the newest survives");
}

#[test]
fn hourly_recount_catches_growth_the_counter_never_sees() {
    let directory = tempfile::tempdir().expect("create a tempdir");
    let root = kronika_layout::DataRoot::open(directory.path()).expect("open the root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire the writer");
    let oldest = address(1);
    for item in [oldest, address(2), address(3)] {
        let mut temp = owner.create_zms_temp(item).expect("create a temporary");
        std::io::Write::write_all(temp.file_mut(), b"ZMSBODY").expect("write the body");
        temp.publish().expect("publish the segment");
    }
    // Budget 25: the seeded counter (21) stays under it, so the scan-free
    // precheck alone would never fire.
    let mut rotation = Rotation::new(
        Some(RetentionConfig::Fixed(25)),
        &owner,
        LayoutLimits::default(),
        Instant::now(),
    )
    .expect("seed rotation")
    .expect("rotation is enabled");
    // A 10-byte sidecar from the web process pushes the real tree to 31,
    // invisible to the incremental counter.
    let sidecar = directory
        .path()
        .join(oldest.day.to_string())
        .join(oldest.idx_name());
    std::fs::write(&sidecar, b"OVERGROWTH").expect("write the sidecar");

    let now = Instant::now();
    rotation
        .enforce(&owner, 0, now)
        .expect("enforce under the stale counter");
    assert_eq!(
        root.scan(LayoutLimits::default())
            .expect("rescan")
            .segments
            .len(),
        3,
        "without a due recount the stale counter sees no deficit"
    );

    rotation.last_recount = now
        .checked_sub(RECOUNT_PERIOD)
        .expect("the test clock is past the recount period");
    rotation.enforce(&owner, 0, now).expect("enforce recounted");
    let after = root.scan(LayoutLimits::default()).expect("rescan");
    assert_eq!(
        after.segments.len(),
        2,
        "the recount folds the sidecar in and deletes the oldest segment"
    );
    assert_eq!(
        rotation.non_journal_bytes,
        countable_bytes(&after),
        "the counter matches a full recount after the pass"
    );
}

#[test]
fn incremental_counter_converges_with_a_full_recount() {
    // Seed from one tree, apply a publication and a deletion, and confirm the
    // running counter equals a fresh recount of the resulting tree.
    let before = snapshot(
        vec![segment(1, 100, None), segment(2, 200, None)],
        Vec::new(),
    );
    let seed = countable_bytes(&before);

    let published = 150_u64; // a new segment finished
    let deleted = 100_u64; // the oldest segment removed
    let incremental = seed.saturating_add(published).saturating_sub(deleted);

    let after = snapshot(
        vec![segment(2, 200, None), segment(MICROS_PER_DAY, 150, None)],
        Vec::new(),
    );
    assert_eq!(incremental, countable_bytes(&after));
}
