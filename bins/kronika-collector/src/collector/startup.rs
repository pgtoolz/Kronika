//! Validate configured sources, acquire storage ownership, and recover the journal.

use anyhow::{Context, Result};
use kronika_layout::{DataRoot, LayoutLimits, TemporaryKind, WriterOwner};
use kronika_writer::Journal;

use crate::config::Config;
use crate::log_sources::LogSources;
use crate::logging::{LogLevel, field, log_event};
use crate::segments::{open_collector_journal, report_written};
use kronika_source_pg::PgCollector;

/// Validate connections and transport settings before changing collector storage.
/// Then take exclusive writer ownership and publish any recoverable journal.
///
/// # Errors
///
/// Returns errors from source initialization, storage setup, the temporary-file
/// scan, or journal recovery.
pub(crate) fn initialize_collector(
    config: &Config,
) -> Result<(WriterOwner, Journal, LogSources, PgCollector)> {
    let logs = LogSources::open(config).context("open the configured log files")?;
    let pg = crate::pg_sources::open(config).context("open the configured PostgreSQL metrics")?;

    std::fs::create_dir_all(&config.storage_dir).context("create the storage directory")?;
    let data_root = DataRoot::open(&config.storage_dir).context("open the data root")?;
    let writer_owner = data_root
        .acquire_writer(LayoutLimits::default())
        .context("acquire exclusive writer ownership")?;
    remove_writer_temporaries(&writer_owner, LayoutLimits::default())?;
    let (journal, recovered) = open_collector_journal(&writer_owner, config.journal_max_bytes)?;
    if let Some(dest) = recovered {
        report_written(&dest, "recovered");
    }
    Ok((writer_owner, journal, logs, pg))
}

/// Exclusive writer ownership makes unpublished ZMS files from a prior process
/// safe to remove. Report individual removal failures and continue recovery.
fn remove_writer_temporaries(owner: &WriterOwner, limits: LayoutLimits) -> Result<()> {
    let snapshot = owner
        .root()
        .scan(limits)
        .context("scan for stale writer temporaries")?;
    for temporary in &snapshot.temporaries {
        if temporary.kind != TemporaryKind::Zms {
            continue;
        }
        if let Err(error) = owner.remove_temporary(temporary) {
            log_event(
                LogLevel::Warn,
                "writer_temporary_remove_failed",
                &[field("error", format!("{error}"))],
            );
        }
    }
    Ok(())
}
