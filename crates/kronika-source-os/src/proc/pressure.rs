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
    /// Fraction of time tasks stalled (full) over the last 10 s. `None` for cpu.
    pub full_avg10: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 60 s. `None` for cpu.
    pub full_avg60: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 300 s. `None` for cpu.
    pub full_avg300: Option<f64>,
    /// Cumulative stall time (full), microseconds. `None` for cpu.
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
    )
}

pub(crate) fn parse_pressure_at(
    cpu: Option<&str>,
    memory: Option<&str>,
    io: Option<&str>,
    ts: i64,
    source: &str,
    files: [&str; 3],
) -> Result<Vec<PsiRow>, ParseError> {
    let mut rows = Vec::with_capacity(3);

    if let Some(content) = cpu {
        rows.push(parse_resource(source, files[0], 0, content, false, ts)?);
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
mod tests {
    use super::parse_pressure;

    const CPU_SAMPLE: &str = "\
some avg10=0.10 avg60=0.05 avg300=0.02 total=10000\n";

    const MEMORY_SAMPLE: &str = "\
some avg10=1.50 avg60=0.80 avg300=0.30 total=500000\n\
full avg10=0.20 avg60=0.10 avg300=0.05 total=100000\n";

    const IO_SAMPLE: &str = "\
some avg10=0.50 avg60=0.25 avg300=0.10 total=200000\n\
full avg10=0.05 avg60=0.02 avg300=0.01 total=20000\n";

    #[test]
    fn three_resources_yield_three_rows_cpu_full_is_none() {
        let rows = parse_pressure(
            Some(CPU_SAMPLE),
            Some(MEMORY_SAMPLE),
            Some(IO_SAMPLE),
            1_000,
        )
        .expect("parse");
        assert_eq!(rows.len(), 3);

        let cpu = &rows[0];
        assert_eq!(cpu.resource, 0);
        assert_eq!(cpu.ts, 1_000);
        assert!((cpu.some_avg10 - 0.10).abs() < 1e-9);
        assert!((cpu.some_avg60 - 0.05).abs() < 1e-9);
        assert!((cpu.some_avg300 - 0.02).abs() < 1e-9);
        assert_eq!(cpu.some_total, 10_000);
        assert_eq!(cpu.full_avg10, None);
        assert_eq!(cpu.full_avg60, None);
        assert_eq!(cpu.full_avg300, None);
        assert_eq!(cpu.full_total, None);

        let mem = &rows[1];
        assert_eq!(mem.resource, 1);
        assert!((mem.some_avg10 - 1.50).abs() < 1e-9);
        assert_eq!(mem.some_total, 500_000);
        assert!((mem.full_avg10.unwrap() - 0.20).abs() < 1e-9);
        assert_eq!(mem.full_total, Some(100_000));

        let io = &rows[2];
        assert_eq!(io.resource, 2);
        assert!((io.some_avg10 - 0.50).abs() < 1e-9);
        assert_eq!(io.some_total, 200_000);
        assert!((io.full_avg10.unwrap() - 0.05).abs() < 1e-9);
        assert_eq!(io.full_total, Some(20_000));
    }

    #[test]
    fn only_cpu_yields_one_row() {
        let rows = parse_pressure(Some(CPU_SAMPLE), None, None, 2_000).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].resource, 0);
        assert_eq!(rows[0].full_avg10, None);
        assert_eq!(rows[0].full_total, None);
    }

    #[test]
    fn all_absent_yields_empty_vec() {
        let rows = parse_pressure(None, None, None, 3_000).expect("parse");
        assert!(rows.is_empty());
    }

    #[test]
    fn to_section_carries_all_floor_fields_and_scope() {
        let rows = parse_pressure(
            Some(CPU_SAMPLE),
            Some(MEMORY_SAMPLE),
            Some(IO_SAMPLE),
            9_999,
        )
        .expect("parse");

        let cpu_section = rows[0].to_section(1);
        assert_eq!(cpu_section.ts.0, 9_999);
        assert_eq!(cpu_section.resource, 0);
        assert!((cpu_section.some_avg10 - 0.10).abs() < 1e-9);
        assert!((cpu_section.some_avg60 - 0.05).abs() < 1e-9);
        assert!((cpu_section.some_avg300 - 0.02).abs() < 1e-9);
        assert_eq!(cpu_section.some_total, 10_000);
        assert_eq!(cpu_section.full_avg10, None);
        assert_eq!(cpu_section.full_avg60, None);
        assert_eq!(cpu_section.full_avg300, None);
        assert_eq!(cpu_section.full_total, None);
        assert_eq!(cpu_section.scope, 1);

        let mem_section = rows[1].to_section(0);
        assert_eq!(mem_section.resource, 1);
        assert!((mem_section.full_avg10.unwrap() - 0.20).abs() < 1e-9);
        assert_eq!(mem_section.full_total, Some(100_000));
        assert_eq!(mem_section.scope, 0);
    }
}
