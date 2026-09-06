//! JS-safe JSON rendering for registry, dictionary, and index values.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kronika_reader::{Cell, Dictionary, Resolved};
use kronika_registry::{Column, TypeContract, section_implementation};
use serde_json::{Value, json};

use crate::QueryError;

pub(crate) fn record(value: impl std::borrow::Borrow<Value>) -> Result<Vec<u8>, QueryError> {
    let mut bytes = serde_json::to_vec(value.borrow())?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(crate) fn projected_layout(
    logical_name: &str,
    contract: &'static TypeContract,
    fields: &[(&str, Option<&Column>)],
) -> Value {
    json!({
        "logical_name": logical_name,
        "physical_name": contract.name,
        "type_id": contract.type_id.get().to_string(),
        "implementation": section_implementation(contract.type_id.get()),
        "identity": contract.identity,
        "columns": fields
            .iter()
            .map(|(name, selected)| selected.map_or_else(
                || json!({ "name": name, "available": false }),
                |column| {
                    let mut value = column_value(column);
                    value.insert("name".to_owned(), json!(name));
                    value.insert("available".to_owned(), json!(true));
                    Value::Object(value)
                },
            ))
            .collect::<Vec<_>>(),
    })
}

fn column_value(column: &Column) -> serde_json::Map<String, Value> {
    let mut value = serde_json::Map::new();
    value.insert("name".to_owned(), json!(column.name));
    value.insert("type".to_owned(), json!(column.ty.code()));
    value.insert("class".to_owned(), json!(column.class.code()));
    value.insert(
        "unit".to_owned(),
        json!(column.unit.map_or("none", kronika_registry::Unit::code)),
    );
    value.insert("nullable".to_owned(), json!(column.nullable));
    value
}

pub(crate) fn cell(cell: &Cell, dictionary: &Dictionary) -> Result<Value, QueryError> {
    Ok(match cell {
        Cell::Null => Value::Null,
        Cell::I16(value) => json!(value),
        Cell::I32(value) => json!(value),
        Cell::I64(value) | Cell::Ts(value) => Value::String(value.to_string()),
        Cell::U32(value) => json!(value),
        Cell::U64(value) => Value::String(value.to_string()),
        Cell::F64(value) => finite(*value),
        Cell::Bool(value) => json!(value),
        Cell::ListI32(values) => json!(values),
        Cell::StrId(id) => match dictionary.resolve(*id) {
            Some(Resolved::Str(bytes)) => bytes_value(bytes),
            Some(Resolved::Blob(blob)) => blob_value(
                blob.stored_bytes,
                blob.full_len,
                blob.truncated,
                blob.full_sha256,
            ),
            None => {
                return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unresolved dictionary id {id}"),
                ))));
            }
        },
    })
}

fn finite(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or_else(
        || {
            let code = if value.is_nan() {
                "nan"
            } else if value.is_sign_positive() {
                "positive_infinity"
            } else {
                "negative_infinity"
            };
            json!({
                "representation": "non_finite",
                "value": code,
            })
        },
        Value::Number,
    )
}

pub(crate) fn shorten(mut value: Value, limit: u64) -> Value {
    match &mut value {
        Value::String(text) => shorten_text(text, limit),
        Value::Object(payload) if payload.get("representation") == Some(&json!("text")) => {
            if let Some(Value::String(text)) = payload.get_mut("stored_text") {
                shorten_text(text, limit);
            }
        }
        _ => {}
    }
    value
}

fn shorten_text(text: &mut String, limit: u64) {
    let Ok(limit) = usize::try_from(limit) else {
        return;
    };
    if text.chars().count() <= limit {
        return;
    }
    let mut kept: String = text.chars().take(limit).collect();
    kept.push('…');
    *text = kept;
}

fn bytes_value(bytes: &[u8]) -> Value {
    std::str::from_utf8(bytes).map_or_else(
        |_invalid| {
            json!({
                "representation": "bytes",
                "base64": STANDARD.encode(bytes),
            })
        },
        |text| Value::String(text.to_owned()),
    )
}

fn blob_value(stored: &[u8], full_len: u64, truncated: bool, hash: Option<[u8; 32]>) -> Value {
    let mut value = serde_json::Map::new();
    match std::str::from_utf8(stored) {
        Ok(text) => {
            value.insert("representation".to_owned(), json!("text"));
            value.insert("stored_text".to_owned(), json!(text));
        }
        Err(_invalid) => {
            value.insert("representation".to_owned(), json!("bytes"));
            value.insert("stored_base64".to_owned(), json!(STANDARD.encode(stored)));
        }
    }
    value.insert("full_len".to_owned(), Value::String(full_len.to_string()));
    value.insert("truncated".to_owned(), json!(truncated));
    value.insert(
        "sha256".to_owned(),
        hash.map_or(Value::Null, |bytes| Value::String(hex(&bytes))),
    );
    Value::Object(value)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests;
