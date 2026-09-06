use kronika_format::Resolved;

use super::*;

fn small_interner() -> Interner {
    Interner::new(DictLimits::new(8, 16).expect("8 <= 16"))
}

/// Flush the window pretending the journal write always succeeds.
fn flush_ok(interner: &mut Interner) -> usize {
    interner
        .flush_window(|_| Ok::<(), ()>(()))
        .expect("infallible write")
}

#[test]
fn str_id_is_stable_across_instances() {
    let mut a = small_interner();
    let mut b = small_interner();
    let id_a = a.intern(b"pg_stat_activity").expect("interns");
    let id_b = b.intern(b"pg_stat_activity").expect("interns");
    assert_eq!(id_a, id_b);
}

#[test]
fn window_holds_only_values_new_since_last_flush() {
    let mut interner = small_interner();
    interner.intern(b"a").expect("interns");
    interner.intern(b"b").expect("interns");
    assert_eq!(flush_ok(&mut interner), 2);
    assert!(interner.window().is_empty());

    // A repeat of a flushed value does not re-enter memory; a new
    // value does.
    let again = interner.intern(b"a").expect("re-interns");
    assert!(interner.window().resolve(again).is_none());
    assert!(interner.is_interned(again));
    let c = interner.intern(b"c").expect("interns");
    assert!(interner.window().resolve(c).is_some());
    assert_eq!(flush_ok(&mut interner), 1);
}

#[test]
fn failed_journal_write_keeps_the_window() {
    let mut interner = small_interner();
    interner.intern(b"value").expect("interns");
    let err = interner.flush_window(|_| Err::<(), &str>("disk full"));
    assert_eq!(err, Err("disk full"));
    assert_eq!(
        interner.window().len(),
        1,
        "the only copy of the bytes must survive a failed write"
    );
    // A failed write must not create flushed records, or write_segment() would
    // emit directives for values that exist nowhere in the journal.
    let finished = interner.write_segment();
    assert!(finished.flushed.is_empty());
    assert_eq!(finished.window.len(), 1);
}

#[test]
fn close_reports_upgrades_not_yet_flushed_again() {
    // Blob upgrade after a flush, finished before the next flush: the
    // directive must already carry the new placement, because the
    // merge takes placement from directives, not from part dicts.
    let mut interner = small_interner();
    let id = interner.intern(b"plan").expect("interns");
    flush_ok(&mut interner);
    interner.intern_blob(b"plan").expect("upgrade");
    let finished = interner.write_segment();
    let entry = finished
        .flushed
        .iter()
        .find(|entry| entry.str_id == id)
        .expect("directive");
    assert_eq!(entry.placement, Placement::Blobs);

    // Same for a strict hot upgrade.
    let mut interner = small_interner();
    let id = interner.intern(b"src/42").expect("interns");
    flush_ok(&mut interner);
    interner.intern_hot(b"src/42").expect("hot upgrade");
    let finished = interner.write_segment();
    let entry = finished
        .flushed
        .iter()
        .find(|entry| entry.str_id == id)
        .expect("directive");
    assert_eq!(entry.hot, HotMark::Hard);
}

#[test]
fn soft_hot_survives_the_flush_boundary() {
    let mut interner = small_interner();
    let (id, hot) = interner.intern_hot_best_effort(b"label").expect("soft hot");
    assert!(hot);
    flush_ok(&mut interner);

    // A repeat of the flushed soft-hot value reports hot without
    // re-entering the window.
    let (again, hot) = interner.intern_hot_best_effort(b"label").expect("repeat");
    assert_eq!(again, id);
    assert!(hot);
    assert!(interner.window().is_empty());

    // A late soft mark on a flushed plain string updates the
    // directive without loading the bytes back into the window.
    let plain = interner.intern(b"note").expect("interns");
    flush_ok(&mut interner);
    let (_, hot) = interner.intern_hot_best_effort(b"note").expect("soft mark");
    assert!(hot);
    assert!(interner.window().is_empty());
    assert_eq!(interner.stats().hot_count, 2);
    let finished = interner.write_segment();
    let entry = finished
        .flushed
        .iter()
        .find(|entry| entry.str_id == plain)
        .expect("directive");
    assert_eq!(entry.hot, HotMark::Soft);
}

#[test]
fn soft_hot_on_flushed_blob_does_not_reload_value() {
    let mut interner = small_interner();
    let oversized = b"this value is longer than sixteen bytes";
    let id = interner.intern(oversized).expect("interns as a blob");
    flush_ok(&mut interner);

    // A soft mark can never become effective on a blob-placed value:
    // it must not pull the stored bytes back into memory.
    let (again, hot) = interner
        .intern_hot_best_effort(oversized)
        .expect("soft mark on a blob");
    assert_eq!(again, id);
    assert!(!hot);
    assert!(
        interner.window().is_empty(),
        "soft hot on a blob must not reload the value"
    );
    let finished = interner.write_segment();
    assert_eq!(finished.flushed[0].hot, HotMark::None);
}

#[test]
fn flushed_values_are_verified_not_trusted() {
    let mut interner = small_interner();
    let id = interner.intern(b"short").expect("interns");
    flush_ok(&mut interner);

    // The same value re-interns fine even though its bytes are gone.
    assert_eq!(interner.intern(b"short").expect("repeat"), id);
    // Different bytes under the same id would be a collision; the
    // public path cannot construct one (no known xxh3 preimages), so
    // the verifier is tested directly.
    let flushed = Flushed {
        full_len: 5,
        check: check16(b"short"),
        blob_required: false,
        hot_hard: false,
        hot_soft: false,
    };
    assert!(flushed.matches(b"short"));
    assert!(!flushed.matches(b"shore"), "same length, different bytes");
    assert!(!flushed.matches(b"shorter"), "different length");
}

#[test]
fn upgrade_of_a_flushed_value_reenters_the_window() {
    let mut interner = small_interner();
    let id = interner.intern(b"plan").expect("interns as a string");
    flush_ok(&mut interner);

    // The registry now requires the same value in dict.blobs. The next
    // part must record that, so the value enters the window again.
    let same = interner.intern_blob(b"plan").expect("upgrade");
    assert_eq!(same, id);
    assert!(
        matches!(interner.window().resolve(id), Some(Resolved::Blob(_))),
        "the window records the upgraded placement"
    );
    assert_eq!(flush_ok(&mut interner), 1);

    // After the next flush the directive remains in the flushed map.
    let finished = interner.write_segment();
    let entry = finished
        .flushed
        .iter()
        .find(|entry| entry.str_id == id)
        .expect("flushed directive");
    assert_eq!(entry.placement, Placement::Blobs);
}

#[test]
fn conflicts_on_flushed_values_fail_at_the_call_site() {
    let mut interner = small_interner();
    interner.intern_blob(b"plan").expect("forced blob");
    flush_ok(&mut interner);

    let err = interner
        .intern_hot(b"plan")
        .expect_err("hot of a flushed forced-blob value");
    assert!(matches!(err, DictError::PlacementConflict { .. }));
    assert!(
        interner.window().is_empty(),
        "a rejected upgrade must not re-enter the window"
    );
}

#[test]
fn pinned_hot_values_reach_every_window() {
    let mut interner = small_interner();
    let source = interner.intern_hot(b"src/42").expect("strict hot");
    flush_ok(&mut interner);

    // The fresh window already carries the pinned value, so the next part
    // resolves its own catalog labels.
    assert!(interner.window().resolve(source).is_some());
    assert_eq!(interner.window().hot_strings().count(), 1);
    assert_eq!(flush_ok(&mut interner), 1, "the pin is flushed again");
    assert!(interner.window().resolve(source).is_some());
}

#[test]
fn close_returns_remaining_window_and_flushed_directives() {
    let mut interner = small_interner();
    let flushed_id = interner.intern(b"flushed").expect("interns");
    flush_ok(&mut interner);
    let window_id = interner.intern(b"window").expect("interns");

    let finished = interner.write_segment();
    assert!(finished.window.resolve(window_id).is_some());
    assert!(finished.window.resolve(flushed_id).is_none());
    assert_eq!(finished.flushed.len(), 1);
    assert_eq!(finished.flushed[0].str_id, flushed_id);
    assert_eq!(finished.flushed[0].placement, Placement::Strings);

    // The interner starts the next segment empty.
    assert!(interner.window().is_empty());
    assert!(!interner.is_interned(flushed_id));
    assert_eq!(interner.stats(), DictStats::default());
}

#[test]
fn stats_cover_window_and_flushed_without_double_counting() {
    let mut interner = small_interner();
    interner.intern(b"one").expect("string");
    interner.intern_hot(b"hot").expect("hot string");
    interner.intern(b"longer than the threshold").expect("blob");
    flush_ok(&mut interner);
    interner.intern(b"fresh").expect("window string");
    // An upgrade present in both maps must be counted once.
    interner.intern_blob(b"one").expect("upgrade");

    let stats = interner.stats();
    // Strings: "hot" (pinned, in window), "fresh" (window). "one" is
    // now a blob. Blobs: "one" + the oversized value.
    assert_eq!(stats.string_count, 2);
    assert_eq!(stats.blob_count, 2);
    assert_eq!(stats.truncated_blob_count, 1);
    assert_eq!(stats.hot_count, 1);
}

#[test]
fn oversized_strict_hot_fails_without_state_change() {
    let mut interner = small_interner();
    let err = interner
        .intern_hot(b"longer than the eight-byte threshold")
        .expect_err("strict hot of an oversized value");
    assert!(matches!(err, DictError::PlacementConflict { .. }));
    assert!(interner.window().is_empty());
    assert_eq!(interner.stats(), DictStats::default());
}

#[test]
fn full_window_signals_flush_and_recovers() {
    let limits = DictLimits::new(8, 16)
        .expect("valid")
        .with_max_total_bytes(16)
        .expect("cap fits one value");
    let mut interner = Interner::new(limits);

    interner.intern(b"0123456789").expect("fits the window cap");
    let err = interner
        .intern(b"abcdefghij")
        .expect_err("the window is full");
    assert!(matches!(err, DictError::Full { .. }));

    // The signal means "flush": after the flush the value fits.
    flush_ok(&mut interner);
    interner
        .intern(b"abcdefghij")
        .expect("fits after the flush");
}
