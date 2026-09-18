mod fields;
mod glob;
mod quantity;

pub use fields::{result_field, search_fields};
pub use glob::GlobPattern;

use quantity::{byte_multiplier, duration_factors, parse_quantity};

/// Maximum predicates accepted in one structured snapshot search.
pub const SEARCH_MAX_CLAUSES: usize = 8;
/// Maximum Unicode scalar values accepted in one search value.
pub const SEARCH_MAX_VALUE_CHARS: usize = 256;
const SEARCH_MAX_EXPRESSION_CHARS: usize = 1_024;
const SEARCH_MAX_GROUP_DEPTH: usize = 4;
const SEARCH_MAX_TOKENS: usize = 31;
const SEARCH_MAX_SIGNIFICANT_DIGITS: usize = 38;
const SEARCH_MAX_FRACTIONAL_DIGITS: usize = 9;

/// Parsed and canonicalized bounded snapshot search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredSearch {
    /// Boolean expression evaluated for this search.
    pub expr: Expr,
    /// Predicates in source order, used for projection planning.
    pub clauses: Vec<SearchClause>,
    canonical: String,
}

/// Boolean expression over structured snapshot predicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// One predicate.
    Predicate(SearchClause),
    /// Both operands must match.
    And(Box<Self>, Box<Self>),
    /// Either operand may match.
    Or {
        /// Left operand.
        left: Box<Self>,
        /// Right operand.
        right: Box<Self>,
        /// Byte span of the operator, retained for diagnostics.
        operator_span: (usize, usize),
    },
}

/// One resolved structured-search predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchClause {
    canonical: String,
    /// Canonical public field name.
    pub key: &'static str,
    /// Physical columns needed to evaluate the predicate.
    pub columns: &'static [&'static str],
    /// Comparison operation.
    pub operator: SearchOperator,
    /// Typed comparison value.
    pub value: SearchValue,
}

impl SearchClause {
    /// Constructs a typed clause for MCP. `canonical` is empty because typed
    /// MCP searches never bind or resume HTTP cursors.
    #[must_use]
    pub const fn from_parts(
        key: &'static str,
        columns: &'static [&'static str],
        operator: SearchOperator,
        value: SearchValue,
    ) -> Self {
        Self {
            canonical: String::new(),
            key,
            columns,
            operator,
            value,
        }
    }
}

/// Operation supported by a structured-search predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOperator {
    /// Identifier, text, or equality match.
    Colon,
    /// Strict greater-than comparison.
    Greater,
    /// Strict less-than comparison.
    Less,
}

/// Typed right-hand value of a structured-search predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchValue {
    /// Canonical signed or unsigned identifier.
    Identifier(String),
    /// Case-insensitive text pattern.
    Pattern(GlobPattern),
    /// Exact rational quantity.
    Quantity(Quantity),
    /// Typed disjunction used by MCP `in` filters.
    AnyOf(Vec<Self>),
}

impl SearchValue {
    /// Builds the text parser's case-insensitive substring glob. `*` and `?`
    /// retain their glob semantics.
    #[must_use]
    pub fn pattern(raw: &str) -> Self {
        Self::Pattern(GlobPattern::new(raw))
    }

    /// Builds a case-insensitive literal substring pattern for typed MCP
    /// filters. Unlike the text DSL, `*` and `?` have no wildcard meaning.
    #[must_use]
    pub fn contains(raw: &str) -> Self {
        Self::Pattern(GlobPattern::contains(raw))
    }

    /// Wrap raw text in an anchored, literal-only pattern: whole-value,
    /// case-insensitive equality — no substring behavior, `*`/`?` taken
    /// literally. The text DSL cannot express this; the typed MCP filter
    /// input uses it for its `eq` operator on string fields.
    #[must_use]
    pub fn exact(raw: &str) -> Self {
        Self::Pattern(GlobPattern::exact(raw))
    }

    /// Whether two identifier or pattern values are exact duplicates.
    #[must_use]
    pub fn same_exact(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Identifier(left), Self::Identifier(right)) => left == right,
            (Self::Pattern(left), Self::Pattern(right)) => left.same_exact(right),
            _ => false,
        }
    }
}

#[must_use]
/// Match searchable text against a typed search value.
pub fn search_value_matches(text: &str, value: &SearchValue) -> bool {
    match value {
        SearchValue::Identifier(wanted) => text == wanted,
        SearchValue::Pattern(pattern) => pattern.matches(text),
        SearchValue::Quantity(_) => false,
        SearchValue::AnyOf(values) => values
            .iter()
            .any(|candidate| search_value_matches(text, candidate)),
    }
}

/// Non-negative exact rational quantity in a field's comparison unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quantity {
    /// Exact numerator.
    pub numerator: u128,
    /// Non-zero exact denominator.
    pub denominator: u128,
    canonical: String,
}

impl Quantity {
    /// Builds a non-negative integer threshold already expressed in the field's
    /// comparison unit.
    #[must_use]
    pub fn from_integer(value: u128) -> Self {
        Self {
            numerator: value,
            denominator: 1,
            canonical: value.to_string(),
        }
    }
}

/// Units accepted by a numeric structured-search field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityKind {
    /// Bytes per second.
    ByteRate,
    /// Bytes.
    Bytes,
    /// Integral count.
    Count,
    /// Count per second.
    CountRate,
    /// Duration.
    Duration,
    /// Duration per second.
    DurationRate,
    /// Percentage.
    Percentage,
    /// Unitless scalar.
    Scalar,
}

/// Derived metric and projection dependencies for a numeric search field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultField {
    /// Public metric name evaluated after aggregation or rate calculation.
    pub metric: &'static str,
    /// Metric comparison unit.
    pub kind: QuantityKind,
    /// Physical columns needed to derive the metric.
    pub dependencies: &'static [&'static str],
}

/// Public structured-search field definition for one snapshot surface.
#[derive(Debug, Clone, Copy)]
pub struct SearchField {
    /// Canonical field name.
    pub key: &'static str,
    aliases: &'static [&'static str],
    /// Physical columns searched for member values.
    pub columns: &'static [&'static str],
    /// Field value and phase semantics.
    pub kind: SearchFieldKind,
}

/// Value and evaluation phase of a structured-search field.
#[derive(Debug, Clone, Copy)]
pub enum SearchFieldKind {
    /// Signed or unsigned canonical identifier.
    Identifier {
        /// Whether a leading minus sign is accepted.
        signed: bool,
    },
    /// Case-insensitive text.
    String,
    /// Derived numeric result.
    Quantity(ResultField),
}

/// Stable structured-search validation diagnostic with a byte span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchDiagnostic {
    /// Stable machine-readable diagnostic code.
    pub code: &'static str,
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl StructuredSearch {
    /// Parse and validate a bounded search for one logical snapshot section.
    ///
    /// # Errors
    ///
    /// Returns a diagnostic when the expression exceeds a bound or contains
    /// invalid syntax, fields, values, or phase combinations.
    pub fn parse(raw: &str, logical_name: &str) -> Result<Self, SearchDiagnostic> {
        if raw.chars().count() > SEARCH_MAX_EXPRESSION_CHARS {
            return Err(diagnostic("expression_too_long", 0, raw.len()));
        }
        let fields = search_fields(logical_name);
        if fields.is_empty() {
            return Err(diagnostic("unknown_field", 0, raw.len()));
        }
        let first = skip_space(raw, 0);
        if first == raw.len() {
            return Err(diagnostic("missing_value", first, first));
        }
        if let Some((start, end)) = first_unsupported(raw) {
            return Err(diagnostic("unsupported_syntax", start, end));
        }
        if !has_structured_syntax(raw) {
            let value = raw.trim();
            if value.chars().count() > SEARCH_MAX_VALUE_CHARS {
                return Err(diagnostic("value_too_long", first, raw.len()));
            }
            let clause = SearchClause {
                canonical: value.to_owned(),
                key: "text",
                columns: fields
                    .iter()
                    .find(|field| field.key == "text")
                    .map_or(&[], |field| field.columns),
                operator: SearchOperator::Colon,
                value: SearchValue::Pattern(GlobPattern::new(value)),
            };
            return Ok(Self {
                expr: Expr::Predicate(clause.clone()),
                clauses: vec![clause],
                canonical: value.to_owned(),
            });
        }
        let mut parser = Parser::new(raw, fields, first);
        let expr = parser.parse_expression(0)?;
        parser.cursor = skip_space(raw, parser.cursor);
        if parser.cursor != raw.len() {
            if raw.as_bytes().get(parser.cursor) == Some(&b')') {
                return Err(diagnostic(
                    "unbalanced_parenthesis",
                    parser.cursor,
                    parser.cursor + 1,
                ));
            }
            return Err(diagnostic(
                "expected_boolean_operator",
                parser.cursor,
                next_token(raw, parser.cursor),
            ));
        }
        let canonical = canonical_expr(&expr, 0);
        Ok(Self {
            expr,
            clauses: parser.clauses,
            canonical,
        })
    }

    /// Canonical expression used to bind HTTP page cursors.
    #[must_use]
    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    /// Constructs a typed MCP search. `canonical` is empty because this path
    /// never creates or validates an HTTP snapshot cursor.
    #[must_use]
    pub const fn from_expr(expr: Expr, clauses: Vec<SearchClause>) -> Self {
        Self {
            expr,
            clauses,
            canonical: String::new(),
        }
    }

    /// Member-phase predicates needed while scanning physical rows.
    pub fn member_clauses(&self) -> impl Iterator<Item = &SearchClause> {
        self.clauses
            .iter()
            .filter(|clause| !matches!(clause.value, SearchValue::Quantity(_)))
    }

    /// Reject an `OR` that mixes member and derived-result phases.
    ///
    /// # Errors
    ///
    /// Returns a diagnostic when one disjunction crosses evaluation phases.
    pub fn validate_grouped_phase(&self) -> Result<(), SearchDiagnostic> {
        phase(&self.expr).map(|_phase| ())
    }

    /// Evaluate the member phase, treating result predicates as deferred matches.
    pub fn matches_member(&self, mut predicate: impl FnMut(&SearchClause) -> bool) -> bool {
        evaluate(&self.expr, &mut |clause| {
            matches!(clause.value, SearchValue::Quantity(_)) || predicate(clause)
        })
    }

    /// Evaluate the result phase, treating member predicates as prior matches.
    pub fn matches_result(&self, mut predicate: impl FnMut(&SearchClause) -> bool) -> bool {
        evaluate(&self.expr, &mut |clause| {
            !matches!(clause.value, SearchValue::Quantity(_)) || predicate(clause)
        })
    }

    /// Evaluate every predicate through one typed callback.
    pub fn matches_all(&self, mut predicate: impl FnMut(&SearchClause) -> bool) -> bool {
        evaluate(&self.expr, &mut predicate)
    }

    /// Derived-result predicates and their metric definitions.
    pub fn result_clauses(
        &self,
        logical_name: &str,
    ) -> impl Iterator<Item = (&SearchClause, ResultField)> {
        self.clauses.iter().filter_map(|clause| {
            let field = search_fields(logical_name)
                .iter()
                .find(|field| field.key == clause.key)?;
            match field.kind {
                SearchFieldKind::Quantity(result) => Some((clause, result)),
                SearchFieldKind::Identifier { .. } | SearchFieldKind::String => None,
            }
        })
    }

    /// Exact query identifier requested by a valid first-match expression.
    #[must_use]
    pub fn first_match_query_id(&self) -> Option<i64> {
        let Expr::Predicate(clause) = &self.expr else {
            return None;
        };
        let SearchValue::Identifier(value) = &clause.value else {
            return None;
        };
        (clause.key == "query_id" && clause.operator == SearchOperator::Colon)
            .then(|| value.parse().ok())
            .flatten()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Member,
    Result,
    Both,
}

fn phase(expr: &Expr) -> Result<Phase, SearchDiagnostic> {
    match expr {
        Expr::Predicate(clause) => Ok(if matches!(clause.value, SearchValue::Quantity(_)) {
            Phase::Result
        } else {
            Phase::Member
        }),
        Expr::And(left, right) => Ok(match (phase(left)?, phase(right)?) {
            (Phase::Member, Phase::Member) => Phase::Member,
            (Phase::Result, Phase::Result) => Phase::Result,
            _ => Phase::Both,
        }),
        Expr::Or {
            left,
            right,
            operator_span,
        } => {
            let left = phase(left)?;
            let right = phase(right)?;
            if left == right && !matches!(left, Phase::Both) {
                Ok(left)
            } else {
                Err(diagnostic(
                    "mixed_phase_or",
                    operator_span.0,
                    operator_span.1,
                ))
            }
        }
    }
}

fn evaluate(expr: &Expr, predicate: &mut impl FnMut(&SearchClause) -> bool) -> bool {
    match expr {
        Expr::Predicate(clause) => predicate(clause),
        Expr::And(left, right) => evaluate(left, predicate) && evaluate(right, predicate),
        Expr::Or { left, right, .. } => evaluate(left, predicate) || evaluate(right, predicate),
    }
}

fn canonical_expr(expr: &Expr, parent_precedence: u8) -> String {
    let (precedence, rendered) = match expr {
        Expr::Predicate(clause) => (3, canonical_clause(clause)),
        Expr::And(left, right) => (
            2,
            format!(
                "{} AND {}",
                canonical_expr(left, 2),
                canonical_expr(right, 2)
            ),
        ),
        Expr::Or { left, right, .. } => (
            1,
            format!(
                "{} OR {}",
                canonical_expr(left, 1),
                canonical_expr(right, 1)
            ),
        ),
    };
    if precedence < parent_precedence {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn canonical_clause(clause: &SearchClause) -> String {
    clause.canonical.clone()
}

struct Parser<'a> {
    raw: &'a str,
    fields: &'static [SearchField],
    cursor: usize,
    clauses: Vec<SearchClause>,
    tokens: usize,
}

impl<'a> Parser<'a> {
    const fn new(raw: &'a str, fields: &'static [SearchField], cursor: usize) -> Self {
        Self {
            raw,
            fields,
            cursor,
            clauses: Vec::new(),
            tokens: 0,
        }
    }

    fn parse_expression(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        self.parse_or(depth)
    }

    fn parse_or(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        let mut left = self.parse_and(depth)?;
        loop {
            self.cursor = skip_space(self.raw, self.cursor);
            let Some((start, end)) = keyword_at(self.raw, self.cursor, "OR") else {
                return Ok(left);
            };
            self.consume_token(start, end)?;
            self.cursor = skip_space(self.raw, end);
            self.require_operand(start, end)?;
            let right = self.parse_and(depth)?;
            left = Expr::Or {
                left: Box::new(left),
                right: Box::new(right),
                operator_span: (start, end),
            };
        }
    }

    fn parse_and(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        let mut left = self.parse_primary(depth)?;
        loop {
            self.cursor = skip_space(self.raw, self.cursor);
            let Some((start, end)) = keyword_at(self.raw, self.cursor, "AND") else {
                return Ok(left);
            };
            self.consume_token(start, end)?;
            self.cursor = skip_space(self.raw, end);
            self.require_operand(start, end)?;
            let right = self.parse_primary(depth)?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
    }

    fn parse_primary(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        self.cursor = skip_space(self.raw, self.cursor);
        if let Some((start, end)) = keyword_at(self.raw, self.cursor, "NOT") {
            return Err(diagnostic("unsupported_syntax", start, end));
        }
        for keyword in ["AND", "OR"] {
            if let Some((start, end)) = keyword_at(self.raw, self.cursor, keyword) {
                return Err(diagnostic("missing_operand", start, end));
            }
        }
        if self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic(
                "unbalanced_parenthesis",
                self.cursor,
                self.cursor + 1,
            ));
        }
        if self.raw.as_bytes().get(self.cursor) != Some(&b'(') {
            return self.parse_predicate();
        }

        let open = self.cursor;
        self.consume_token(open, open + 1)?;
        if depth >= SEARCH_MAX_GROUP_DEPTH {
            return Err(diagnostic("group_too_deep", open, open + 1));
        }
        self.cursor = skip_space(self.raw, open + 1);
        if self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic("empty_group", open, self.cursor + 1));
        }
        if self.cursor == self.raw.len() {
            return Err(diagnostic("unbalanced_parenthesis", open, open + 1));
        }
        let expr = self.parse_expression(depth + 1)?;
        self.cursor = skip_space(self.raw, self.cursor);
        if self.raw.as_bytes().get(self.cursor) != Some(&b')') {
            return Err(diagnostic("unbalanced_parenthesis", open, open + 1));
        }
        let close = self.cursor;
        self.consume_token(close, close + 1)?;
        self.cursor += 1;
        Ok(expr)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "predicate parsing keeps field, operator, value, and diagnostic spans aligned"
    )]
    fn parse_predicate(&mut self) -> Result<Expr, SearchDiagnostic> {
        if self.cursor == self.raw.len() {
            return Err(diagnostic("missing_operand", self.cursor, self.cursor));
        }
        if self.clauses.len() >= SEARCH_MAX_CLAUSES {
            return Err(diagnostic("too_many_clauses", self.cursor, self.raw.len()));
        }
        let start = self.cursor;
        let key_start = self.cursor;
        while self
            .raw
            .as_bytes()
            .get(self.cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            self.cursor += 1;
        }
        if self.cursor == key_start {
            return Err(diagnostic(
                "empty_clause",
                self.cursor,
                next_byte(self.raw, self.cursor),
            ));
        }
        let raw_key = self
            .raw
            .get(key_start..self.cursor)
            .expect("the ASCII field scan preserves UTF-8 boundaries");
        let Some(field) = self.fields.iter().find(|field| {
            field.key.eq_ignore_ascii_case(raw_key)
                || field
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(raw_key))
        }) else {
            return Err(diagnostic("unknown_field", key_start, self.cursor));
        };

        self.cursor = skip_space(self.raw, self.cursor);
        let operator_start = self.cursor;
        while self
            .raw
            .as_bytes()
            .get(self.cursor)
            .is_some_and(|byte| matches!(byte, b'!' | b'<' | b'>' | b'=' | b':'))
        {
            self.cursor += 1;
        }
        let token = self
            .raw
            .get(operator_start..self.cursor)
            .expect("the ASCII operator scan preserves UTF-8 boundaries");
        let operator = match token {
            ":" => SearchOperator::Colon,
            ">" => SearchOperator::Greater,
            "<" => SearchOperator::Less,
            ">=" | "<=" | "==" | "!=" | "=" => {
                return Err(diagnostic(
                    "unsupported_operator",
                    operator_start,
                    self.cursor,
                ));
            }
            "" => {
                return Err(diagnostic(
                    "expected_colon",
                    operator_start,
                    next_byte(self.raw, operator_start),
                ));
            }
            _ => {
                return Err(diagnostic(
                    "malformed_operator",
                    operator_start,
                    self.cursor,
                ));
            }
        };
        let comparison = !matches!(operator, SearchOperator::Colon);
        if comparison != matches!(field.kind, SearchFieldKind::Quantity(_)) {
            return Err(diagnostic(
                "operator_not_allowed",
                operator_start,
                self.cursor,
            ));
        }
        self.cursor = skip_space(self.raw, self.cursor);
        if self.cursor == self.raw.len() || self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic("missing_value", self.cursor, self.cursor));
        }
        if comparison && self.raw.as_bytes().get(self.cursor) == Some(&b'"') {
            let (_, end) = parse_quoted(self.raw, self.cursor)?;
            return Err(diagnostic("quoted_quantity", self.cursor, end));
        }
        let value_start = self.cursor;
        let (value, next, quoted) = parse_value(self.raw, self.cursor)?;
        if value.is_empty() {
            return Err(diagnostic("missing_value", self.cursor, next));
        }
        if value.chars().count() > SEARCH_MAX_VALUE_CHARS {
            return Err(diagnostic("value_too_long", self.cursor, next));
        }
        let parsed_value = match field.kind {
            SearchFieldKind::String => SearchValue::Pattern(GlobPattern::new(&value)),
            SearchFieldKind::Identifier { signed } => {
                if !valid_identifier(&value, signed) {
                    return Err(diagnostic("invalid_identifier", self.cursor, next));
                }
                SearchValue::Identifier(value.clone())
            }
            SearchFieldKind::Quantity(result) => {
                if quoted {
                    return Err(diagnostic("quoted_quantity", self.cursor, next));
                }
                let after = skip_space(self.raw, next);
                if after > next {
                    let unit_end = next_token(self.raw, after);
                    let unit = self
                        .raw
                        .get(after..unit_end)
                        .expect("the unit scan preserves UTF-8 boundaries");
                    if looks_like_unit(unit) {
                        return Err(diagnostic("whitespace_before_unit", after, unit_end));
                    }
                }
                SearchValue::Quantity(parse_quantity(&value, result.kind, value_start)?)
            }
        };
        self.cursor = next;
        let operator_text = match operator {
            SearchOperator::Colon => ":",
            SearchOperator::Greater => ">",
            SearchOperator::Less => "<",
        };
        let canonical_value = match &parsed_value {
            SearchValue::Identifier(value) => value.clone(),
            SearchValue::Pattern(_) => canonical_value(&value),
            SearchValue::Quantity(quantity) => quantity.canonical.clone(),
            SearchValue::AnyOf(_) => unreachable!("the text parser does not construct typed sets"),
        };
        let clause = SearchClause {
            canonical: format!("{}{operator_text}{canonical_value}", field.key),
            key: field.key,
            columns: field.columns,
            operator,
            value: parsed_value,
        };
        self.consume_token(start, next)?;
        self.clauses.push(clause.clone());
        Ok(Expr::Predicate(clause))
    }

    fn require_operand(
        &self,
        operator_start: usize,
        operator_end: usize,
    ) -> Result<(), SearchDiagnostic> {
        if self.cursor == self.raw.len() || self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic("missing_operand", operator_start, operator_end));
        }
        for keyword in ["AND", "OR"] {
            if let Some((start, end)) = keyword_at(self.raw, self.cursor, keyword) {
                return Err(diagnostic("missing_operand", start, end));
            }
        }
        Ok(())
    }

    const fn consume_token(&mut self, start: usize, end: usize) -> Result<(), SearchDiagnostic> {
        if self.tokens >= SEARCH_MAX_TOKENS {
            return Err(diagnostic("too_many_tokens", start, end));
        }
        self.tokens += 1;
        Ok(())
    }
}

#[expect(
    clippy::string_slice,
    reason = "the caller provides a grammar boundary and next_space advances over whole ASCII bytes"
)]
fn parse_value(input: &str, start: usize) -> Result<(String, usize, bool), SearchDiagnostic> {
    if input.as_bytes().get(start) == Some(&b'"') {
        let (value, end) = parse_quoted(input, start)?;
        Ok((value, end, true))
    } else {
        let end = next_token(input, start);
        Ok((input[start..end].to_owned(), end, false))
    }
}

#[expect(
    clippy::string_slice,
    reason = "cursor starts at an ASCII quote and then advances by each character's UTF-8 width"
)]
fn parse_quoted(input: &str, start: usize) -> Result<(String, usize), SearchDiagnostic> {
    let mut value = String::new();
    let mut cursor = start + 1;
    while cursor < input.len() {
        let character = input[cursor..]
            .chars()
            .next()
            .ok_or_else(|| diagnostic("unterminated_quote", start, input.len()))?;
        if character == '"' {
            return Ok((value, cursor + 1));
        }
        if character == '\\' {
            let escaped = input[cursor + 1..]
                .chars()
                .next()
                .filter(|escaped| matches!(escaped, '"' | '\\'))
                .ok_or_else(|| {
                    diagnostic("invalid_escape", cursor, next_byte(input, cursor + 1))
                })?;
            value.push(escaped);
            cursor += 1 + escaped.len_utf8();
        } else {
            value.push(character);
            cursor += character.len_utf8();
        }
    }
    Err(diagnostic("unterminated_quote", start, input.len()))
}

fn canonical_value(value: &str) -> String {
    if value.bytes().all(|byte| {
        !byte.is_ascii_whitespace() && !matches!(byte, b':' | b'"' | b'\\' | b'(' | b')')
    }) {
        return value.to_owned();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Validate the canonical decimal spelling of a signed or unsigned identifier.
#[must_use]
pub fn valid_identifier(value: &str, signed: bool) -> bool {
    if signed {
        if value == "-0" {
            return false;
        }
        let decimal = value.strip_prefix('-').unwrap_or(value);
        !decimal.is_empty()
            && (decimal.len() == 1 || !decimal.starts_with('0'))
            && decimal.bytes().all(|byte| byte.is_ascii_digit())
            && value.parse::<i64>().is_ok()
    } else {
        !value.is_empty()
            && (value.len() == 1 || !value.starts_with('0'))
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && value.parse::<u64>().is_ok()
    }
}

fn has_structured_syntax(input: &str) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for (start, character) in input.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if !quoted {
            if matches!(character, ':' | '<' | '>' | '!' | '=' | '(' | ')') {
                return true;
            }
            if ["AND", "OR", "NOT"]
                .iter()
                .any(|keyword| keyword_at(input, start, keyword).is_some())
            {
                return true;
            }
        }
    }
    false
}

fn first_unsupported(input: &str) -> Option<(usize, usize)> {
    let mut quoted = false;
    let mut escaped = false;
    for (start, character) in input.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if !quoted && let Some(span) = keyword_at(input, start, "NOT") {
            return Some(span);
        }
    }
    None
}

fn keyword_at(input: &str, start: usize, keyword: &str) -> Option<(usize, usize)> {
    let end = start.checked_add(keyword.len())?;
    if !input
        .get(start..end)
        .is_some_and(|token| token.eq_ignore_ascii_case(keyword))
        || (start > 0
            && input
                .as_bytes()
                .get(start - 1)
                .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'(' | b')')))
        || input
            .as_bytes()
            .get(end)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'(' | b')'))
    {
        return None;
    }
    Some((start, end))
}

fn looks_like_unit(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if token.eq_ignore_ascii_case("AND") || token.eq_ignore_ascii_case("OR") {
        return false;
    }
    byte_multiplier(token).is_some()
        || duration_factors(token).is_some()
        || matches!(token, "/s" | "%")
        || token
            .strip_suffix("/s")
            .is_some_and(|unit| byte_multiplier(unit).is_some() || duration_factors(unit).is_some())
        || token
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'/' | b'%'))
}

fn skip_space(input: &str, mut cursor: usize) -> usize {
    while input
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn next_token(input: &str, mut cursor: usize) -> usize {
    while input
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'(' | b')'))
    {
        cursor += 1;
    }
    cursor
}

fn next_byte(input: &str, cursor: usize) -> usize {
    (cursor + 1).min(input.len())
}

const fn diagnostic(code: &'static str, start: usize, end: usize) -> SearchDiagnostic {
    SearchDiagnostic { code, start, end }
}
