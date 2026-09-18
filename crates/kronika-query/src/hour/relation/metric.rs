//! Exact relation metric comparison and JSON rendering.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use kronika_reader::{Cell, Row};
use serde_json::{Value, json};

use super::{
    Metric, MetricOrderValue, MetricValue, OrderedNumber, RateAggregate, RateValue, counter_input,
};

use crate::exact_product::compare_products;
use crate::projection::Plan;

impl std::fmt::Debug for Metric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Metric").field(&self.json()).finish()
    }
}

impl Metric {
    pub(super) const fn number(value: OrderedNumber) -> Self {
        Self {
            value: MetricValue::Number(value),
        }
    }

    pub(super) const fn rate(value: RateValue) -> Self {
        Self {
            value: MetricValue::Rate(value),
        }
    }

    pub(super) const fn rate_ratio(
        numerator: RateValue,
        denominator: RateValue,
        scale: f64,
    ) -> Self {
        Self {
            value: MetricValue::RateRatio {
                numerator,
                denominator,
                scale,
            },
        }
    }

    pub(super) const fn ratio(
        numerator: OrderedNumber,
        denominator: OrderedNumber,
        scale: f64,
    ) -> Self {
        Self {
            value: MetricValue::Ratio {
                numerator,
                denominator,
                scale,
            },
        }
    }

    pub(super) const fn integer(value: i128) -> Self {
        Self {
            value: MetricValue::Integer(value),
        }
    }

    pub(super) const fn timestamp(value: i64) -> Self {
        Self {
            value: MetricValue::Timestamp(value),
        }
    }

    pub(super) const fn boolean(value: bool) -> Self {
        Self {
            value: MetricValue::Boolean(value),
        }
    }

    pub(super) const fn text(value: String) -> Self {
        Self {
            value: MetricValue::Text(value),
        }
    }

    /// Render this metric using the existing JSON scalar contract.
    #[must_use]
    pub fn json(&self) -> Value {
        match &self.value {
            MetricValue::Number(OrderedNumber::Integer(value)) | MetricValue::Integer(value) => {
                Value::String(value.to_string())
            }
            MetricValue::Number(OrderedNumber::Float(value)) => finite_json(*value),
            MetricValue::Rate(value) => finite_json(value.per_second()),
            MetricValue::RateRatio {
                numerator,
                denominator,
                scale,
            } => finite_json(numerator.per_microsecond() / denominator.per_microsecond() * scale),
            MetricValue::Ratio {
                numerator,
                denominator,
                scale,
            } => finite_json(numerator.as_f64() / denominator.as_f64() * scale),
            MetricValue::Timestamp(value) => Value::String(value.to_string()),
            MetricValue::Boolean(value) => Value::Bool(*value),
            MetricValue::Text(value) => Value::String(value.clone()),
        }
    }

    /// Compare two compatible metric values without applying transport sort direction.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        compare_metric_order(self.order_value()?, other.order_value()?)
    }

    /// Compare this metric with an exact non-negative rational threshold.
    #[must_use]
    pub fn compare_ratio(&self, numerator: u128, denominator: u128) -> Option<Ordering> {
        if denominator == 0 {
            return None;
        }
        match &self.value {
            MetricValue::Number(OrderedNumber::Integer(value)) | MetricValue::Integer(value) => {
                Some(compare_u128_ratios(
                    u128::try_from(*value).ok()?,
                    1,
                    numerator,
                    denominator,
                ))
            }
            MetricValue::Number(OrderedNumber::Float(value)) => {
                compare_float_ratio(*value, numerator, denominator)
            }
            MetricValue::Rate(RateValue::Exact {
                numerator: value,
                denominator: value_denominator,
            }) => Some(compare_products(
                &[*value, 1_000_000, denominator],
                &[*value_denominator, numerator],
            )),
            MetricValue::Rate(RateValue::Float(value)) => {
                compare_float_ratio(*value * 1_000_000.0, numerator, denominator)
            }
            MetricValue::RateRatio {
                numerator: value_numerator,
                denominator: value_denominator,
                scale,
            } => match (*value_numerator, *value_denominator) {
                (
                    RateValue::Exact {
                        numerator: value,
                        denominator: numerator_denominator,
                    },
                    RateValue::Exact {
                        numerator: denominator_numerator,
                        denominator: denominator_denominator,
                    },
                ) if denominator_numerator > 0 => Some(compare_products(
                    &[
                        value,
                        denominator_denominator,
                        exact_scale(*scale)?,
                        denominator,
                    ],
                    &[numerator_denominator, denominator_numerator, numerator],
                )),
                _ => compare_float_ratio(
                    value_numerator.per_microsecond() / value_denominator.per_microsecond() * scale,
                    numerator,
                    denominator,
                ),
            },
            MetricValue::Ratio {
                numerator: value_numerator,
                denominator: value_denominator,
                scale,
            } => match (*value_numerator, *value_denominator) {
                (OrderedNumber::Integer(value), OrderedNumber::Integer(value_denominator))
                    if value >= 0 && value_denominator > 0 =>
                {
                    Some(compare_products(
                        &[
                            u128::try_from(value).ok()?,
                            exact_scale(*scale)?,
                            denominator,
                        ],
                        &[u128::try_from(value_denominator).ok()?, numerator],
                    ))
                }
                _ => compare_float_ratio(
                    value_numerator.as_f64() / value_denominator.as_f64() * scale,
                    numerator,
                    denominator,
                ),
            },
            MetricValue::Timestamp(_) | MetricValue::Boolean(_) | MetricValue::Text(_) => None,
        }
    }

    pub(super) fn order_value(&self) -> Option<MetricOrderValue<'_>> {
        match &self.value {
            MetricValue::Number(OrderedNumber::Integer(value)) | MetricValue::Integer(value) => {
                Some(MetricOrderValue::Integer(*value))
            }
            MetricValue::Number(OrderedNumber::Float(value)) => {
                value.is_finite().then_some(MetricOrderValue::Float(*value))
            }
            MetricValue::Rate(value) => rate_order_value(*value),
            MetricValue::RateRatio {
                numerator,
                denominator,
                ..
            } => rate_ratio_order_value(*numerator, *denominator),
            MetricValue::Ratio {
                numerator: OrderedNumber::Integer(numerator),
                denominator: OrderedNumber::Integer(denominator),
                ..
            } if *numerator >= 0 && *denominator > 0 => Some(MetricOrderValue::ExactRatio {
                numerator: u128::try_from(*numerator).ok()?,
                denominator: u128::try_from(*denominator).ok()?,
            }),
            MetricValue::Ratio {
                numerator,
                denominator,
                ..
            } => {
                let denominator = denominator.as_f64();
                let ratio = numerator.as_f64() / denominator;
                (denominator > 0.0 && ratio.is_finite())
                    .then_some(MetricOrderValue::FloatRatio(ratio))
            }
            MetricValue::Timestamp(value) => Some(MetricOrderValue::Integer(i128::from(*value))),
            MetricValue::Boolean(value) => Some(MetricOrderValue::Integer(i128::from(*value))),
            MetricValue::Text(value) => Some(MetricOrderValue::Text(value.as_bytes())),
        }
    }
}

fn rate_order_value(value: RateValue) -> Option<MetricOrderValue<'static>> {
    match value {
        RateValue::Exact {
            numerator,
            denominator,
        } => Some(MetricOrderValue::ExactRatio {
            numerator,
            denominator,
        }),
        RateValue::Float(value) => value
            .is_finite()
            .then_some(MetricOrderValue::FloatRatio(value)),
    }
}

fn rate_ratio_order_value(
    numerator: RateValue,
    denominator: RateValue,
) -> Option<MetricOrderValue<'static>> {
    if let (
        RateValue::Exact {
            numerator,
            denominator: numerator_denominator,
        },
        RateValue::Exact {
            numerator: denominator_numerator,
            denominator: denominator_denominator,
        },
    ) = (numerator, denominator)
        && denominator_numerator > 0
        && let Some((numerator, denominator)) = numerator
            .checked_mul(denominator_denominator)
            .zip(numerator_denominator.checked_mul(denominator_numerator))
    {
        let RateValue::Exact {
            numerator,
            denominator,
        } = RateValue::exact(numerator, denominator)
        else {
            unreachable!("an exact rate remains exact")
        };
        return Some(MetricOrderValue::ExactRatio {
            numerator,
            denominator,
        });
    }
    let denominator_value = denominator.per_microsecond();
    let value = numerator.per_microsecond() / denominator_value;
    (denominator_value > 0.0 && value.is_finite()).then_some(MetricOrderValue::FloatRatio(value))
}

fn compare_metric_order(
    left: MetricOrderValue<'_>,
    right: MetricOrderValue<'_>,
) -> Option<Ordering> {
    match (left, right) {
        (MetricOrderValue::Integer(left), MetricOrderValue::Integer(right)) => {
            Some(left.cmp(&right))
        }
        (MetricOrderValue::Float(left), MetricOrderValue::Float(right))
        | (MetricOrderValue::FloatRatio(left), MetricOrderValue::FloatRatio(right)) => {
            left.partial_cmp(&right)
        }
        (
            MetricOrderValue::ExactRatio {
                numerator: left_numerator,
                denominator: left_denominator,
            },
            MetricOrderValue::ExactRatio {
                numerator: right_numerator,
                denominator: right_denominator,
            },
        ) => Some(compare_u128_ratios(
            left_numerator,
            left_denominator,
            right_numerator,
            right_denominator,
        )),
        (
            MetricOrderValue::ExactRatio {
                numerator,
                denominator,
            },
            MetricOrderValue::FloatRatio(right),
        ) => integer_ratio_as_f64(numerator, denominator).partial_cmp(&right),
        (
            MetricOrderValue::FloatRatio(left),
            MetricOrderValue::ExactRatio {
                numerator,
                denominator,
            },
        ) => left.partial_cmp(&integer_ratio_as_f64(numerator, denominator)),
        (MetricOrderValue::Text(left), MetricOrderValue::Text(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "this path compares an exact ratio with an already inexact floating source"
)]
fn integer_ratio_as_f64(numerator: u128, denominator: u128) -> f64 {
    numerator as f64 / denominator as f64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "floating comparison is used only when the authoritative reducer is already inexact"
)]
fn compare_float_ratio(value: f64, numerator: u128, denominator: u128) -> Option<Ordering> {
    let threshold = numerator as f64 / denominator as f64;
    (value.is_finite() && threshold.is_finite()).then(|| value.total_cmp(&threshold))
}

const fn exact_scale(scale: f64) -> Option<u128> {
    match scale.to_bits() {
        bits if bits == 1.0_f64.to_bits() => Some(1),
        bits if bits == 100.0_f64.to_bits() => Some(100),
        _ => None,
    }
}

fn compare_u128_ratios(
    mut left_numerator: u128,
    mut left_denominator: u128,
    mut right_numerator: u128,
    mut right_denominator: u128,
) -> Ordering {
    let mut reverse = false;
    loop {
        let whole = (left_numerator / left_denominator).cmp(&(right_numerator / right_denominator));
        if !matches!(whole, Ordering::Equal) {
            return if reverse { whole.reverse() } else { whole };
        }
        let left_remainder = left_numerator % left_denominator;
        let right_remainder = right_numerator % right_denominator;
        match (left_remainder == 0, right_remainder == 0) {
            (true, true) => return Ordering::Equal,
            (true, false) => {
                return if reverse {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (false, true) => {
                return if reverse {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, false) => {}
        }
        left_numerator = left_denominator;
        left_denominator = left_remainder;
        right_numerator = right_denominator;
        right_denominator = right_remainder;
        reverse = !reverse;
    }
}

/// Whether one physical index row has an exact zero scan rate.
#[must_use]
pub fn index_scan_rate_is_zero(
    plan: &Plan,
    row: &Row,
    before: Option<&BTreeMap<&'static str, Cell>>,
    elapsed: Option<i64>,
) -> bool {
    let mut scans = RateAggregate::default();
    scans.add(counter_input(plan, row, before, "idx_scan", false), elapsed);
    scans.metric().is_some_and(|metric| match metric.value {
        MetricValue::Rate(RateValue::Exact { numerator, .. }) => numerator == 0,
        MetricValue::Rate(RateValue::Float(value)) => value == 0.0,
        _ => false,
    })
}

fn finite_json(value: f64) -> Value {
    if value.is_finite() {
        json!(value)
    } else {
        Value::Null
    }
}
