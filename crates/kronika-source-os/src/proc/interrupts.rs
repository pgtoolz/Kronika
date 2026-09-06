//! Parse `/proc/interrupts` (`1_114`) and `/proc/softirqs` (`1_115`).

/// One interrupt line: its name, the per-CPU sum, and the trailing device
/// text when the kernel prints one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptRow {
    /// IRQ name from the first column, without the trailing colon.
    pub irq: String,
    /// Interrupts on this line since boot, summed across CPUs.
    pub count: i64,
    /// Trailing free-text device description, or `None` when the line has
    /// nothing past the counters.
    pub device: Option<String>,
}

/// Parse `/proc/interrupts`.
///
/// The first line names the CPU columns; every later line is
/// `NAME: <count per CPU>... [description]`. Counts are summed because the
/// per-CPU split costs one row per CPU per line for a number that only
/// matters when chasing IRQ affinity. Lines whose counters do not parse are
/// skipped: the file mixes numeric IRQs with architecture-specific rows whose
/// shape varies by kernel, and one odd row must not lose the rest.
#[must_use]
pub fn parse_interrupts(content: &str, cpu_count: usize) -> Vec<InterruptRow> {
    let mut rows = Vec::new();
    for line in content.lines().skip(1) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut count: i64 = 0;
        let mut counted = 0_usize;
        let mut tail = rest;
        for token in rest.split_whitespace() {
            if counted == cpu_count {
                break;
            }
            let Ok(value) = token.parse::<i64>() else {
                break;
            };
            count = count.saturating_add(value);
            counted += 1;
        }
        if counted == 0 {
            continue;
        }
        // Everything after the numeric block is the description.
        for _ in 0..counted {
            tail = tail.trim_start();
            tail = tail
                .split_at(tail.find(char::is_whitespace).unwrap_or(tail.len()))
                .1;
        }
        let device = {
            let text = tail.trim();
            (!text.is_empty()).then(|| text.to_owned())
        };
        rows.push(InterruptRow {
            irq: name.to_owned(),
            count,
            device,
        });
    }
    rows
}

/// One softirq vector and its per-CPU sum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftirqRow {
    /// Vector name (`TIMER`, `NET_RX`, ...).
    pub vector: String,
    /// Softirqs of this vector since boot, summed across CPUs.
    pub count: i64,
}

/// Parse `/proc/softirqs`.
///
/// Same shape as `/proc/interrupts` without the trailing description. The
/// kernel defines the vector set, so the row count is fixed at around ten and
/// needs no caller-supplied bound.
#[must_use]
pub fn parse_softirqs(content: &str) -> Vec<SoftirqRow> {
    let mut rows = Vec::new();
    for line in content.lines().skip(1) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut count: i64 = 0;
        let mut counted = 0_usize;
        for token in rest.split_whitespace() {
            let Ok(value) = token.parse::<i64>() else {
                break;
            };
            count = count.saturating_add(value);
            counted += 1;
        }
        if counted == 0 {
            continue;
        }
        rows.push(SoftirqRow {
            vector: name.to_owned(),
            count,
        });
    }
    rows
}

#[cfg(test)]
mod tests;
