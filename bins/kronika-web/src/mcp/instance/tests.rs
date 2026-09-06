use std::collections::BTreeMap;

use kronika_query::snapshot::PlainRowOut;
use serde_json::{Value, json};

use super::select_settings;
use crate::mcp::catalog::SettingsScopeInput;

fn row(source: Option<Value>, ordinal: u64) -> PlainRowOut {
    let mut fields = BTreeMap::new();
    fields.insert("name".to_owned(), json!(format!("setting-{ordinal}")));
    if let Some(source) = source {
        fields.insert("source".to_owned(), source);
    }
    PlainRowOut {
        segment_id: 1,
        type_id: 2,
        row_ordinal: ordinal,
        at: 3,
        identity: serde_json::Map::new(),
        fields,
    }
}

#[test]
fn non_default_excludes_only_the_exact_default_source() {
    let rows = vec![
        row(Some(json!("default")), 0),
        row(Some(json!("configuration file")), 1),
        row(Some(json!("unknown")), 2),
        row(Some(Value::Null), 3),
        row(None, 4),
    ];
    let (selected, omitted) = select_settings(rows, SettingsScopeInput::NonDefault);
    assert!(omitted);
    assert_eq!(
        selected
            .iter()
            .map(|row| row.row_ordinal)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

#[test]
fn all_preserves_every_row_and_reports_no_omission() {
    let rows = vec![
        row(Some(json!("default")), 0),
        row(Some(json!("override")), 1),
    ];
    let (selected, omitted) = select_settings(rows, SettingsScopeInput::All);
    assert!(!omitted);
    assert_eq!(selected.len(), 2);
}
