use super::lines;

#[test]
fn malformed_ndjson_names_its_line() {
    let error = lines("{\"kind\":\"segment\"}\nnot JSON\n")
        .expect_err("the malformed second line must fail parsing");

    assert_eq!(error.to_string(), "parse dumper JSON line 2");
}

#[test]
fn row_fields_are_distinct_from_provenance() {
    let printed = concat!(
        "{\"kind\":\"row\",\"path\":\"one.zms\",\"type_id\":1021001,",
        "\"row\":{\"path\":\"inside\",\"environment\":1}}\n"
    );
    let rows = lines(printed).expect("the row is valid JSON");

    assert_eq!(rows[0].get("path").as_deref(), Some("one.zms"));
    assert_eq!(rows[0].row_get("path").as_deref(), Some("inside"));
    assert_eq!(rows[0].row_number("environment"), Some(1));
}
