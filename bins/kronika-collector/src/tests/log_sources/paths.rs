use super::matches;

#[test]
fn a_literal_pattern_matches_only_itself() {
    assert!(matches("pgbouncer.log", "pgbouncer.log"));
    assert!(!matches("pgbouncer.log", "pgbouncer.log.1"));
}

#[test]
fn a_star_swallows_any_run_including_none() {
    assert!(matches("*.log", "pgbouncer.log"));
    assert!(matches("*.log", ".log"));
    assert!(matches("pgbouncer-*.log", "pgbouncer-shard2.log"));
    assert!(!matches("*.log", "pgbouncer.txt"));
}

#[test]
fn a_star_backtracks_when_the_tail_does_not_fit_yet() {
    assert!(matches("*.log", "a.log.log"));
    assert!(matches("post*gres*.csv", "postgresql-gres.csv"));
}

#[test]
fn a_question_mark_takes_exactly_one_character() {
    assert!(matches("pgbouncer-?.log", "pgbouncer-1.log"));
    assert!(!matches("pgbouncer-?.log", "pgbouncer-12.log"));
    assert!(!matches("pgbouncer-?.log", "pgbouncer-.log"));
}

#[test]
fn trailing_stars_may_match_nothing() {
    assert!(matches("pgbouncer**", "pgbouncer"));
    assert!(matches("*", ""));
}

#[test]
fn missing_literals_and_relative_globs_remain_concrete() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("missing.log");
    let expanded = super::expand(path.to_str().expect("path"));
    assert!(expanded.complete);
    assert_eq!(expanded.paths, [path]);
    let relative = super::expand("Cargo.t?ml");
    assert!(relative.complete);
    assert_eq!(relative.paths, [std::path::PathBuf::from("./Cargo.toml")]);
}

#[test]
fn partial_enumeration_retains_matches_and_reports_incompleteness() {
    let dir = tempfile::tempdir().expect("fixture");
    let file = dir.path().join("good.log");
    std::fs::write(&file, "").expect("log");
    let entries = std::fs::read_dir(dir.path())
        .expect("entries")
        .chain([Err(std::io::Error::from_raw_os_error(13))]);
    let expanded = super::matching_files(&dir.path().join("*.log"), "*.log", entries);
    assert!(!expanded.complete);
    assert_eq!(expanded.paths, [file]);
    for _ in 0..2 {
        let empty = super::expand(dir.path().join("*.csv").to_str().expect("pattern"));
        assert!(empty.complete);
        assert!(empty.paths.is_empty());
    }
}
