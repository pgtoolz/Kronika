use super::*;

fn small_limits() -> DictLimits {
    DictLimits::new(8, 16).expect("8 <= 16")
}

fn id_of(bytes: &[u8]) -> StrId {
    StrId::of(bytes).expect("test value must not hash to zero")
}

#[test]
fn rejects_inverted_limits() {
    assert!(DictLimits::new(0, 16).is_err());
    assert!(DictLimits::new(32, 16).is_err());
    assert!(DictLimits::new(16, 16).is_ok());
    // The cap must fit at least one value of the maximum stored size.
    assert!(
        DictLimits::new(8, 16)
            .expect("valid")
            .with_max_total_bytes(15)
            .is_err()
    );
}

#[test]
fn total_bytes_cap_signals_full() {
    let limits = DictLimits::new(8, 16)
        .expect("valid")
        .with_max_total_bytes(16)
        .expect("cap fits one value");
    let mut dicts = SegmentDicts::new(limits);

    dicts.intern(b"0123456789").expect("ten bytes fit the cap");
    let err = dicts
        .intern(b"abcdefghij")
        .expect_err("ten more would exceed the cap");
    assert_eq!(
        err,
        DictError::Full {
            stored_bytes: 10,
            max: 16
        }
    );
    assert_eq!(dicts.len(), 1, "a rejected value is not stored");

    // Repeats of stored values add no bytes and stay allowed.
    dicts.intern(b"0123456789").expect("repeat is free");
    // Strict-hot values are exempt: registry-bounded by contract.
    dicts.intern_hot(b"hot").expect("hot bypasses the cap");
    assert_eq!(dicts.stored_bytes(), 13);
}

#[test]
fn empty_string_is_a_regular_value() {
    let mut dicts = SegmentDicts::new(small_limits());
    let id = dicts.intern(b"").expect("empty string interns");
    assert_ne!(id.get(), 0);
    assert_eq!(dicts.resolve(id), Some(Resolved::Str(&[][..])));
}

#[test]
fn resolved_exposes_stored_bytes_for_both_placements() {
    let string = Resolved::Str(b"string");
    let blob = Resolved::Blob(BlobEntry {
        str_id: id_of(b"blob"),
        stored_bytes: b"blob",
        full_len: 4,
        truncated: false,
        full_sha256: None,
    });

    assert_eq!(string.stored_bytes(), b"string");
    assert_eq!(blob.stored_bytes(), b"blob");
}

#[test]
fn zero_hash_is_a_collision() {
    // No xxh3 preimage of zero is known, so the rule is tested on the
    // extracted conversion that every public intern call goes through.
    assert_eq!(
        id_or_collision(None).expect_err("zero hash must be rejected"),
        DictError::Collision { id: 0 }
    );
    let real = StrId::of(b"value");
    assert_eq!(id_or_collision(real).ok(), real);
}

#[test]
fn same_id_different_bytes_is_a_collision() {
    let mut dicts = SegmentDicts::new(small_limits());
    let id = dicts.intern(b"short").expect("interns");
    let err = dicts
        .try_insert(id, b"other", Requirements::default())
        .expect_err("different bytes under one id");
    assert_eq!(err, DictError::Collision { id: id.get() });
    // The failed call must not have changed the stored value.
    assert_eq!(dicts.resolve(id), Some(Resolved::Str(&b"short"[..])));
}

#[test]
fn truncation_keeps_full_value_identity() {
    let mut dicts = SegmentDicts::new(small_limits());
    let value = b"this value is longer than sixteen bytes";
    let id = dicts.intern(value).expect("interns");
    assert_eq!(id, id_of(value), "id is computed over the full value");

    let Some(Resolved::Blob(entry)) = dicts.resolve(id) else {
        panic!("oversized value must resolve as a blob");
    };
    assert!(entry.truncated);
    assert_eq!(entry.full_len, value.len() as u64);
    assert_eq!(entry.stored_bytes, &value[..16]);
    let expected: [u8; 32] = Sha256::digest(value).into();
    assert_eq!(entry.full_sha256, Some(expected));
}

#[test]
fn reinterning_an_oversized_value_is_not_a_collision() {
    let mut dicts = SegmentDicts::new(small_limits());
    let value = b"this value is longer than sixteen bytes";
    let first = dicts.intern(value).expect("interns");
    let second = dicts.intern(value).expect("same value re-interns");
    assert_eq!(first, second);
    assert_eq!(dicts.len(), 1);
}

#[test]
fn truncated_entries_collide_via_full_value_identity() {
    let mut dicts = SegmentDicts::new(small_limits());
    // Same length, same stored prefix, different tail: only
    // (full_len, full_sha256) can tell these apart after truncation.
    let original = b"0123456789abcdef this is tail A";
    let impostor = b"0123456789abcdef this is tail B";
    assert_eq!(original.len(), impostor.len());

    let id = dicts.intern(original).expect("interns");
    let err = dicts
        .try_insert(id, impostor, Requirements::default())
        .expect_err("same id and length, different content");
    assert_eq!(err, DictError::Collision { id: id.get() });
}

#[test]
fn threshold_and_truncation_boundaries() {
    let mut dicts = SegmentDicts::new(small_limits());

    // blob_threshold = 8: seven bytes is a string, eight is a blob.
    let seven = dicts.intern(&[7_u8; 7]).expect("interns");
    assert!(matches!(dicts.resolve(seven), Some(Resolved::Str(_))));
    let eight = dicts.intern(&[8_u8; 8]).expect("interns");
    assert!(matches!(dicts.resolve(eight), Some(Resolved::Blob(_))));

    // truncate_limit = 16: sixteen bytes is stored whole and carries
    // no sha, seventeen is cut to the limit.
    let sixteen = dicts.intern(&[16_u8; 16]).expect("interns");
    let Some(Resolved::Blob(entry)) = dicts.resolve(sixteen) else {
        panic!("sixteen bytes is a blob");
    };
    assert!(!entry.truncated);
    assert_eq!(entry.full_sha256, None);
    assert_eq!(entry.stored_bytes.len(), 16);

    let seventeen = dicts.intern(&[17_u8; 17]).expect("interns");
    let Some(Resolved::Blob(entry)) = dicts.resolve(seventeen) else {
        panic!("seventeen bytes is a blob");
    };
    assert!(entry.truncated);
    assert_eq!(entry.stored_bytes.len(), 16);
    assert_eq!(entry.full_len, 17);
}

#[test]
fn hot_of_an_oversized_value_is_a_conflict() {
    let mut dicts = SegmentDicts::new(small_limits());
    let err = dicts
        .intern_hot(b"longer than the eight-byte threshold")
        .expect_err("strict hot cannot live in blobs");
    assert!(matches!(err, DictError::PlacementConflict { .. }));
    assert!(dicts.is_empty(), "a rejected value is not stored");
}

#[test]
fn hard_hot_and_forced_blob_conflict_in_both_orders() {
    let value = b"plan";

    let mut dicts = SegmentDicts::new(small_limits());
    let id = dicts.intern_hot(value).expect("hot first");
    let err = dicts.intern_blob(value).expect_err("then blob");
    assert!(matches!(err, DictError::PlacementConflict { .. }));
    // The failed call must not have moved the value or dropped its
    // hot mark.
    assert!(matches!(dicts.resolve(id), Some(Resolved::Str(_))));
    assert_eq!(dicts.hot_strings().count(), 1);
    assert_eq!(dicts.stats().blob_count, 0);

    let mut dicts = SegmentDicts::new(small_limits());
    let id = dicts.intern_blob(value).expect("blob first");
    let err = dicts.intern_hot(value).expect_err("then hot");
    assert!(matches!(err, DictError::PlacementConflict { .. }));
    assert!(matches!(dicts.resolve(id), Some(Resolved::Blob(_))));
    assert_eq!(dicts.hot_strings().count(), 0);
}

#[test]
fn soft_hot_skips_hot_cache_for_blob_without_error() {
    let value = b"label";

    // Soft hot first, forced blob later: the value moves to blobs and
    // the soft hot mark does not add it to the hot cache, in either
    // call order.
    let mut dicts = SegmentDicts::new(small_limits());
    let (id, hot) = dicts.intern_hot_best_effort(value).expect("soft hot");
    assert!(hot, "short value is string-placed, so it is hot");
    dicts
        .intern_blob(value)
        .expect("forced blob wins over soft hot");
    assert_eq!(dicts.hot_strings().count(), 0);
    assert!(matches!(dicts.resolve(id), Some(Resolved::Blob(_))));

    let mut dicts = SegmentDicts::new(small_limits());
    dicts.intern_blob(value).expect("forced blob first");
    let (_, hot) = dicts.intern_hot_best_effort(value).expect("soft hot later");
    assert!(!hot, "blob-placed value stays out of the hot cache");
    assert_eq!(dicts.hot_strings().count(), 0);
}

#[test]
fn stats_count_both_dictionaries() {
    let mut dicts = SegmentDicts::new(small_limits());
    dicts.intern(b"a").expect("string");
    dicts.intern_hot(b"hot").expect("hot string");
    dicts.intern(b"longer than the threshold").expect("blob");
    let stats = dicts.stats();
    assert_eq!(stats.string_count, 2);
    assert_eq!(stats.hot_count, 1);
    assert_eq!(stats.blob_count, 1);
    assert_eq!(stats.truncated_blob_count, 1);
    assert_eq!(stats.string_bytes, 4);
    assert_eq!(stats.blob_bytes, 16);
}
