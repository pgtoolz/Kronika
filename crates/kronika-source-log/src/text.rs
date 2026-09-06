//! Bounds on the text a log event carries into the dictionary.

/// Longest normalized grouping pattern, bytes.
pub const MAX_PATTERN_BYTES: usize = 256;

/// Longest stored message, statement or continuation, bytes.
pub const MAX_TEXT_BYTES: usize = 5120;

/// Cut `value` to at most `max_bytes`, on a character boundary.
#[must_use]
pub(crate) fn truncate(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.get(..end).unwrap_or_default()
}

/// The trimmed text, bounded, or `None` when nothing is left.
#[must_use]
pub(crate) fn bounded(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| truncate(trimmed, MAX_TEXT_BYTES).to_owned())
}

/// Append `text` to `target`, separated by a space and bounded.
pub(crate) fn append(target: &mut Option<String>, text: &str) {
    append_str(target.get_or_insert_with(String::new), text);
}

/// Append `text` to `target`, separated by a space and bounded.
pub(crate) fn append_str(target: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() || target.len() >= MAX_TEXT_BYTES {
        return;
    }
    if !target.is_empty() {
        target.push(' ');
    }
    let room = MAX_TEXT_BYTES.saturating_sub(target.len());
    target.push_str(truncate(text, room));
}

#[cfg(test)]
mod tests;
