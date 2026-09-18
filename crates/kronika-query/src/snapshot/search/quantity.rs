//! Exact numeric search quantities, units, and bounds.

use super::{
    Quantity, QuantityKind, SEARCH_MAX_FRACTIONAL_DIGITS, SEARCH_MAX_SIGNIFICANT_DIGITS,
    SearchDiagnostic, diagnostic,
};

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive quantity match keeps every public unit and bound in one audited conversion"
)]
#[expect(
    clippy::string_slice,
    reason = "the split position is found by scanning ASCII numeric bytes"
)]
pub(super) fn parse_quantity(
    raw: &str,
    kind: QuantityKind,
    offset: usize,
) -> Result<Quantity, SearchDiagnostic> {
    if raw.starts_with('-') {
        return Err(diagnostic(
            "negative_not_allowed",
            offset,
            offset + raw.len(),
        ));
    }
    if raw.starts_with('+') {
        return Err(diagnostic("invalid_number", offset, offset + raw.len()));
    }
    if raw.contains(',') || raw.contains('_') {
        return Err(diagnostic("invalid_number", offset, offset + raw.len()));
    }
    let number_end = raw
        .bytes()
        .position(|byte| !(byte.is_ascii_digit() || byte == b'.'))
        .unwrap_or(raw.len());
    let number = &raw[..number_end];
    let unit = &raw[number_end..];
    if number.is_empty()
        || number.matches('.').count() > 1
        || number.ends_with('.')
        || number.contains(',')
        || number.contains('_')
        || unit.strip_prefix(['e', 'E']).is_some_and(|rest| {
            rest.starts_with(|character: char| {
                character.is_ascii_digit() || matches!(character, '+' | '-')
            })
        })
    {
        return Err(diagnostic("invalid_number", offset, offset + raw.len()));
    }
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(diagnostic("invalid_number", offset, offset + number_end));
    }
    let significant = format!("{whole}{fraction}");
    let significant_digits = significant.trim_start_matches('0').len().max(1);
    if significant_digits > SEARCH_MAX_SIGNIFICANT_DIGITS
        || fraction.len() > SEARCH_MAX_FRACTIONAL_DIGITS
    {
        return Err(diagnostic("out_of_range", offset, offset + number_end));
    }
    let coefficient = significant
        .parse::<u128>()
        .map_err(|_error| diagnostic("out_of_range", offset, offset + number_end))?;
    let scale = checked_power(10, fraction.len())
        .ok_or_else(|| diagnostic("out_of_range", offset, offset + number_end))?;
    let trimmed = fraction.trim_end_matches('0');
    let canonical_number = if trimmed.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{trimmed}")
    };
    let (mut numerator, mut denominator) = match kind {
        QuantityKind::Bytes | QuantityKind::ByteRate => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            let suffix = if kind == QuantityKind::ByteRate {
                "/s"
            } else {
                ""
            };
            let byte_unit = unit
                .strip_suffix(suffix)
                .filter(|unit| !unit.is_empty())
                .ok_or_else(|| {
                    diagnostic("invalid_unit", offset + number_end, offset + raw.len())
                })?;
            let multiplier = byte_multiplier(byte_unit).ok_or_else(|| {
                diagnostic("invalid_unit", offset + number_end, offset + raw.len())
            })?;
            let scaled = coefficient
                .checked_mul(multiplier)
                .ok_or_else(|| diagnostic("out_of_range", offset, offset + raw.len()))?;
            if kind == QuantityKind::Bytes && scaled % scale != 0 {
                return Err(diagnostic(
                    "non_integral_base_value",
                    offset,
                    offset + raw.len(),
                ));
            }
            if kind == QuantityKind::Bytes {
                (scaled / scale, 1)
            } else {
                (scaled, scale)
            }
        }
        QuantityKind::Duration | QuantityKind::DurationRate => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            let duration_unit = if kind == QuantityKind::DurationRate {
                unit.strip_suffix("/s").filter(|unit| !unit.is_empty())
            } else {
                Some(unit)
            };
            let (numerator, denominator) =
                duration_unit.and_then(duration_factors).ok_or_else(|| {
                    diagnostic("invalid_unit", offset + number_end, offset + raw.len())
                })?;
            if kind == QuantityKind::DurationRate
                && !matches!(duration_unit, Some("ns" | "us" | "ms" | "s"))
            {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            (
                coefficient
                    .checked_mul(numerator)
                    .ok_or_else(|| diagnostic("out_of_range", offset, offset + raw.len()))?,
                scale
                    .checked_mul(denominator)
                    .ok_or_else(|| diagnostic("out_of_range", offset, offset + raw.len()))?,
            )
        }
        QuantityKind::Count => {
            if !unit.is_empty() {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if !fraction.is_empty() {
                return Err(diagnostic(
                    "non_integral_base_value",
                    offset,
                    offset + raw.len(),
                ));
            }
            (coefficient, 1)
        }
        QuantityKind::CountRate => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if unit != "/s" {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            (coefficient, scale)
        }
        QuantityKind::Percentage => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if unit != "%" {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if coefficient > 100_u128.saturating_mul(scale) {
                return Err(diagnostic("out_of_range", offset, offset + raw.len()));
            }
            (coefficient, scale)
        }
        QuantityKind::Scalar => {
            if !unit.is_empty() {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            (coefficient, scale)
        }
    };
    let divisor = greatest_common_divisor(numerator, denominator);
    numerator /= divisor;
    denominator /= divisor;
    Ok(Quantity {
        numerator,
        denominator,
        canonical: format!("{canonical_number}{unit}"),
    })
}

pub(super) const fn byte_multiplier(unit: &str) -> Option<u128> {
    match unit.as_bytes() {
        b"B" => Some(1),
        b"kB" => Some(1_000),
        b"MB" => Some(1_000_000),
        b"GB" => Some(1_000_000_000),
        b"TB" => Some(1_000_000_000_000),
        b"PB" => Some(1_000_000_000_000_000),
        b"EB" => Some(1_000_000_000_000_000_000),
        b"KiB" => Some(1_024),
        b"MiB" => Some(1_048_576),
        b"GiB" => Some(1_073_741_824),
        b"TiB" => Some(1_099_511_627_776),
        b"PiB" => Some(1_125_899_906_842_624),
        b"EiB" => Some(1_152_921_504_606_846_976),
        _ => None,
    }
}

pub(super) const fn duration_factors(unit: &str) -> Option<(u128, u128)> {
    match unit.as_bytes() {
        b"ns" => Some((1, 1_000_000)),
        b"us" => Some((1, 1_000)),
        b"ms" => Some((1, 1)),
        b"s" => Some((1_000, 1)),
        b"min" => Some((60_000, 1)),
        b"h" => Some((3_600_000, 1)),
        _ => None,
    }
}

const fn checked_power(base: u128, exponent: usize) -> Option<u128> {
    let mut result = 1_u128;
    let mut index = 0;
    while index < exponent {
        result = match result.checked_mul(base) {
            Some(value) => value,
            None => return None,
        };
        index += 1;
    }
    Some(result)
}

const fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}
