//! Stable row identity carried alongside physical locator hints.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kronika_reader::{Cell, Row};
use kronika_registry::{ColumnClass, Semantics, TypeContract, contract, logical_section_name};
use serde::Serialize;
use serde_json::{Map, Value};

const DETAIL_REF_VERSION: u8 = 1;
const DETAIL_REF_CHECKSUM_BYTES: usize = size_of::<u32>();
/// Maximum accepted byte length of one encoded detail reference.
pub const DETAIL_REF_MAX_ENCODED_BYTES: usize = 8 * 1024;

type DetailPayload = (u8, String, i64, i64, u32, u64, RowIdentity);

/// Complete registry identity kept inside an opaque detail reference.
pub type RowIdentity = Map<String, Value>;

/// Stable logical row identity with an optional physical-position hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DetailLocator {
    pub(crate) section: String,
    #[serde(serialize_with = "serialize_decimal")]
    pub(crate) segment_id: i64,
    #[serde(serialize_with = "serialize_decimal")]
    pub(crate) at: i64,
    #[serde(serialize_with = "serialize_decimal")]
    pub(crate) type_id: u32,
    #[serde(serialize_with = "serialize_decimal")]
    pub(crate) row_ordinal: u64,
    pub(crate) identity: RowIdentity,
}

/// Builds one stable locator. The ordinal is only a physical hint.
#[must_use]
pub fn detail_locator(
    section: &str,
    segment_id: i64,
    at: i64,
    type_id: u32,
    row_ordinal: u64,
    identity: RowIdentity,
) -> DetailLocator {
    DetailLocator {
        section: section.to_owned(),
        segment_id,
        at,
        type_id,
        row_ordinal,
        identity,
    }
}

impl DetailLocator {
    /// Encodes the locator as one opaque, stateless value for adapters.
    ///
    /// # Errors
    ///
    /// Returns an explanation when the locator is invalid or oversized.
    pub fn detail_ref(&self) -> Result<String, String> {
        if logical_section_name(self.type_id) != Some(self.section.as_str())
            || validate(self.type_id, &self.identity).is_err()
        {
            return Err("cannot encode an invalid row locator".to_owned());
        }
        let payload = serde_json::to_vec(&(
            DETAIL_REF_VERSION,
            &self.section,
            self.segment_id,
            self.at,
            self.type_id,
            self.row_ordinal,
            &self.identity,
        ))
        .map_err(|error| format!("encode row locator: {error}"))?;
        let detail_ref = encode_payload(payload);
        if detail_ref.len() > DETAIL_REF_MAX_ENCODED_BYTES {
            return Err("cannot encode an oversized row locator".to_owned());
        }
        Ok(detail_ref)
    }

    /// Decodes and validates the one current detail-reference format.
    pub(crate) fn from_detail_ref(detail_ref: &str) -> Result<Self, String> {
        if detail_ref.is_empty() || detail_ref.len() > DETAIL_REF_MAX_ENCODED_BYTES {
            return Err("detail_ref length is invalid".to_owned());
        }
        let encoded = URL_SAFE_NO_PAD
            .decode(detail_ref)
            .map_err(|_error| "detail_ref is not canonical URL-safe base64".to_owned())?;
        if URL_SAFE_NO_PAD.encode(&encoded) != detail_ref {
            return Err("detail_ref is not canonical URL-safe base64".to_owned());
        }
        let payload_len = encoded
            .len()
            .checked_sub(DETAIL_REF_CHECKSUM_BYTES)
            .ok_or_else(|| "detail_ref is truncated".to_owned())?;
        let (payload, checksum) = encoded.split_at(payload_len);
        let checksum = u32::from_le_bytes(
            checksum
                .try_into()
                .map_err(|_error| "detail_ref is truncated".to_owned())?,
        );
        if kronika_format::crc32c(payload) != checksum {
            return Err("detail_ref checksum does not match".to_owned());
        }
        let decoded: DetailPayload = serde_json::from_slice(payload)
            .map_err(|error| format!("detail_ref payload is invalid: {error}"))?;
        let canonical = serde_json::to_vec(&decoded)
            .map_err(|error| format!("encode detail_ref payload: {error}"))?;
        if canonical != payload {
            return Err("detail_ref payload is not canonical".to_owned());
        }
        let (version, section, segment_id, at, type_id, row_ordinal, identity) = decoded;
        if version != DETAIL_REF_VERSION {
            return Err(format!("unsupported detail_ref version {version}"));
        }
        if logical_section_name(type_id) != Some(section.as_str()) {
            return Err("detail_ref section does not match its row layout".to_owned());
        }
        validate(type_id, &identity)
            .map_err(|_error| "detail_ref row identity is invalid".to_owned())?;
        Ok(Self {
            section,
            segment_id,
            at,
            type_id,
            row_ordinal,
            identity,
        })
    }
}

fn encode_payload(mut payload: Vec<u8>) -> String {
    let checksum = kronika_format::crc32c(&payload);
    payload.extend_from_slice(&checksum.to_le_bytes());
    URL_SAFE_NO_PAD.encode(payload)
}

pub(super) fn serialize_decimal<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: std::fmt::Display,
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

/// Columns forming the durable locator identity for a physical layout.
///
/// Snapshot-like sources use their declared cross-snapshot registry identity.
/// Event streams can repeat a semantic key at one timestamp, so the complete
/// non-timestamp stored row is their identity.
pub fn identity_columns(contract: &'static TypeContract) -> impl Iterator<Item = &'static str> {
    let event = contract.semantics == Semantics::EventStream;
    contract
        .columns
        .iter()
        .filter(move |column| {
            if event {
                column.class != ColumnClass::Timestamp
            } else {
                contract.identity.contains(&column.name)
            }
        })
        .map(|column| column.name)
}

/// Encodes one row's identity without resolving dictionary-backed payloads.
///
/// # Errors
///
/// Returns an explanation when the type or decoded row is incompatible with
/// the registry identity contract.
pub fn identity(type_id: u32, row: &Row) -> Result<RowIdentity, String> {
    let contract =
        contract(type_id).ok_or_else(|| format!("type_id {type_id} has no registry contract"))?;
    if contract.type_id.get() != row.contract().type_id.get() {
        return Err(format!(
            "type_id {type_id} does not match decoded row type_id {}",
            row.contract().type_id.get()
        ));
    }
    identity_columns(contract)
        .map(|name| {
            row.get(name)
                .map(|cell| (name.to_owned(), identity_value(cell)))
                .ok_or_else(|| format!("type_id {type_id} identity column {name:?} is absent"))
        })
        .collect()
}

/// Validates that an input carries exactly the registry identity members.
///
/// # Errors
///
/// Returns an explanation naming missing or unexpected identity members.
pub fn validate(type_id: u32, requested: &RowIdentity) -> Result<(), String> {
    let contract =
        contract(type_id).ok_or_else(|| format!("type_id {type_id} has no registry contract"))?;
    let expected = identity_columns(contract).collect::<Vec<_>>();
    let missing = expected
        .iter()
        .copied()
        .filter(|name| !requested.contains_key(*name))
        .collect::<Vec<_>>();
    let extra = requested
        .keys()
        .filter(|name| !expected.contains(&name.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if missing.is_empty() && extra.is_empty() {
        return Ok(());
    }
    Err(format!(
        "invalid detail_locator identity for type_id {type_id}: expected [{}], missing [{}], unexpected [{}]",
        expected.join(", "),
        missing.join(", "),
        extra.join(", "),
    ))
}

fn identity_value(cell: &Cell) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::Bool(value) => Value::Bool(*value),
        Cell::I16(value) => Value::String(value.to_string()),
        Cell::I32(value) => Value::String(value.to_string()),
        Cell::I64(value) | Cell::Ts(value) => Value::String(value.to_string()),
        Cell::U32(value) => Value::String(value.to_string()),
        Cell::U64(value) | Cell::StrId(value) => Value::String(value.to_string()),
        Cell::F64(value) => Value::String(format!("f64:{:016x}", value.to_bits())),
        Cell::ListI32(values) => Value::Array(
            values
                .iter()
                .map(|value| Value::String(value.to_string()))
                .collect(),
        ),
    }
}

/// Stored text kept out of mass results and returned only by row detail.
#[must_use]
pub fn is_detail_text(section: &str, field: &str) -> bool {
    matches!(
        (section, field),
        ("os_process", "cmdline")
            | (
                "pg_stat_activity" | "pg_locks" | "pg_stat_statements",
                "query"
            )
            | ("pg_store_plans", "plan")
            | (
                "pg_log_errors",
                "sample" | "detail" | "hint" | "context" | "statement"
            )
            | ("pg_log_slow_queries", "sample")
            | ("pg_log_checkpoints", "reason")
            | ("pg_log_lock_waits", "detail" | "context" | "statement")
            | ("pg_log_temp_files", "statement")
            | ("pg_log_lifecycle", "message" | "query_detail")
            | ("pgbouncer_events", "text")
    )
}

#[cfg(test)]
mod tests;
