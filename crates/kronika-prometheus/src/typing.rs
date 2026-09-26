//! Column typing and value parsing at the executor boundary.
//!
//! The executor resolves each result column to a [`ColumnKind`] from the
//! `PostgreSQL` type OID (the patched tokio-postgres exposes it on simple-query
//! `RowDescription` columns) and hands text-protocol cell values to
//! [`parse_value`]. The engine never sees OIDs, so its tests run without the
//! patch.

/// Result column classification.
///
/// The value-column type set mirrors what upstream's pgx decode loop
/// exposes: `int4`, `int8`, `float4`, `float8` and `bool` (prometheus.go
/// switch in `WritePromMetrics`). `int2`, `xid`, `cid` and `numeric` hit the
/// default DROP there, so they classify as [`ColumnKind::Text`] — dropped
/// as value columns, still fine as `tag_` labels, which upstream
/// stringifies before the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    /// `bool`: exposed as 0/1.
    Bool,
    /// Integer OIDs with an upstream value path (`int4`/`int8`).
    Int,
    /// `float4`/`float8`.
    Float,
    /// Known textual OIDs: label-only, never a value column.
    Text,
    /// Unrecognized OID: treated like [`ColumnKind::Text`].
    Unknown,
}

/// A simple-protocol cell: `None` is SQL NULL, text otherwise.
pub type Cell = Option<String>;

/// Classifies a `PostgreSQL` type OID.
#[must_use]
pub const fn kind_for_oid(oid: u32) -> ColumnKind {
    match oid {
        16 => ColumnKind::Bool,
        // int8 and int4 are the only integers with an upstream value path
        20 | 23 => ColumnKind::Int,
        700 | 701 => ColumnKind::Float,
        // dropped by the upstream default: int2 (21), xid (28), cid (29),
        // numeric (1700), plus the textual family
        17 | 18 | 19 | 21 | 25 | 28 | 29 | 142 | 1042 | 1043 | 114 | 1700 | 3802 | 1082 | 1083
        | 1114 | 1184 | 1186 | 2950 | 869 | 650 | 829 => ColumnKind::Text,
        _ => ColumnKind::Unknown,
    }
}

/// Parses one non-NULL cell into an exposed value.
///
/// `None` means the cell is not exposable as a number: wrong shape for the
/// kind, or a type upstream drops.
#[must_use]
pub fn parse_value(kind: ColumnKind, text: &str) -> Option<f64> {
    match kind {
        ColumnKind::Bool => match text {
            "t" => Some(1.0),
            "f" => Some(0.0),
            _ => None,
        },
        // int64 values beyond 2^53 lose mantissa bits, exactly like the
        // float64 conversion pgwatch applies on its Go side
        #[allow(
            clippy::cast_precision_loss,
            reason = "pgwatch converts int64 to float64 on its Go side"
        )]
        ColumnKind::Int => text.parse::<i64>().ok().map(|v| v as f64),
        // upstream v5.3.0 exposes non-finite floats (NaN, +Inf, -Inf) — no
        // sanitizeValue on its Prometheus path; the exposition renders them
        ColumnKind::Float => text.parse::<f64>().ok(),
        // Text and unknown OIDs never produce values, only tag_ labels.
        ColumnKind::Text | ColumnKind::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oid_classification() {
        assert_eq!(kind_for_oid(16), ColumnKind::Bool);
        // only int8/int4 have an upstream value path
        for oid in [20, 23] {
            assert_eq!(kind_for_oid(oid), ColumnKind::Int, "oid {oid}");
        }
        for oid in [700, 701] {
            assert_eq!(kind_for_oid(oid), ColumnKind::Float, "oid {oid}");
        }
        // int2, xid, cid and numeric drop exactly like text (upstream
        // default branch); a stock example is table_bloat fillfactor ::smallint
        for oid in [21, 28, 29, 1700, 19, 25, 1043, 1042, 114, 3802, 1184, 2950] {
            assert_eq!(kind_for_oid(oid), ColumnKind::Text, "oid {oid}");
        }
        assert_eq!(kind_for_oid(999_999), ColumnKind::Unknown);
    }

    #[test]
    fn values_by_kind() {
        assert_eq!(parse_value(ColumnKind::Bool, "t"), Some(1.0));
        assert_eq!(parse_value(ColumnKind::Bool, "f"), Some(0.0));
        assert_eq!(parse_value(ColumnKind::Bool, "x"), None);
        assert_eq!(parse_value(ColumnKind::Int, "42"), Some(42.0));
        assert_eq!(parse_value(ColumnKind::Int, "-7"), Some(-7.0));
        assert_eq!(parse_value(ColumnKind::Int, "1.5"), None);
        assert_eq!(
            parse_value(ColumnKind::Int, "9223372036854775807"),
            Some(9_223_372_036_854_776_000.0)
        );
        assert_eq!(parse_value(ColumnKind::Float, "1.5"), Some(1.5));
        // numeric columns drop like text (upstream default branch)
        assert_eq!(parse_value(ColumnKind::Text, "1.5"), None);
    }

    #[test]
    fn non_finite_floats_are_exposed() {
        // upstream v5.3.0 has no sanitizeValue on the Prometheus path
        for (text, value) in [
            ("NaN", f64::NAN),
            ("inf", f64::INFINITY),
            ("infinity", f64::INFINITY),
            ("-inf", f64::NEG_INFINITY),
        ] {
            let parsed = parse_value(ColumnKind::Float, text);
            assert_eq!(parsed.map(f64::to_bits), Some(value.to_bits()), "{text}");
        }
        assert_eq!(parse_value(ColumnKind::Int, "NaN"), None);
    }

    #[test]
    fn text_columns_never_yield_values() {
        for kind in [ColumnKind::Text, ColumnKind::Unknown] {
            assert_eq!(parse_value(kind, "1.5"), None);
            assert_eq!(parse_value(kind, "abc"), None);
        }
    }
}
