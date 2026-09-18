//! Parse Linux host or cgroup PSI into the `1_107_001` registry section.

use kronika_registry::Ts;
use kronika_registry::os_psi::OsPsi;

use super::stat::ParseError;

/// Parsed fields from one `/proc/pressure/<resource>` file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PsiRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Resource: `0`=cpu, `1`=memory, `2`=io.
    pub resource: u8,
    /// Fraction of time tasks stalled (some) over the last 10 s.
    pub some_avg10: f64,
    /// Fraction of time tasks stalled (some) over the last 60 s.
    pub some_avg60: f64,
    /// Fraction of time tasks stalled (some) over the last 300 s.
    pub some_avg300: f64,
    /// Cumulative stall time (some), microseconds.
    pub some_total: i64,
    /// Fraction of time tasks stalled (full) over the last 10 s. Undefined for system CPU; optional for cgroup CPU.
    pub full_avg10: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 60 s. Undefined for system CPU; optional for cgroup CPU.
    pub full_avg60: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 300 s. Undefined for system CPU; optional for cgroup CPU.
    pub full_avg300: Option<f64>,
    /// Cumulative stall time (full), microseconds. Undefined for system CPU; optional for cgroup CPU.
    pub full_total: Option<i64>,
}

/// Parse one PSI line of the form `some avg10=0.00 avg60=0.00 avg300=0.00 total=12345`.
///
/// Returns `(avg10, avg60, avg300, total)` on success.
fn parse_psi_line(
    source: &str,
    file: &str,
    kind: &str,
    line: &str,
) -> Result<(f64, f64, f64, i64), ParseError> {
    let mut avg10: Option<f64> = None;
    let mut avg60: Option<f64> = None;
    let mut avg300: Option<f64> = None;
    let mut total: Option<i64> = None;

    for token in line.split_whitespace().skip(1) {
        let Some((key, val)) = token.split_once('=') else {
            continue;
        };
        match key {
            "avg10" => {
                avg10 = Some(
                    val.parse::<f64>()
                        .map_err(|e| ParseError(format!("{source}/{file} {kind} avg10: {e}")))?,
                );
            }
            "avg60" => {
                avg60 = Some(
                    val.parse::<f64>()
                        .map_err(|e| ParseError(format!("{source}/{file} {kind} avg60: {e}")))?,
                );
            }
            "avg300" => {
                avg300 = Some(
                    val.parse::<f64>()
                        .map_err(|e| ParseError(format!("{source}/{file} {kind} avg300: {e}")))?,
                );
            }
            "total" => {
                total = Some(
                    val.parse::<i64>()
                        .map_err(|e| ParseError(format!("{source}/{file} {kind} total: {e}")))?,
                );
            }
            _ => {}
        }
    }

    let avg10 =
        avg10.ok_or_else(|| ParseError(format!("{source}/{file} {kind}: missing avg10")))?;
    let avg60 =
        avg60.ok_or_else(|| ParseError(format!("{source}/{file} {kind}: missing avg60")))?;
    let avg300 =
        avg300.ok_or_else(|| ParseError(format!("{source}/{file} {kind}: missing avg300")))?;
    let total =
        total.ok_or_else(|| ParseError(format!("{source}/{file} {kind}: missing total")))?;

    Ok((avg10, avg60, avg300, total))
}

/// Parse one `/proc/pressure/<resource>` file content into a [`PsiRow`].
///
/// `has_full` should be `false` for cpu (no `full` line); `true` for memory/io.
///
/// # Errors
///
/// Returns [`ParseError`] when a required field is missing or unparseable.
fn parse_resource(
    source: &str,
    file: &str,
    resource: u8,
    content: &str,
    has_full: bool,
    ts: i64,
) -> Result<PsiRow, ParseError> {
    let mut some_line: Option<&str> = None;
    let mut full_line: Option<&str> = None;

    for line in content.lines() {
        if line.starts_with("some ") {
            some_line = Some(line);
        } else if line.starts_with("full ") {
            full_line = Some(line);
        }
    }

    let some_line =
        some_line.ok_or_else(|| ParseError(format!("{source}/{file}: missing 'some' line")))?;

    let (some_avg10, some_avg60, some_avg300, some_total) =
        parse_psi_line(source, file, "some", some_line)?;

    let (full_avg10, full_avg60, full_avg300, full_total) = if has_full {
        let line =
            full_line.ok_or_else(|| ParseError(format!("{source}/{file}: missing 'full' line")))?;
        let (a10, a60, a300, tot) = parse_psi_line(source, file, "full", line)?;
        (Some(a10), Some(a60), Some(a300), Some(tot))
    } else {
        (None, None, None, None)
    };

    Ok(PsiRow {
        ts,
        resource,
        some_avg10,
        some_avg60,
        some_avg300,
        some_total,
        full_avg10,
        full_avg60,
        full_avg300,
        full_total,
    })
}

/// Parse PSI files for cpu, memory, and io resources.
///
/// Each argument is `None` when the corresponding `/proc/pressure/<resource>`
/// file is absent (e.g. on kernels without PSI support). Absent resources
/// produce no row; if all three are `None` the returned vec is empty and the
/// caller skips the section.
///
/// # Errors
///
/// Returns [`ParseError`] when a present file cannot be parsed.
pub fn parse_pressure(
    cpu: Option<&str>,
    memory: Option<&str>,
    io: Option<&str>,
    ts: i64,
) -> Result<Vec<PsiRow>, ParseError> {
    parse_pressure_at(
        cpu,
        memory,
        io,
        ts,
        "/proc/pressure",
        ["cpu", "memory", "io"],
        false,
    )
}

pub(crate) fn parse_pressure_at(
    cpu: Option<&str>,
    memory: Option<&str>,
    io: Option<&str>,
    ts: i64,
    source: &str,
    files: [&str; 3],
    cpu_full: bool,
) -> Result<Vec<PsiRow>, ParseError> {
    let mut rows = Vec::with_capacity(3);

    if let Some(content) = cpu {
        let has_full = cpu_full && content.lines().any(|line| line.starts_with("full "));
        rows.push(parse_resource(source, files[0], 0, content, has_full, ts)?);
    }
    if let Some(content) = memory {
        rows.push(parse_resource(source, files[1], 1, content, true, ts)?);
    }
    if let Some(content) = io {
        rows.push(parse_resource(source, files[2], 2, content, true, ts)?);
    }

    Ok(rows)
}

impl PsiRow {
    /// Registry row for `1_107_001` with the given scope.
    #[must_use]
    pub const fn to_section(self, scope: u8) -> OsPsi {
        OsPsi {
            ts: Ts(self.ts),
            resource: self.resource,
            some_avg10: self.some_avg10,
            some_avg60: self.some_avg60,
            some_avg300: self.some_avg300,
            some_total: self.some_total,
            full_avg10: self.full_avg10,
            full_avg60: self.full_avg60,
            full_avg300: self.full_avg300,
            full_total: self.full_total,
            scope,
        }
    }
}

#[cfg(test)]
#[path = "../tests/proc/pressure_cgroup_full.rs"]
mod cgroup_full_tests;
#[cfg(test)]
#[path = "../tests/proc/pressure.rs"]
mod tests;
