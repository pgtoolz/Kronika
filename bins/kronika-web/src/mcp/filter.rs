//! Converts MCP filter arrays to the snapshot search engine.
//! Predicates are combined with AND; OR and nested groups are not accepted.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use kronika_query::snapshot::{
    Expr, Quantity, SEARCH_MAX_CLAUSES, SEARCH_MAX_VALUE_CHARS, SearchClause, SearchField,
    SearchFieldKind, SearchOperator, SearchValue, StructuredSearch, search_fields,
    valid_identifier,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    /// Whole-value match: case-insensitive equality on a string field,
    /// exact value on an identifier field.
    Eq,
    Gt,
    Lt,
    /// Case-insensitive substring match, string fields only.
    Contains,
    /// Exact match against any member, string and identifier fields only.
    In,
}

/// JSON atom accepted by a structured predicate. Encoding this restriction in
/// the input schema keeps arrays, objects, booleans, nulls, and floats from
/// being advertised as values the runtime will later refuse.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum FilterAtom {
    Text(String),
    Signed(i64),
    Unsigned(u64),
}

impl FilterAtom {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Signed(_) | Self::Unsigned(_) => None,
        }
    }
}

/// One predicate. Tool handlers combine all predicates with AND; at most
/// 8 predicates per call, the same clause budget the text parser
/// enforces.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FilterInput {
    Eq {
        field: String,
        value: FilterAtom,
    },
    Gt {
        field: String,
        value: FilterAtom,
    },
    Lt {
        field: String,
        value: FilterAtom,
    },
    Contains {
        field: String,
        value: FilterAtom,
    },
    In {
        field: String,
        #[schemars(length(min = 1, max = SEARCH_MAX_CLAUSES))]
        values: Vec<FilterAtom>,
    },
}

impl FilterInput {
    fn field(&self) -> &str {
        match self {
            Self::Eq { field, .. }
            | Self::Gt { field, .. }
            | Self::Lt { field, .. }
            | Self::Contains { field, .. }
            | Self::In { field, .. } => field,
        }
    }

    const fn op(&self) -> Op {
        match self {
            Self::Eq { .. } => Op::Eq,
            Self::Gt { .. } => Op::Gt,
            Self::Lt { .. } => Op::Lt,
            Self::Contains { .. } => Op::Contains,
            Self::In { .. } => Op::In,
        }
    }

    const fn scalar(&self) -> Option<&FilterAtom> {
        match self {
            Self::Eq { value, .. }
            | Self::Gt { value, .. }
            | Self::Lt { value, .. }
            | Self::Contains { value, .. } => Some(value),
            Self::In { .. } => None,
        }
    }
}

/// A refused filter together with the values that a retry can pick from.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) message: String,
    pub(crate) valid_options: Vec<String>,
}

impl Refusal {
    const fn new(message: String) -> Self {
        Self {
            message,
            valid_options: Vec::new(),
        }
    }

    pub(crate) fn into_error(self) -> rmcp::model::CallToolResult {
        super::semantics::mcp_error_with(self.message, self.valid_options)
    }
}

const fn op_name(op: Op) -> &'static str {
    match op {
        Op::Eq => "eq",
        Op::Gt => "gt",
        Op::Lt => "lt",
        Op::Contains => "contains",
        Op::In => "in",
    }
}

fn accepted_ops(field: &SearchField) -> Vec<String> {
    [Op::Eq, Op::In, Op::Gt, Op::Lt, Op::Contains]
        .into_iter()
        .filter(|op| operator_for(field, *op).is_some())
        .map(|op| op_name(op).to_owned())
        .collect()
}

/// Refuses an unknown tagged operator before serde decodes the tagged enum, so
/// the structured MCP error can include the exact alternatives for a known
/// field. Other malformed shapes remain the input decoder's responsibility.
pub(crate) fn validate_filter_operators(
    logical_name: &str,
    arguments: &Map<String, Value>,
) -> Result<(), rmcp::model::CallToolResult> {
    let Some(filters) = arguments.get("filters").and_then(Value::as_array) else {
        return Ok(());
    };
    let fields = search_fields(logical_name);
    for filter in filters {
        let Some(operator) = filter.get("op").and_then(Value::as_str) else {
            continue;
        };
        if ["eq", "in", "gt", "lt", "contains"].contains(&operator) {
            continue;
        }
        let valid_options = filter
            .get("field")
            .and_then(Value::as_str)
            .and_then(|name| fields.iter().find(|field| field.key == name))
            .map_or_else(
                || {
                    ["eq", "in", "gt", "lt", "contains"]
                        .map(str::to_owned)
                        .to_vec()
                },
                accepted_ops,
            );
        return Err(super::semantics::mcp_error_with(
            format!("unknown filter operator {operator:?}"),
            valid_options,
        ));
    }
    Ok(())
}

/// Combines filters with AND. Empty input returns `None`; invalid input reports
/// the first rejected predicate together with the accepted alternatives.
pub(crate) fn build_search(
    logical_name: &str,
    filters: &[FilterInput],
) -> Result<Option<StructuredSearch>, Refusal> {
    if filters.is_empty() {
        return Ok(None);
    }
    if filters.len() > SEARCH_MAX_CLAUSES {
        return Err(Refusal::new(format!(
            "too many filters: {}, the limit is {SEARCH_MAX_CLAUSES}",
            filters.len()
        )));
    }
    let fields = search_fields(logical_name);
    let mut expr: Option<Expr> = None;
    let mut clauses = Vec::with_capacity(filters.len());
    for filter in filters {
        let field = fields
            .iter()
            .find(|candidate| candidate.key == filter.field())
            .ok_or_else(|| {
                let names: Vec<String> = fields.iter().map(|field| field.key.to_owned()).collect();
                Refusal {
                    message: format!(
                        "unknown field for {logical_name}: {}; the filterable fields are {}",
                        filter.field(),
                        names.join(", "),
                    ),
                    valid_options: names,
                }
            })?;
        let operator = operator_for(field, filter.op()).ok_or_else(|| {
            let accepted = accepted_ops(field);
            Refusal {
                message: format!(
                    "operator {} is not valid for field {}: it accepts {}",
                    op_name(filter.op()),
                    filter.field(),
                    accepted.join(", "),
                ),
                valid_options: accepted,
            }
        })?;
        let value = filter_value(field, filter)?;
        let clause = SearchClause::from_parts(field.key, field.columns, operator, value);
        clauses.push(clause.clone());
        expr = Some(match expr {
            None => Expr::Predicate(clause),
            Some(existing) => Expr::And(Box::new(existing), Box::new(Expr::Predicate(clause))),
        });
    }
    let Some(expr) = expr else {
        return Ok(None);
    };
    Ok(Some(StructuredSearch::from_expr(expr, clauses)))
}

const fn operator_for(field: &SearchField, op: Op) -> Option<SearchOperator> {
    match (field.kind, op) {
        (SearchFieldKind::String, Op::Eq | Op::Contains)
        | (SearchFieldKind::String | SearchFieldKind::Identifier { .. }, Op::In)
        | (SearchFieldKind::Identifier { .. }, Op::Eq) => Some(SearchOperator::Colon),
        (SearchFieldKind::Quantity(_), Op::Gt) => Some(SearchOperator::Greater),
        (SearchFieldKind::Quantity(_), Op::Lt) => Some(SearchOperator::Less),
        _ => None,
    }
}

fn filter_value(field: &SearchField, filter: &FilterInput) -> Result<SearchValue, Refusal> {
    if let FilterInput::In { values, .. } = filter {
        if !(1..=SEARCH_MAX_CLAUSES).contains(&values.len()) {
            return Err(Refusal::new(format!(
                "in values must contain between 1 and {SEARCH_MAX_CLAUSES} entries"
            )));
        }
        let mut normalized = Vec::with_capacity(values.len());
        for raw in values {
            validate_text_length(filter.field(), raw)?;
            let value =
                search_value(field, Op::Eq, raw).ok_or_else(|| invalid_value(filter.field()))?;
            if !normalized
                .iter()
                .any(|existing: &SearchValue| existing.same_exact(&value))
            {
                normalized.push(value);
            }
        }
        return Ok(SearchValue::AnyOf(normalized));
    }
    let Some(raw) = filter.scalar() else {
        return Err(invalid_value(filter.field()));
    };
    validate_text_length(filter.field(), raw)?;
    search_value(field, filter.op(), raw).ok_or_else(|| invalid_value(filter.field()))
}

fn validate_text_length(field: &str, value: &FilterAtom) -> Result<(), Refusal> {
    if value
        .text()
        .is_some_and(|text| text.chars().count() > SEARCH_MAX_VALUE_CHARS)
    {
        return Err(Refusal::new(format!(
            "value for {field} is longer than {SEARCH_MAX_VALUE_CHARS} characters"
        )));
    }
    Ok(())
}

fn invalid_value(field: &str) -> Refusal {
    Refusal::new(format!(
        "invalid value for {field}: text uses a string, identifiers use an integer or decimal string, and quantities use a non-negative integer in the documented unit"
    ))
}

/// String matching is case-insensitive. `eq` is whole-value and `contains`
/// is a literal substring, while the text DSL retains its glob syntax.
fn search_value(field: &SearchField, op: Op, value: &FilterAtom) -> Option<SearchValue> {
    match (field.kind, op) {
        (SearchFieldKind::String, Op::Eq) => value.text().map(SearchValue::exact),
        (SearchFieldKind::String, Op::Contains) => value.text().map(SearchValue::contains),
        (SearchFieldKind::String, _) => value.text().map(SearchValue::pattern),
        (SearchFieldKind::Identifier { signed }, _) => identifier_value(value, signed),
        (SearchFieldKind::Quantity(_), _) => quantity_value(value),
    }
}

/// Accepts identifiers as JSON integers or canonical decimal strings; strings
/// preserve signed 64-bit IDs beyond JSON's safe range.
fn identifier_value(value: &FilterAtom, signed: bool) -> Option<SearchValue> {
    let text = match value {
        FilterAtom::Text(text) => text.clone(),
        FilterAtom::Signed(number) if signed => number.to_string(),
        FilterAtom::Signed(number) => u64::try_from(*number).ok()?.to_string(),
        FilterAtom::Unsigned(number) if signed => i64::try_from(*number).ok()?.to_string(),
        FilterAtom::Unsigned(number) => number.to_string(),
    };
    valid_identifier(&text, signed).then_some(SearchValue::Identifier(text))
}

fn quantity_value(value: &FilterAtom) -> Option<SearchValue> {
    let count = match value {
        FilterAtom::Signed(count) => u64::try_from(*count).ok()?,
        FilterAtom::Unsigned(count) => *count,
        FilterAtom::Text(_) => return None,
    };
    Some(SearchValue::Quantity(Quantity::from_integer(u128::from(
        count,
    ))))
}

#[cfg(test)]
mod tests;
