use std::collections::BTreeSet;

use serde_json::Value;

use super::exclusive_recorded_range;
use crate::config::{Account, Config};
use crate::tests::artifacts::Fixture;

fn test_config(data_root: std::path::PathBuf) -> Config {
    Config {
        data_root,
        listen: "127.0.0.1:0".parse().expect("listen address"),
        account: Account {
            user: "dba".to_owned(),
            password: "secret".to_owned(),
        },
        authentication_required: true,
        sources: crate::config::SOURCE_OS | crate::config::SOURCE_POSTGRESQL,
        synthetic_demo: false,
        export_gate: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
    }
}

fn assert_no_internal_catalog_fields(value: &Value) {
    match value {
        Value::Object(object) => {
            for field in [
                "detail_locator",
                "type_id",
                "segment_id",
                "row_ordinal",
                "row_key",
                "physical_name",
                "identity",
                "implementation",
            ] {
                assert!(
                    !object.contains_key(field),
                    "context exposed {field}: {value:#?}",
                );
            }
            for child in object.values() {
                assert_no_internal_catalog_fields(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                assert_no_internal_catalog_fields(child);
            }
        }
        _ => {}
    }
}

#[test]
fn recorded_range_becomes_a_checked_half_open_range() {
    assert_eq!(exclusive_recorded_range(None), Ok(None));
    assert_eq!(
        exclusive_recorded_range(Some((100, 300))),
        Ok(Some((100, 301)))
    );
    assert_eq!(
        exclusive_recorded_range(Some((0, i64::MAX))),
        Err("last recorded timestamp cannot form an exclusive upper bound")
    );
}

#[test]
fn context_lists_only_logical_product_sections_and_fields() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (300, 102, 60, "beta")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let result = super::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let output = result.structured_content.expect("structured content");
    assert_no_internal_catalog_fields(&output);
    let sections = output["sections"].as_array().expect("sections");
    let process = sections
        .iter()
        .find(|section| section["logical_name"] == "os_process")
        .expect("process section");
    let keys = process
        .as_object()
        .expect("section object")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from(["bytes", "fields", "logical_name", "rows", "source_family"]),
    );
    assert_eq!(process["source_family"], "os");
    assert_eq!(process["rows"], "2");
    for field in process["fields"].as_array().expect("field catalog") {
        let keys = field
            .as_object()
            .expect("field object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(keys, BTreeSet::from(["class", "name", "unit"]));
    }
}
