//! Opening the journal at startup and publishing a readable WAL.

use super::{
    Context, FileKind, Instant, Journal, JournalConfig, LogLevel, PathBuf, Result, SegmentAddress,
    WriterOwner, duration_ms, field, log_close_failure, log_event, peak_rss_kib, write_segment,
};

/// Open the journal under the storage directory and write out windows a
/// previous process left behind, so a restart loses no collected data.
///
/// A readable non-empty journal gets one publication attempt. Any open,
/// validation, publication, or reset failure is fatal and leaves `active.wal`
/// in its canonical location for inspection or a later retry.
pub(crate) fn open_collector_journal(
    owner: &WriterOwner,
    journal_max_bytes: u64,
) -> Result<(Journal, Option<PathBuf>)> {
    let config = JournalConfig {
        max_journal_len: usize::try_from(journal_max_bytes)
            .context("KRONIKA_JOURNAL_MAX_BYTES exceeds usize")?,
        ..JournalConfig::default()
    };
    let mut journal = Journal::open(owner, config)
        .context("open active.wal; the existing file is preserved on failure")?;
    if journal.parts().is_empty() {
        return Ok((journal, None));
    }
    let dest = write_recovered_journal(&mut journal, owner)?;
    Ok((journal, dest))
}

/// Write recovered windows under the exact identity persisted in journal v1.
pub(super) fn write_recovered_journal(
    journal: &mut Journal,
    owner: &WriterOwner,
) -> Result<Option<PathBuf>> {
    let segment_id = journal
        .segment_id()
        .context("an active journal must carry SegmentId")?;
    for part in journal.parts().to_vec() {
        let body = journal.read_part(part).context("read a recovered part")?;
        let catalog = kronika_format::validate_part(&body).context("validate a recovered part")?;
        let has_data = catalog.entries.iter().any(|entry| {
            !matches!(
                entry.type_id,
                kronika_registry::DICT_STRINGS_TYPE_ID | kronika_registry::DICT_BLOBS_TYPE_ID
            )
        });
        if has_data
            && (catalog.min_ts == i64::MAX
                || catalog.max_ts == i64::MIN
                || catalog.min_ts > catalog.max_ts)
        {
            anyhow::bail!(
                "recovered part has invalid data timestamp bounds {}..{}; active.wal is preserved",
                catalog.min_ts,
                catalog.max_ts,
            );
        }
    }
    let address = SegmentAddress::new(segment_id).context("derive the recovered UTC address")?;
    let dest = owner.root().diagnostic_file_path(address, FileKind::Zms);
    let journal_bytes = journal.bytes();
    let journal_parts = journal.parts().len();
    let started = Instant::now();
    let summary = match write_segment(journal, owner, address) {
        Ok(summary) => summary,
        Err(error) => {
            log_close_failure(
                &dest,
                segment_id,
                "recovered",
                "write",
                journal_bytes,
                journal_parts,
                started.elapsed(),
                &error,
            );
            return Err(anyhow::Error::new(error).context("write the recovered segment"));
        }
    };
    log_event(
        LogLevel::Info,
        "segment_write_finish",
        &[
            field("segment_path", dest.display()),
            field("segment_id", segment_id.get()),
            field("reason", "recovered"),
            field("sections", summary.sections),
            field("segment_bytes", summary.bytes),
            field("journal_bytes", journal_bytes),
            field("journal_parts", journal_parts),
            field("min_ts", summary.min_ts),
            field("max_ts", summary.max_ts),
            field("elapsed_ms", duration_ms(started.elapsed())),
            field("rss_kib", peak_rss_kib()),
        ],
    );
    if let Err(error) = journal.reset() {
        log_close_failure(
            &dest,
            segment_id,
            "recovered",
            "journal-reset",
            journal_bytes,
            journal_parts,
            started.elapsed(),
            &error,
        );
        return Err(anyhow::Error::new(error)
            .context("reset the journal after the recovered segment write"));
    }
    Ok(Some(dest))
}
