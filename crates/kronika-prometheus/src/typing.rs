//! Column typing and value parsing at the executor boundary.
//!
//! The executor resolves each result column to a [`ColumnKind`] from the
//! `PostgreSQL` type OID (the patched tokio-postgres exposes it on simple-query
//! `RowDescription` columns) and hands text-protocol cell values to
//! [`parse_value`]. The engine never sees OIDs, so its tests run without the
//! patch.

/// Result column classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    /// `bool`: exposed as 0/1.
    Bool,
    /// Integer OIDs (`int2`/`int4`/`int8`/`oid`/`xid`/`cid`).
    Int,
    /// `float4`/`float8`.
    Float,
    /// `numeric`: decimal text parsed to f64.
    Numeric,
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
        20 | 21 | 23 | 26 | 28 | 29 => ColumnKind::Int,
        700 | 701 => ColumnKind::Float,
        1700 => ColumnKind::Numeric,
        // text, name, char, bpchar, varchar, bytea, date, time, timestamp,
        // timestamptz, interval, uuid, json, jsonb, xml, inet, cidr, macaddr
        17 | 18 | 19 | 25 | 142 | 1042 | 1043 | 114 | 3802 | 1082 | 1083 | 1114 | 1184 | 1186
        | 2950 | 869 | 650 | 829 => ColumnKind::Text,
        _ => ColumnKind::Unknown,
    }
}

/// Parses one non-NULL cell into an exposed value.
///
/// `None` means the cell is not exposable as a number: wrong shape for the
/// kind, or a non-finite float (skipped like pgwatch's `sanitizeValue`, which
/// turns NaN/Infinity into NULL before the sink).
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
        ColumnKind::Float | ColumnKind::Numeric => {
            let v = text.parse::<f64>().ok()?;
            v.is_finite().then_some(v)
        }
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
        for oid in [20, 21, 23, 26, 28, 29] {
            assert_eq!(kind_for_oid(oid), ColumnKind::Int, "oid {oid}");
        }
        for oid in [700, 701] {
            assert_eq!(kind_for_oid(oid), ColumnKind::Float, "oid {oid}");
        }
        assert_eq!(kind_for_oid(1700), ColumnKind::Numeric);
        for oid in [19, 25, 1043, 1042, 114, 3802, 1184, 2950] {
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
        assert_eq!(parse_value(ColumnKind::Numeric, "1.5"), Some(1.5));
        assert_eq!(parse_value(ColumnKind::Numeric, "-0.001"), Some(-0.001));
    }

    #[test]
    fn non_finite_floats_are_skipped() {
        for kind in [ColumnKind::Float, ColumnKind::Numeric] {
            assert_eq!(parse_value(kind, "NaN"), None);
            assert_eq!(parse_value(kind, "Infinity"), None);
            assert_eq!(parse_value(kind, "-Infinity"), None);
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
