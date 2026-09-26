//! Prometheus text-format exposition.
//!
//! Deterministic output: families sorted by name, samples within a family
//! sorted by label sequence, every catalog sample carrying its fetch
//! timestamp in epoch milliseconds.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::measurement::SampleSet;

/// One exposable sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Fully-qualified family name, `pgwatch_<metric>[_<column>]`.
    pub family: String,
    /// HELP text: the storage-name-resolved metric name.
    pub help: String,
    /// Whether the sample is a gauge rather than a counter.
    pub is_gauge: bool,
    /// Labels sorted by name.
    pub labels: Vec<(String, String)>,
    /// Sample value.
    pub value: f64,
}

/// One exposition family under construction: HELP text, gauge flag, samples.
type Family<'a> = (String, bool, Vec<(&'a SampleSet, &'a Sample)>);

/// Renders sample sets as Prometheus text format.
///
/// An empty result still ends with a trailing newline on the last line, as
/// the format requires one line per sample and the collector's HTTP layer
/// appends nothing.
#[allow(
    single_use_lifetimes,
    reason = "anonymous lifetimes in impl Trait are not stable; the iterator bound needs a named one"
)]
pub fn expose_samples<'a>(sets: impl IntoIterator<Item = &'a SampleSet>) -> String {
    let mut families: BTreeMap<&str, Family<'a>> = BTreeMap::new();
    for set in sets {
        for sample in &set.samples {
            let entry = families
                .entry(sample.family.as_str())
                .or_insert_with(|| (sample.help.clone(), sample.is_gauge, Vec::new()));
            // a family keeps one HELP/TYPE pair; conflicting declarations
            // cannot occur because gauge-ness and help derive from the
            // metric definition shared by all its samples
            entry.2.push((set, sample));
        }
    }

    let mut out = String::new();
    for (family, (help, is_gauge, mut samples)) in families {
        let kind = if is_gauge { "gauge" } else { "counter" };
        writeln!(
            out,
            "# HELP {family} {}\n# TYPE {family} {kind}",
            escape_help(&help)
        )
        .expect("writing to a String never fails");
        samples.sort_by(|a, b| a.1.labels.cmp(&b.1.labels));
        for (set, sample) in samples {
            let mut labels = sample.labels.clone();
            labels.sort_by(|a, b| a.0.cmp(&b.0));
            out.push_str(family);
            if !labels.is_empty() {
                out.push('{');
                for (i, (k, v)) in labels.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(k);
                    out.push_str("=\"");
                    escape_label_value_into(&mut out, v);
                    out.push('"');
                }
                out.push('}');
            }
            out.push(' ');
            out.push_str(&format_value(sample.value));
            out.push(' ');
            out.push_str(&set.timestamp_ms.to_string());
            out.push('\n');
        }
    }
    out
}

/// Escapes a label value per the text format: backslash, double quote,
/// newline. Shared by catalog samples and self-metric lines.
#[must_use]
pub fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    escape_label_value_into(&mut out, value);
    out
}

/// Escapes a label value per the text format: backslash, double quote, newline.
fn escape_label_value_into(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
}

fn escape_help(help: &str) -> String {
    help.replace('\\', "\\\\").replace('\n', "\\n")
}

/// Formats an f64 like Go's `strconv.FormatFloat(f, 'g', -1, 64)`.
///
/// Shortest round-trip digits; plain decimal while the decimal exponent
/// stays in `-4..6`, scientific `e+NN`/`e-NN` outside; zero prints `0` and
/// non-finite values print `NaN`/`+Inf`/`-Inf` as expfmt does.
///
/// # Panics
///
/// Panics only if Rust's `{:e}` formatting changes shape (no `e` separator
/// or a non-decimal exponent), which cannot happen for finite `f64`.
#[must_use]
pub fn format_value(value: f64) -> String {
    // the text-format spellings Go's expfmt writes for non-finite values
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    // Go prints zero (both signs) as "0", never "-0"
    if value == 0.0 {
        return "0".to_owned();
    }
    // Go 'g' with shortest digits picks %e when the decimal exponent is
    // < -4 or >= 6 (ftoa sets eprec=6 for shortest) — so 1e6 renders as
    // "1e+06". Rust's {:e} carries the same shortest digits plus that
    // exponent; plain rendering below uses Display, which never goes
    // scientific.
    let sci = format!("{value:e}");
    let (mantissa, exp) = sci
        .split_once('e')
        .expect("lowercase-exponential output keeps the 'e' separator");
    let exp: i32 = exp.parse().expect("the exponent part is plain decimal");
    if (-4..6).contains(&exp) {
        format!("{value}")
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        let digits = exp.unsigned_abs();
        format!("{mantissa}e{sign}{digits:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::instance_up_sample_set;

    fn sample_set(timestamp_ms: i64, samples: Vec<Sample>) -> SampleSet {
        SampleSet {
            timestamp_ms,
            samples,
            errors: 0,
        }
    }

    fn sample(family: &str, labels: &[(&str, &str)], value: f64) -> Sample {
        Sample {
            family: family.to_owned(),
            help: family.trim_start_matches("pgwatch_").to_owned(),
            is_gauge: false,
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            value,
        }
    }

    #[test]
    fn families_sorted_and_typed() {
        let set = sample_set(
            1000,
            vec![
                sample("pgwatch_b_x", &[], 1.0),
                sample("pgwatch_a_x", &[], 2.0),
                sample("pgwatch_a_x", &[("n", "2")], 3.0),
                sample("pgwatch_a_x", &[("n", "1")], 4.0),
            ],
        );
        let text = expose_samples(std::iter::once(&set));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "# HELP pgwatch_a_x a_x",
                "# TYPE pgwatch_a_x counter",
                "pgwatch_a_x 2 1000",
                "pgwatch_a_x{n=\"1\"} 4 1000",
                "pgwatch_a_x{n=\"2\"} 3 1000",
                "# HELP pgwatch_b_x b_x",
                "# TYPE pgwatch_b_x counter",
                "pgwatch_b_x 1 1000",
            ]
        );
    }

    #[test]
    fn sample_ordering_is_deterministic_across_sets() {
        let one = sample_set(
            5,
            vec![
                sample("pgwatch_m_v", &[("a", "1"), ("b", "1")], 1.0),
                sample("pgwatch_m_v", &[("a", "1"), ("b", "0")], 2.0),
                sample("pgwatch_m_w", &[], 3.0),
            ],
        );
        // same samples built in another order with unsorted label vectors
        let two = sample_set(
            5,
            vec![
                sample("pgwatch_m_w", &[], 3.0),
                sample("pgwatch_m_v", &[("b", "0"), ("a", "1")], 2.0),
                sample("pgwatch_m_v", &[("b", "1"), ("a", "1")], 1.0),
            ],
        );
        let a = expose_samples(std::iter::once(&one));
        let b = expose_samples(std::iter::once(&two));
        assert_eq!(a, b);
        // label order inside a sample follows sorted label names
        assert!(a.contains("pgwatch_m_v{a=\"1\",b=\"1\"} 1 5"), "{a}");
    }

    #[test]
    fn value_formats_match_go_g_shortest() {
        assert_eq!(format_value(0.0), "0");
        assert_eq!(format_value(-0.0), "0");
        assert_eq!(format_value(1.0), "1");
        assert_eq!(format_value(1.5), "1.5");
        assert_eq!(format_value(-7.0), "-7");
        assert_eq!(format_value(1024.0), "1024");
        assert_eq!(format_value(999_999.0), "999999");
        // Go switches to %e at decimal exponent 6 (eprec=6 for shortest)
        assert_eq!(format_value(1e6), "1e+06");
        assert_eq!(format_value(1_234_567.0), "1.234567e+06");
        assert_eq!(format_value(1e20), "1e+20");
        assert_eq!(
            format_value(1.234_567_890_123_456_8e20),
            "1.2345678901234568e+20"
        );
        assert_eq!(format_value(1e21), "1e+21");
        assert_eq!(format_value(1.5e22), "1.5e+22");
        assert_eq!(format_value(-1e21), "-1e+21");
        // small side: %e below decimal exponent -4
        assert_eq!(format_value(0.001), "0.001");
        assert_eq!(format_value(1e-4), "0.0001");
        assert_eq!(format_value(1e-5), "1e-05");
        assert_eq!(format_value(-1.2e-7), "-1.2e-07");
        // expfmt spellings for non-finite values
        assert_eq!(format_value(f64::NAN), "NaN");
        assert_eq!(format_value(f64::INFINITY), "+Inf");
        assert_eq!(format_value(f64::NEG_INFINITY), "-Inf");
    }

    #[test]
    fn instance_up_renders_gauge() {
        let set = instance_up_sample_set("host_db", true, 1_700_000_000_000);
        let text = expose_samples(std::iter::once(&set));
        assert_eq!(
            text,
            "# HELP pgwatch_instance_up instance_up\n# TYPE pgwatch_instance_up gauge\npgwatch_instance_up{dbname=\"host_db\"} 1 1700000000000\n"
        );
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert_eq!(expose_samples(std::iter::empty()), "");
    }
}
