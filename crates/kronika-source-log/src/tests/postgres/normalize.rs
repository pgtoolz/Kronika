use super::{ErrorCategory, classify_error, normalize_error, normalize_sql};
use crate::postgres::Severity;

#[test]
fn two_occurrences_of_one_error_group_together() {
    assert_eq!(
        normalize_error("relation \"users\" does not exist at character 15"),
        normalize_error("relation \"orders\" does not exist at character 42")
    );
}

#[test]
fn the_values_an_error_carries_are_replaced() {
    assert_eq!(
        normalize_error("relation \"users\" does not exist at character 15"),
        "relation \"...\" does not exist"
    );
    assert_eq!(
        normalize_error("invalid input syntax for type integer: \"abc\""),
        "invalid input syntax for type integer: ..."
    );
    assert_eq!(
        normalize_error("server process (PID 4242) was terminated by signal 9: Killed"),
        "server process (...) was terminated by signal ...: Killed"
    );
    assert_eq!(
        normalize_error("requested WAL segment 0/16B3D40 has already been removed"),
        "requested WAL segment x/x has already been removed"
    );
    assert_eq!(
        normalize_error("PERMISSION DENIED FOR DATABASE tenant_42"),
        "PERMISSION DENIED FOR DATABASE ..."
    );
}

#[test]
fn statements_that_differ_only_in_their_literals_group_together() {
    assert_eq!(
        normalize_sql("select * from orders where id = 42 and total > 19.99"),
        normalize_sql("select * from orders where id = 7 and total > 5.00")
    );
    assert_eq!(
        normalize_sql("select * from t1 where id = 42"),
        "select * from t1 where id = ..."
    );
}

#[test]
fn the_first_family_that_matches_wins() {
    assert_eq!(
        classify_error("deadlock detected while permission denied", Severity::Error),
        ErrorCategory::Lock
    );
    assert_eq!(
        classify_error("could not serialize access", Severity::Error),
        ErrorCategory::Serialization
    );
    assert_eq!(
        classify_error("division by zero", Severity::Error),
        ErrorCategory::Syntax
    );
}

#[test]
fn a_kill_is_a_resource_problem_and_a_crash_is_a_system_one() {
    assert_eq!(
        classify_error(
            "server process (...) was terminated by signal ...: Killed",
            Severity::Log
        ),
        ErrorCategory::Resource
    );
    assert_eq!(
        classify_error(
            "server process (...) was terminated by signal ...: Segmentation fault",
            Severity::Log
        ),
        ErrorCategory::System
    );
}

#[test]
fn panic_and_uncategorized_fatal_have_a_floor() {
    assert_eq!(
        classify_error("anything at all", Severity::Panic),
        ErrorCategory::DataCorruption
    );
    assert_eq!(
        classify_error("something new", Severity::Fatal),
        ErrorCategory::System
    );
    assert_eq!(
        classify_error("something new", Severity::Error),
        ErrorCategory::Other
    );
}
