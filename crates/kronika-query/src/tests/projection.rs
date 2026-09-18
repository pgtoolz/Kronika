use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::{
    ColumnClass, PgStatDatabaseV1, PgStatDatabaseV2, PgStatDatabaseV4, PgStatStatementsV1,
    PgStatStatementsV3, Section as _,
};

use super::{OutputField, cells_equal, output_names, projection, typed_filter, validate_filters};
use crate::QueryError;
use crate::request::Filter;

#[test]
fn filters_are_typed_against_every_layout_of_the_section() {
    let known = [&PgStatDatabaseV2::CONTRACT, &PgStatDatabaseV4::CONTRACT];
    let filter = |column: &str, value: &str| Filter {
        column: column.to_owned(),
        value: value.to_owned(),
    };
    validate_filters(&known, &[filter("datname", "postgres")])
        .expect("a label the section carries");
    let error = validate_filters(&known, &[filter("sessions_fatal", "0")])
        .expect_err("a counter is not a filter even where only an older layout is recorded");
    assert!(matches!(error, QueryError::BadFilter(name) if name == "sessions_fatal"));
    let error = validate_filters(&known, &[filter("datid", "not-a-number")])
        .expect_err("the value must fit the column type");
    assert!(matches!(error, QueryError::BadFilter(name) if name == "datid"));
    let error = validate_filters(&known, &[filter("not_a_column", "x")])
        .expect_err("a name no layout of the section carries");
    assert!(matches!(error, QueryError::NoSuchColumn(name) if name == "not_a_column"));
}

#[test]
fn requested_field_may_exist_in_only_one_physical_layout() {
    let contracts = [&PgStatDatabaseV1::CONTRACT, &PgStatDatabaseV4::CONTRACT];
    let names = output_names(
        &contracts,
        &contracts,
        &["parallel_workers_launched".to_owned()],
    )
    .expect("field exists in one layout");
    assert_eq!(names, ["parallel_workers_launched"]);
}

#[test]
fn a_field_absent_from_every_layout_is_rejected() {
    let contracts = [&PgStatDatabaseV1::CONTRACT, &PgStatDatabaseV4::CONTRACT];
    let error = output_names(&contracts, &contracts, &["not_a_column".to_owned()])
        .expect_err("unknown field");
    assert!(matches!(error, QueryError::NoSuchColumn(name) if name == "not_a_column"));
}

#[test]
fn a_field_known_to_the_section_is_accepted_when_no_recorded_layout_has_it() {
    let recorded = [&PgStatDatabaseV2::CONTRACT];
    let known = [&PgStatDatabaseV2::CONTRACT, &PgStatDatabaseV4::CONTRACT];
    let names = output_names(&recorded, &known, &["sessions_fatal".to_owned()])
        .expect("a newer layout of the section carries the field");
    assert_eq!(names, ["sessions_fatal"]);
    let error = output_names(&recorded, &known, &["not_a_column".to_owned()])
        .expect_err("no layout of the section carries the field");
    assert!(matches!(error, QueryError::NoSuchColumn(name) if name == "not_a_column"));
}

#[test]
fn default_projection_is_the_union_in_stable_layout_order() {
    let contracts = [&PgStatDatabaseV1::CONTRACT, &PgStatDatabaseV4::CONTRACT];
    let names = output_names(&contracts, &contracts, &[]).expect("default fields");
    assert_eq!(names.first().map(String::as_str), Some("ts"));
    assert!(names.iter().any(|name| name == "datname"));
    assert!(names.iter().any(|name| name == "parallel_workers_launched"));
    let unique: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(unique.len(), names.len());
}

#[test]
fn default_projection_covers_only_the_recorded_layouts() {
    let recorded = [&PgStatDatabaseV1::CONTRACT];
    let known = [&PgStatDatabaseV1::CONTRACT, &PgStatDatabaseV4::CONTRACT];
    let names = output_names(&recorded, &known, &[]).expect("default fields");
    assert!(names.iter().any(|name| name == "datname"));
    assert!(!names.iter().any(|name| name == "parallel_workers_launched"));
}

#[test]
fn a_filter_absent_from_one_layout_makes_that_layout_inapplicable() {
    let filter = Filter {
        column: "toplevel".to_owned(),
        value: "true".to_owned(),
    };
    assert!(
        typed_filter(&PgStatStatementsV1::CONTRACT, &filter)
            .expect("absence is not a type error")
            .is_none()
    );
    assert!(
        typed_filter(&PgStatStatementsV3::CONTRACT, &filter)
            .expect("typed v3 filter")
            .is_some()
    );
}

#[test]
fn typed_filters_reject_values_outside_the_physical_type() {
    let filter = Filter {
        column: "datid".to_owned(),
        value: u64::MAX.to_string(),
    };
    let error =
        typed_filter(&PgStatDatabaseV1::CONTRACT, &filter).expect_err("datid does not hold u64");
    assert!(matches!(error, QueryError::BadFilter(name) if name == "datid"));
}

#[test]
fn filters_are_exact_labels_not_metric_predicates() {
    let filter = Filter {
        column: "xact_commit".to_owned(),
        value: "7".to_owned(),
    };
    assert!(matches!(
        typed_filter(&PgStatDatabaseV1::CONTRACT, &filter),
        Err(QueryError::BadFilter(name)) if name == "xact_commit"
    ));
}

#[test]
fn float_equality_is_bit_exact() {
    assert!(cells_equal(
        &kronika_reader::Cell::F64(-0.0),
        &kronika_reader::Cell::F64(-0.0)
    ));
    assert!(!cells_equal(
        &kronika_reader::Cell::F64(-0.0),
        &kronika_reader::Cell::F64(0.0)
    ));
}

#[test]
fn explicit_non_identity_history_field_keeps_timestamp_and_identity_projection() {
    let contract = &OsDiskstats::CONTRACT;
    let timestamp = contract
        .columns
        .iter()
        .find(|column| column.class == ColumnClass::Timestamp)
        .map(|column| column.name);
    let fields = [OutputField {
        name: "reads".to_owned(),
        column: Some("reads"),
    }];
    let projected = projection(contract, &fields, timestamp, &[], true);

    assert!(projected.contains(&"reads"));
    assert!(projected.contains(&"ts"));
    for identity in contract.identity {
        assert!(projected.contains(identity), "missing identity {identity}");
    }
}
