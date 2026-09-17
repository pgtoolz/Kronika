use super::{MAX_TEXT_BYTES, append, append_str, bounded, truncate};

#[test]
fn a_cut_lands_on_a_character_boundary() {
    // Two-byte characters cut at five bytes keep the two that fit whole.
    assert_eq!(truncate("ждём", 5), "жд");
    assert_eq!(truncate("short", 64), "short");
}

#[test]
fn blank_text_is_no_text() {
    assert_eq!(bounded("   "), None);
    assert_eq!(bounded(" value "), Some("value".to_owned()));
}

#[test]
fn appending_separates_with_a_space_and_stops_at_the_bound() {
    let mut target = None;
    append(&mut target, "first");
    append(&mut target, "second");
    assert_eq!(target, Some("first second".to_owned()));

    let mut full = "x".repeat(MAX_TEXT_BYTES);
    append_str(&mut full, "ignored");
    assert_eq!(full.len(), MAX_TEXT_BYTES);
}
