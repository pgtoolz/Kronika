use super::{
    bounded_limit, invalid_arguments, mcp_error, mcp_error_with, mcp_structured, storage_error,
};
use crate::api::ApiError;

#[test]
fn bounded_limit_accepts_a_value_within_the_cap() {
    assert_eq!(bounded_limit("limit", 10, 5_000), Ok(10));
    assert_eq!(bounded_limit("limit", 5_000, 5_000), Ok(5_000));
}

#[test]
fn bounded_limit_rejects_zero_and_values_above_the_cap() {
    let over = bounded_limit("limit", 4_000_000_000, 5_000).expect_err("over cap");
    let message = over.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("limit") && message.contains("5000") && message.contains("4000000000"),
        "error must name the field, the cap and the rejected value: {message}"
    );

    let zero = bounded_limit("limit", 0, 5_000).expect_err("zero");
    let message = zero.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("limit"),
        "error must name the field: {message}"
    );
}

#[test]
fn mcp_structured_mirrors_complete_json_without_losing_wide_integers() {
    let expected = serde_json::json!({
        "sections": ["os_process"],
        "unsigned": u64::MAX,
        "signed": i64::MIN,
    });
    let result = mcp_structured(expected.clone());
    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.structured_content, Some(expected.clone()));
    assert_eq!(result.content.len(), 1);
    let text = &result.content[0].as_text().expect("JSON text").text;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).expect("content JSON"),
        expected
    );
}

#[test]
fn mcp_structured_has_no_artificial_result_size_cap() {
    let expected = serde_json::json!({"text": "x".repeat(8 * 1024 * 1024 + 1)});
    let result = mcp_structured(expected.clone());
    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.structured_content, Some(expected.clone()));
    let text = &result.content[0].as_text().expect("JSON text").text;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).expect("content JSON"),
        expected
    );
}

#[test]
fn an_error_mirrors_its_text_into_structured_content() {
    let result = mcp_error("no such sort field");
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["record"], "error");
    assert_eq!(structured["message"], "no such sort field");
    assert!(structured.get("valid_options").is_none());
}

#[test]
fn a_refusal_with_choices_carries_valid_options() {
    let result = mcp_error_with(
        "operator eq is not valid",
        vec!["gt".to_owned(), "lt".to_owned()],
    );
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["valid_options"], serde_json::json!(["gt", "lt"]));
}

#[test]
fn rejected_arguments_name_the_tool_and_its_usage() {
    let result = invalid_arguments(
        "kronika_find_events",
        "from, to, and limit are required",
        "missing field `limit`",
    );
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("kronika_find_events")
            && message.contains("Usage: from, to, and limit are required"),
        "{message}"
    );
}

#[test]
fn a_missing_section_or_column_error_names_the_listing_tool() {
    let section = storage_error(&ApiError::NoSuchSection);
    let message = section.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("kronika_list_recorded_sections lists recorded sections"),
        "{message}"
    );

    let column = storage_error(&ApiError::NoSuchColumn("rss".to_owned()));
    let message = column.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("kronika_list_recorded_sections"),
        "{message}"
    );

    let unreadable = storage_error(&ApiError::Unreadable(Box::new(std::io::Error::other(
        "broken",
    ))));
    let message = unreadable.content[0].as_text().expect("text").text.clone();
    assert!(
        !message.contains("kronika_list_recorded_sections"),
        "an unreadable store has no listing to point at: {message}"
    );
}

#[test]
fn storage_errors_do_not_publish_internal_coordinate_names() {
    for coordinate in [
        "detail_locator",
        "type_id",
        "segment_id",
        "row_ordinal",
        "row_key",
    ] {
        let result = storage_error(&ApiError::BadLocator(format!("invalid {coordinate}")));
        let message = &result.content[0].as_text().expect("text").text;
        assert_eq!(message, "could not produce detail_ref");
    }

    let result = storage_error(&ApiError::BadLocator(
        "cannot emit detail_ref: row identity is not unique".to_owned(),
    ));
    let message = &result.content[0].as_text().expect("text").text;
    assert_eq!(message, "could not produce detail_ref");

    for private_message in ["row record has no ordinal", "unresolved dictionary id 92"] {
        let result = storage_error(&ApiError::Unreadable(Box::new(std::io::Error::other(
            private_message,
        ))));
        let message = &result.content[0].as_text().expect("text").text;
        assert_eq!(message, "could not read stored data");
        assert!(!message.contains(private_message));
    }

    for error in [
        ApiError::BadFilter("type_id".to_owned()),
        ApiError::NoSuchColumn("segment_id".to_owned()),
    ] {
        let result = storage_error(&error);
        let message = &result.content[0].as_text().expect("text").text;
        assert!(!message.contains("type_id"), "{message}");
        assert!(!message.contains("segment_id"), "{message}");
    }
}
