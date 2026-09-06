//! Turning what the operator wrote into files that exist right now.

use std::path::{Path, PathBuf};

/// Expand one entry into the files it names.
///
/// An entry without `*` or `?` is a path, and it expands to itself when the
/// file is there. Otherwise the last component is a pattern matched against
/// the names in the directory before it.
///
/// ponytail: only the last component may be a pattern; a pattern in the
/// directory part would need a walk, and no log layout wants one yet.
pub(super) fn expand(entry: &str) -> Vec<PathBuf> {
    let path = PathBuf::from(entry);
    if !is_pattern(entry) {
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let Some(pattern) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|candidate| matches(pattern, candidate))
        })
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// Whether an entry names one file or a set of them.
fn is_pattern(entry: &str) -> bool {
    entry.contains('*') || entry.contains('?')
}

/// Match a name against a pattern of literals, `*` and `?`.
fn matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0_usize, 0_usize);
    // Where to resume when a `*` has to swallow one more character.
    let (mut star, mut resume) = (None, 0_usize);
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some(p);
                resume = n;
                p += 1;
            }
            Some('?') => {
                p += 1;
                n += 1;
            }
            Some(literal) if *literal == name[n] => {
                p += 1;
                n += 1;
            }
            _mismatch => {
                let Some(last) = star else {
                    return false;
                };
                p = last + 1;
                resume += 1;
                n = resume;
            }
        }
    }
    pattern[p..].iter().all(|char| *char == '*')
}

#[cfg(test)]
mod tests;
