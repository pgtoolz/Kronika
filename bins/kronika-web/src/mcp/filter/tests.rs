use serde_json::json;

use super::{FilterAtom, FilterInput, Op, build_search};
use kronika_query::snapshot::{SearchOperator, SearchValue};

fn scalar(field: &str, op: Op, value: serde_json::Value) -> FilterInput {
    let field = field.to_owned();
    let value = serde_json::from_value::<FilterAtom>(value).expect("test filter atom");
    match op {
        Op::Eq => FilterInput::Eq { field, value },
        Op::Gt => FilterInput::Gt { field, value },
        Op::Lt => FilterInput::Lt { field, value },
        Op::Contains => FilterInput::Contains { field, value },
        Op::In => panic!("in uses values"),
    }
}

fn atoms(values: impl IntoIterator<Item = serde_json::Value>) -> Vec<FilterAtom> {
    values
        .into_iter()
        .map(|value| serde_json::from_value(value).expect("test filter atom"))
        .collect()
}

#[test]
fn a_single_equality_filter_builds_a_matching_predicate() {
    let filters = vec![scalar("state", Op::Eq, json!("running"))];
    let search = build_search("os_process", &filters)
        .expect("valid filter")
        .expect("non-empty filter list produces a search");
    assert!(search.matches_all(|clause| clause.key == "state"));
}

#[test]
fn an_unknown_field_for_the_section_is_rejected() {
    let filters = vec![scalar("not_a_real_field", Op::Eq, json!("x"))];
    let error = build_search("os_process", &filters).expect_err("unknown field");
    assert!(error.message.contains("not_a_real_field"));
    assert_eq!(
        error.valid_options,
        [
            "text",
            "user",
            "effective_user",
            "user_id",
            "effective_user_id",
            "pid",
            "parent_pid",
            "command",
            "state",
            "rss",
            "vsz",
            "swap",
            "threads",
            "cpu_cores",
            "user_cpu_cores",
            "system_cpu_cores",
            "disk_read_rate",
            "disk_write_rate",
            "logical_read_rate",
            "logical_write_rate",
            "read_syscall_rate",
            "write_syscall_rate",
            "major_fault_rate",
            "minor_fault_rate",
            "context_switch_rate",
            "voluntary_context_switch_rate",
            "involuntary_context_switch_rate",
            "run_delay",
            "block_io_delay",
        ]
        .map(str::to_owned)
    );
}

#[test]
fn no_filters_produces_no_search() {
    assert!(
        build_search("os_process", &[])
            .expect("empty filters are valid")
            .is_none()
    );
}

#[test]
fn two_filters_are_anded_together() {
    // `Gt` applies to quantity fields such as `rss`, not identifiers such as
    // `pid`.
    let filters = vec![
        scalar("state", Op::Eq, json!("running")),
        scalar("rss", Op::Gt, json!(1_000)),
    ];
    let search = build_search("os_process", &filters)
        .expect("valid filters")
        .expect("search");
    let mut seen = Vec::new();
    search.matches_all(|clause| {
        seen.push(clause.key);
        true
    });
    assert_eq!(seen.len(), 2);
}

#[test]
fn a_quantity_filter_builds_a_strict_greater_than_clause() {
    let filters = vec![scalar("rss", Op::Gt, json!(1_048_576))];
    let search = build_search("os_process", &filters)
        .expect("valid filter")
        .expect("search");
    let mut checked = false;
    search.matches_all(|clause| {
        assert_eq!(clause.key, "rss");
        assert_eq!(clause.operator, SearchOperator::Greater);
        assert!(matches!(clause.value, SearchValue::Quantity(_)));
        checked = true;
        true
    });
    assert!(checked);
}

#[test]
fn a_comparison_operator_on_a_string_field_is_rejected() {
    let filters = vec![scalar("state", Op::Gt, json!("running"))];
    let error = build_search("os_process", &filters).expect_err("comparison on a string field");
    assert!(error.message.contains("state"));
}

#[test]
fn contains_on_an_identifier_field_is_rejected() {
    let filters = vec![scalar("pid", Op::Contains, json!(100))];
    let error = build_search("os_process", &filters).expect_err("contains on an identifier field");
    assert!(error.message.contains("pid"));
}

#[test]
fn contains_builds_a_literal_substring_value() {
    let filters = vec![scalar("state", Op::Contains, json!("idle*?"))];
    let search = build_search("os_process", &filters)
        .expect("valid filter")
        .expect("search");
    assert_eq!(search.clauses[0].value, SearchValue::contains("idle*?"));
}

#[test]
fn a_negative_quantity_value_is_rejected() {
    let filters = vec![scalar("rss", Op::Gt, json!(-1))];
    let error = build_search("os_process", &filters)
        .expect_err("quantity fields take non-negative integers");
    assert!(error.message.contains("rss"));
}

#[test]
fn a_non_integer_quantity_value_is_rejected() {
    assert!(
        serde_json::from_value::<FilterInput>(json!({
            "field": "rss",
            "op": "gt",
            "value": 1.5
        }))
        .is_err()
    );
}

#[test]
fn an_identifier_value_may_be_a_decimal_string() {
    let filters = vec![scalar("pid", Op::Eq, json!("4242"))];
    let search = build_search("os_process", &filters)
        .expect("valid filter")
        .expect("search");
    assert!(search.matches_all(|clause| matches!(
        &clause.value,
        SearchValue::Identifier(text) if text == "4242"
    )));
}

#[test]
fn in_builds_one_deduplicated_exact_clause() {
    let filters = vec![FilterInput::In {
        field: "pid".to_owned(),
        values: atoms([json!(42), json!("42"), json!(73)]),
    }];
    let search = build_search("os_process", &filters)
        .expect("valid in filter")
        .expect("search");
    assert_eq!(search.clauses.len(), 1);
    assert!(matches!(
        &search.clauses[0].value,
        SearchValue::AnyOf(values) if values.len() == 2
    ));
}

#[test]
fn in_deduplicates_text_with_the_existing_exact_match_semantics() {
    let filters = vec![FilterInput::In {
        field: "state".to_owned(),
        values: atoms([json!("Running"), json!("running"), json!("idle")]),
    }];
    let search = build_search("os_process", &filters)
        .expect("valid text in filter")
        .expect("search");
    assert!(matches!(
        &search.clauses[0].value,
        SearchValue::AnyOf(values) if values.len() == 2
    ));
}

#[test]
fn in_rejects_empty_too_many_and_quantity_values() {
    let empty = FilterInput::In {
        field: "pid".to_owned(),
        values: Vec::new(),
    };
    assert!(build_search("os_process", &[empty]).is_err());

    let too_many = FilterInput::In {
        field: "pid".to_owned(),
        values: atoms((0..9).map(|value| json!(value))),
    };
    assert!(build_search("os_process", &[too_many]).is_err());

    let quantity = FilterInput::In {
        field: "rss".to_owned(),
        values: atoms([json!(1)]),
    };
    let error = build_search("os_process", &[quantity]).expect_err("quantity in");
    assert_eq!(error.valid_options, ["gt".to_owned(), "lt".to_owned()]);

    let mixed = FilterInput::In {
        field: "pid".to_owned(),
        values: atoms([json!(1), json!("not-an-id")]),
    };
    assert!(build_search("os_process", &[mixed]).is_err());
}

#[test]
fn the_shared_eight_item_bounds_are_accepted() {
    let predicates = (0..8)
        .map(|_| scalar("state", Op::Eq, json!("running")))
        .collect::<Vec<_>>();
    assert!(build_search("os_process", &predicates).is_ok());

    let values = FilterInput::In {
        field: "pid".to_owned(),
        values: atoms((0..8).map(|value| json!(value))),
    };
    let search = build_search("os_process", &[values])
        .expect("eight in values")
        .expect("search");
    assert_eq!(search.clauses.len(), 1);

    let too_many_predicates = (0..9)
        .map(|_| scalar("state", Op::Eq, json!("running")))
        .collect::<Vec<_>>();
    assert!(build_search("os_process", &too_many_predicates).is_err());
}

#[test]
fn identifier_atoms_cover_signed_and_unsigned_integer_extremes() {
    for value in [json!(i64::MIN), json!(i64::MAX), json!(u64::MAX)] {
        let decoded = serde_json::from_value::<FilterInput>(json!({
            "field": "pid",
            "op": "eq",
            "value": value
        }));
        assert!(decoded.is_ok());
    }
    for value in [json!("+1"), json!("01"), json!("-0")] {
        let filter = serde_json::from_value::<FilterInput>(json!({
            "field": "pid",
            "op": "eq",
            "value": value
        }))
        .expect("atom shape");
        assert!(build_search("os_process", &[filter]).is_err());
    }
}

#[test]
fn tagged_filter_shape_rejects_the_other_value_member() {
    assert!(
        serde_json::from_value::<FilterInput>(json!({
            "field": "pid",
            "op": "eq",
            "values": [1]
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<FilterInput>(json!({
            "field": "pid",
            "op": "in",
            "value": 1
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<FilterInput>(json!({
            "field": "pid",
            "op": "eq",
            "value": [1, 2]
        }))
        .is_err()
    );
}

#[test]
fn atom_schema_advertises_only_strings_and_integers() {
    let schema = serde_json::to_value(schemars::schema_for!(FilterAtom)).expect("atom schema");
    let alternatives = schema["anyOf"].as_array().expect("atom alternatives");
    let types = alternatives
        .iter()
        .map(|alternative| alternative["type"].as_str().expect("atom type"))
        .collect::<Vec<_>>();
    assert_eq!(types, vec!["string", "integer", "integer"]);
}
