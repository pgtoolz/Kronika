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
