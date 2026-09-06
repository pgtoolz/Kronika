//! Assertions against the lines an operator reads.

use super::table_rows;
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::then;

const SIX_DECIMAL_PLACES: u128 = 1_000_000;
const NANOS_PER_MILLISECOND: u128 = 1_000_000;
const NANOS_PER_SECOND: u128 = 1_000_000_000;
const MIB: u128 = 1_048_576;

/// Everything the run wrote to stdout and stderr.
fn log(world: &BddWorld) -> Result<String> {
    world.run.as_ref().context("a collector was started")?.log()
}

#[then(regex = r"^the log has a (\S+) line naming these fields$")]
fn line_with_fields(world: &mut BddWorld, action: String, step: &Step) -> Result<()> {
    let text = log(world)?;
    let line = text
        .lines()
        .find(|line| line.contains(&format!("action={action}")))
        .with_context(|| format!("no {action} line in:\n{text}"))?;
    for row in table_rows(step, &["field"])? {
        let [field] = row.as_slice() else {
            anyhow::bail!("a field row needs one cell, got {row:?}");
        };
        anyhow::ensure!(
            line.contains(&format!("{field}=")),
            "{field} is missing from {line}"
        );
    }
    Ok(())
}

#[then("the log reports these collections as degraded")]
fn degraded_collections(world: &mut BddWorld, step: &Step) -> Result<()> {
    let text = log(world)?;
    for row in table_rows(step, &["collection", "reason"])? {
        let [collection, reason] = row.as_slice() else {
            anyhow::bail!("a degradation row needs a collection and a reason, got {row:?}");
        };
        anyhow::ensure!(
            text.lines().any(|line| {
                line.contains("action=collection_degraded")
                    && line.contains(&format!("collection={collection} "))
                    && (line.contains(&format!("reason={reason}"))
                        || line.contains(&format!("reason=\"{reason}\"")))
            }),
            "no line reports {collection} degraded with reason={reason} in:\n{text}"
        );
    }
    Ok(())
}

#[then("the log reports that active.wal was preserved")]
fn journal_was_preserved(world: &mut BddWorld) -> Result<()> {
    let text = log(world)?;
    anyhow::ensure!(
        text.contains("open active.wal") && text.contains("existing file is preserved"),
        "no preserved active.wal startup error in:\n{text}"
    );
    Ok(())
}

#[then("the log has no error line")]
fn no_error_line(world: &mut BddWorld) -> Result<()> {
    let text = log(world)?;
    let errors: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("level=error"))
        .collect();
    anyhow::ensure!(errors.is_empty(), "unexpected error lines: {errors:?}");
    Ok(())
}

#[then(
    regex = r"^the shutdown pg_query_summary for (\S+) payload reports consistent traffic and costs$"
)]
fn shutdown_postgresql_query_summary_reports_traffic(
    world: &mut BddWorld,
    payload_measure: String,
    step: &Step,
) -> Result<()> {
    let text = log(world)?;
    let line = text
        .lines()
        .find(|line| {
            line.split_whitespace()
                .any(|field| field == "action=pg_query_summary")
                && line
                    .split_whitespace()
                    .any(|field| field == "reason=shutdown")
        })
        .with_context(|| format!("no shutdown pg_query_summary line in:\n{text}"))?;
    let rows = table_rows(step, &["field", "requirement"])?;
    let mut requirements = Vec::with_capacity(rows.len());
    for row in &rows {
        let [name, requirement] = row.as_slice() else {
            anyhow::bail!("an integer field row needs a field and a requirement, got {row:?}");
        };
        requirements.push((name.as_str(), requirement.as_str()));
    }
    validate_integer_fields(line, &requirements)?;
    validate_shutdown_query_summary(line, &payload_measure)
}

fn validate_integer_fields(line: &str, requirements: &[(&str, &str)]) -> Result<()> {
    for &(name, requirement) in requirements {
        let value = integer_field(line, name)?;
        match requirement {
            "positive" => anyhow::ensure!(value > 0, "{name} must be positive in {line}"),
            "nonnegative" => {}
            _ => anyhow::bail!("unknown requirement {requirement} for {name} in {line}"),
        }
    }
    Ok(())
}

fn validate_shutdown_query_summary(line: &str, payload_measure: &str) -> Result<()> {
    require_literal_field(line, "action", "pg_query_summary")?;
    require_literal_field(line, "reason", "shutdown")?;
    require_literal_field(line, "payload_measure", payload_measure)?;

    let interval_ms = integer_field(line, "interval_ms")?;
    let query_count = u64_field(line, "query_count")?;
    let (query_rate, query_rate_scaled) = fixed_six_field(line, "query_rate_per_s")?;
    anyhow::ensure!(
        query_rate_scaled > 0,
        "query_rate_per_s must be positive in {line}"
    );
    validate_query_rate(
        line,
        query_count,
        interval_ms,
        query_rate,
        query_rate_scaled,
    )?;

    let from_postgres = u64_field(line, "application_payload_from_postgres_bytes")?;
    validate_mib_field(line, "application_payload_from_postgres_mib", from_postgres)?;
    let to_postgres = u64_field(line, "application_payload_to_postgres_bytes")?;
    validate_mib_field(line, "application_payload_to_postgres_mib", to_postgres)?;

    let query_errors = u64_field(line, "query_errors")?;
    let sink_errors = u64_field(line, "sink_errors")?;
    let connect_errors = u64_field(line, "connect_errors")?;
    let query_timeouts = u64_field(line, "query_timeouts")?;
    let connect_timeouts = u64_field(line, "connect_timeouts")?;
    let errors = u64_field(line, "errors")?;
    let timeouts = u64_field(line, "timeouts")?;

    let expected_errors = query_errors
        .saturating_add(sink_errors)
        .saturating_add(connect_errors);
    anyhow::ensure!(
        errors == expected_errors,
        "errors={errors} does not equal query_errors + sink_errors + connect_errors = \
         {expected_errors} in {line}"
    );
    let expected_timeouts = query_timeouts.saturating_add(connect_timeouts);
    anyhow::ensure!(
        timeouts == expected_timeouts,
        "timeouts={timeouts} does not equal query_timeouts + connect_timeouts = \
         {expected_timeouts} in {line}"
    );

    let fetch_total = integer_field(line, "fetch_elapsed_ms_total")?;
    let fetch_max = integer_field(line, "fetch_elapsed_ms_max")?;
    anyhow::ensure!(
        fetch_max <= fetch_total,
        "fetch_elapsed_ms_max={fetch_max} exceeds fetch_elapsed_ms_total={fetch_total} in {line}"
    );
    Ok(())
}

fn field_value<'a>(line: &'a str, name: &str) -> Result<&'a str> {
    let prefix = format!("{name}=");
    line.split_whitespace()
        .find_map(|field| field.strip_prefix(&prefix))
        .with_context(|| format!("{name} is missing from {line}"))
}

fn require_literal_field(line: &str, name: &str, expected: &str) -> Result<()> {
    let actual = field_value(line, name)?;
    anyhow::ensure!(
        actual == expected,
        "{name}={actual} is not {name}={expected} in {line}"
    );
    Ok(())
}

fn integer_field(line: &str, name: &str) -> Result<u128> {
    let value = field_value(line, name)?;
    value
        .parse()
        .with_context(|| format!("{name}={value} is not a nonnegative integer in {line}"))
}

fn u64_field(line: &str, name: &str) -> Result<u64> {
    let value = field_value(line, name)?;
    value
        .parse()
        .with_context(|| format!("{name}={value} is not a nonnegative integer in {line}"))
}

fn fixed_six_field<'a>(line: &'a str, name: &str) -> Result<(&'a str, u128)> {
    let value = field_value(line, name)?;
    let (whole, fraction) = value
        .split_once('.')
        .with_context(|| format!("{name}={value} has no decimal point in {line}"))?;
    let valid_digits = !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.len() == 6
        && fraction.bytes().all(|byte| byte.is_ascii_digit());
    anyhow::ensure!(
        valid_digits,
        "{name}={value} is not a nonnegative decimal with six fractional digits in {line}"
    );
    let whole: u128 = whole
        .parse()
        .with_context(|| format!("{name}={value} is too large in {line}"))?;
    let fraction: u128 = fraction
        .parse()
        .with_context(|| format!("{name}={value} has an invalid fraction in {line}"))?;
    let scaled = whole
        .checked_mul(SIX_DECIMAL_PLACES)
        .and_then(|whole| whole.checked_add(fraction))
        .with_context(|| format!("{name}={value} is too large in {line}"))?;
    Ok((value, scaled))
}

fn validate_query_rate(
    line: &str,
    query_count: u64,
    interval_ms: u128,
    query_rate: &str,
    query_rate_scaled: u128,
) -> Result<()> {
    let first_possible_nanos = interval_ms
        .checked_mul(NANOS_PER_MILLISECOND)
        .with_context(|| format!("interval_ms={interval_ms} is too large in {line}"))?;
    let last_possible_nanos = interval_ms
        .checked_add(1)
        .and_then(|millis| millis.checked_mul(NANOS_PER_MILLISECOND))
        .and_then(|nanos| nanos.checked_sub(1))
        .with_context(|| format!("interval_ms={interval_ms} is too large in {line}"))?;
    let scaled_queries_per_second = u128::from(query_count) * NANOS_PER_SECOND * SIX_DECIMAL_PLACES;
    let fastest = rounded_ratio(scaled_queries_per_second, first_possible_nanos);
    let slowest = rounded_ratio(scaled_queries_per_second, last_possible_nanos);
    anyhow::ensure!(
        (slowest..=fastest).contains(&query_rate_scaled),
        "query_rate_per_s={query_rate} is outside {}..={} allowed by query_count={query_count}, \
         interval_ms={interval_ms}, interval-ms truncation, and six-decimal rounding in {line}",
        format_fixed_six(slowest),
        format_fixed_six(fastest)
    );
    Ok(())
}

fn validate_mib_field(line: &str, name: &str, bytes: u64) -> Result<()> {
    let (actual, _actual_scaled) = fixed_six_field(line, name)?;
    let expected = rounded_fixed_six(u128::from(bytes), MIB);
    anyhow::ensure!(
        actual == expected,
        "{name}={actual} does not equal {bytes} bytes / 1048576 rounded to six decimals \
         ({expected}) in {line}"
    );
    Ok(())
}

const fn rounded_ratio(numerator: u128, denominator: u128) -> u128 {
    numerator.saturating_add(denominator / 2) / denominator
}

fn rounded_fixed_six(numerator: u128, denominator: u128) -> String {
    let scaled = rounded_ratio(numerator.saturating_mul(SIX_DECIMAL_PLACES), denominator);
    format_fixed_six(scaled)
}

fn format_fixed_six(scaled: u128) -> String {
    let whole = scaled / SIX_DECIMAL_PLACES;
    let fraction = scaled % SIX_DECIMAL_PLACES;
    format!("{whole}.{fraction:06}")
}

#[cfg(test)]
mod tests;
