//! What the demo measured, and how it renders.

use std::fmt::Write as _;

use crate::sections::SectionRows;

/// One run's measurements.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Report {
    /// How long the collector ran, seconds.
    pub(crate) duration_s: u64,
    /// Finished segments found under the data root.
    pub(crate) segments: usize,
    /// Total bytes of those segments.
    pub(crate) segment_bytes: u64,
    /// Bytes still in the raw journal at exit.
    pub(crate) journal_bytes: u64,
    /// Peak resident set of the collector process, bytes.
    pub(crate) peak_rss_bytes: u64,
    /// User plus system CPU consumed by the collector, milliseconds.
    pub(crate) cpu_ms: u64,
    /// Rows per section across the run's segments, in type-id order.
    pub(crate) sections: Vec<SectionRows>,
}

impl Report {
    /// Mean bytes per finished segment, or `0` when nothing was finished.
    pub(crate) const fn mean_segment_bytes(&self) -> u64 {
        if self.segments == 0 {
            0
        } else {
            self.segment_bytes / self.segments as u64
        }
    }

    /// CPU consumed as hundredths of a percent of one core, over the wall
    /// clock. Integer so the summary never depends on float formatting.
    pub(crate) const fn cpu_centipercent_of_one_core(&self) -> u64 {
        if self.duration_s == 0 {
            return 0;
        }
        // cpu_ms / (duration_s * 1000) as a percentage, times 100.
        self.cpu_ms.saturating_mul(10) / self.duration_s
    }

    /// The operator-facing summary.
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "duration_s      {}", self.duration_s);
        let _ = writeln!(out, "segments        {}", self.segments);
        let _ = writeln!(out, "segment_bytes   {}", self.segment_bytes);
        let _ = writeln!(out, "mean_segment    {}", self.mean_segment_bytes());
        let _ = writeln!(out, "journal_bytes   {}", self.journal_bytes);
        let _ = writeln!(out, "peak_rss_bytes  {}", self.peak_rss_bytes);
        let _ = writeln!(out, "cpu_ms          {}", self.cpu_ms);
        let centi = self.cpu_centipercent_of_one_core();
        let _ = writeln!(out, "cpu_percent     {}.{:02}", centi / 100, centi % 100);
        for section in &self.sections {
            let _ = writeln!(
                out,
                "section         {} {} {} rows",
                section.type_id, section.name, section.rows
            );
        }
        out
    }

    /// The same numbers as JSON, for a benchmark to diff across runs.
    pub(crate) fn to_json(&self) -> String {
        let sections: Vec<String> = self
            .sections
            .iter()
            .map(|section| {
                format!(
                    "{{\"type_id\":{},\"name\":\"{}\",\"rows\":{}}}",
                    section.type_id, section.name, section.rows
                )
            })
            .collect();
        format!(
            "{{\"duration_s\":{},\"segments\":{},\"segment_bytes\":{},\
             \"mean_segment_bytes\":{},\"journal_bytes\":{},\
             \"peak_rss_bytes\":{},\"cpu_ms\":{},\"sections\":[{}]}}\n",
            self.duration_s,
            self.segments,
            self.segment_bytes,
            self.mean_segment_bytes(),
            self.journal_bytes,
            self.peak_rss_bytes,
            self.cpu_ms,
            sections.join(",")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Report;
    use crate::sections::SectionRows;

    fn report() -> Report {
        Report {
            duration_s: 60,
            segments: 4,
            segment_bytes: 400_000,
            journal_bytes: 1_024,
            peak_rss_bytes: 12_000_000,
            cpu_ms: 1_200,
            sections: vec![SectionRows {
                type_id: 2_001_001,
                name: "pg_log_errors",
                rows: 7,
            }],
        }
    }

    #[test]
    fn the_mean_segment_is_the_total_over_the_count() {
        assert_eq!(report().mean_segment_bytes(), 100_000);
    }

    #[test]
    fn a_run_that_finished_nothing_reports_a_zero_mean_rather_than_dividing() {
        let empty = Report {
            segments: 0,
            segment_bytes: 0,
            ..report()
        };
        assert_eq!(empty.mean_segment_bytes(), 0);
    }

    #[test]
    fn cpu_percent_is_of_one_core_over_the_wall_clock() {
        // 1.2 s of CPU over 60 s is 2.00 % of one core.
        assert_eq!(report().cpu_centipercent_of_one_core(), 200);
        let instant = Report {
            duration_s: 0,
            ..report()
        };
        assert_eq!(instant.cpu_centipercent_of_one_core(), 0);
    }

    #[test]
    fn json_carries_every_measured_field() {
        let json = report().to_json();
        for key in [
            "duration_s",
            "segments",
            "segment_bytes",
            "mean_segment_bytes",
            "journal_bytes",
            "peak_rss_bytes",
            "cpu_ms",
        ] {
            assert!(json.contains(key), "{key} missing from {json}");
        }
        assert!(
            json.contains(r#"{"type_id":2001001,"name":"pg_log_errors","rows":7}"#),
            "the section totals are missing from {json}"
        );
    }

    #[test]
    fn the_summary_names_every_measured_field() {
        let text = report().render();
        assert!(text.contains("segments        4"));
        assert!(text.contains("peak_rss_bytes  12000000"));
        assert!(text.contains("cpu_percent     2.00"));
        assert!(text.contains("section         2001001 pg_log_errors 7 rows"));
    }
}
