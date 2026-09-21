//! Resolve configured literal paths and filename patterns.

use std::fs::DirEntry;
use std::io;
use std::path::{Path, PathBuf};

use crate::logging::{LogLevel, field, log_event};

pub(super) struct Expansion {
    pub(super) paths: Vec<PathBuf>,
    pub(super) complete: bool,
}

/// Patterns apply only to the filename component.
pub(super) fn expand(entry: &str) -> Expansion {
    let path = PathBuf::from(entry);
    if !is_pattern(entry) {
        return Expansion {
            paths: vec![path],
            complete: true,
        };
    }
    let Some(pattern) = path.file_name().and_then(|name| name.to_str()) else {
        expansion_failed(
            &path,
            &io::Error::new(io::ErrorKind::InvalidInput, "missing filename pattern"),
        );
        return Expansion {
            paths: Vec::new(),
            complete: false,
        };
    };
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match std::fs::read_dir(directory) {
        Ok(entries) => matching_files(&path, pattern, entries),
        Err(error) => {
            expansion_failed(&path, &error);
            Expansion {
                paths: Vec::new(),
                complete: false,
            }
        }
    }
}

fn matching_files(
    path: &Path,
    pattern: &str,
    entries: impl Iterator<Item = io::Result<DirEntry>>,
) -> Expansion {
    let mut expanded = Expansion {
        paths: Vec::new(),
        complete: true,
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                expanded.complete = false;
                expansion_failed(path, &error);
                continue;
            }
        };
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| matches(pattern, name))
        {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_file() => expanded.paths.push(entry.path()),
            Ok(_) => {}
            Err(error) => {
                expanded.complete = false;
                expansion_failed(&entry.path(), &error);
            }
        }
    }
    expanded.paths.sort();
    if expanded.complete && expanded.paths.is_empty() {
        log_event(
            LogLevel::Warn,
            "log_source_absent",
            &[
                field("pattern", path.display()),
                field("reason", "no matching files"),
            ],
        );
    }
    expanded
}

fn expansion_failed(path: &Path, error: &io::Error) {
    log_event(
        LogLevel::Warn,
        "log_source_expansion_failure",
        &[field("path", path.display()), field("error", error)],
    );
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
#[path = "../tests/log_sources/paths.rs"]
mod tests;
