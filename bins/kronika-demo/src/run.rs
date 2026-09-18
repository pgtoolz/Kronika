//! Stop activity and the collector, retain its journal, then measure completed segments.

use anyhow::{Context, Result};
use std::sync::Arc;

use crate::collector::Collector;
use crate::config::Config;
use crate::report::Report;
use crate::system_activity::SystemActivity;
use crate::workload::Workload;
use crate::{sections, shutdown};

pub(crate) fn run(config: Config) -> Result<()> {
    std::fs::create_dir_all(&config.root).context("create the demo output directory")?;
    std::fs::create_dir_all(&config.storage_dir).context("create the demo storage directory")?;
    let stop = shutdown::watch().context("watch for shutdown signals")?;
    let system_activity = config
        .system_activity
        .as_ref()
        .map(|config| SystemActivity::start(config, Arc::clone(&stop)));
    let workload = config
        .workload
        .map(Workload::start)
        .transpose()
        .context("start the demo workload")?;
    let mut collector = Collector::start(
        &config.collector_bin,
        &config.storage_dir,
        &config.root,
        config.collector_log,
    )?;
    println!(
        "demo: collector pid {} for {}s, log {}",
        collector.pid(),
        config.duration_s,
        collector.log_description()
    );
    let measurement = collector.measure(config.duration_s, &stop);

    // Stop activity before the collector; its open segment stays in active.wal.
    if let Some(workload) = workload {
        workload.stop();
    }
    if let Some(activity) = system_activity {
        activity.stop();
    }
    let shutdown = collector.stop();
    let measurement = measurement?;
    shutdown?;

    let segments = sections::measure(&config.storage_dir)?;
    let journal_bytes = std::fs::metadata(config.storage_dir.join("active.wal"))
        .map(|meta| meta.len())
        .unwrap_or_default();
    let report = Report {
        duration_s: config.duration_s,
        segments: segments.count,
        segment_bytes: segments.bytes,
        journal_bytes,
        peak_rss_bytes: measurement.peak_rss_bytes,
        cpu_ms: measurement.cpu_ms,
        sections: segments.sections,
    };
    print!("{}", report.render());
    let report_path = config.root.join("report.json");
    std::fs::write(&report_path, report.to_json()).context("write report.json")?;
    println!("demo: report {}", report_path.display());
    Ok(())
}
