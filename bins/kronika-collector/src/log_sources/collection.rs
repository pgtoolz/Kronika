//! Read log batches, admit their rows, then acknowledge their input positions.

use anyhow::Context as _;
use kronika_registry::SECTION_WRITE_BATCH_ROWS;
use kronika_source_log::MAX_READ_BYTES;
use std::time::Instant;

use super::{LogRows, LogSources, PgBouncerBatch, PostgresBatch};
use crate::logging::{log_collection_failure, log_collection_finish, log_collection_start};

// PostgreSQL collection diagnostics use the error-event section's type ID;
// one read can also produce checkpoints, vacuum events and other sections.
const PG_LOG_TYPE_ID: u32 = 2_001_001;
// PgBouncer collection diagnostics use its single event section's type ID.
const PGBOUNCER_TYPE_ID: u32 = 2_100_001;

// Limit each file's raw I/O per scheduled cycle so one busy log cannot consume
// an unbounded amount of work. Each new batch must fit its worst-case 4 MiB read.
const MAX_SOURCE_READ_BYTES: usize = 256 * 1_048_576;

impl LogSources {
    pub(super) fn collect_files(
        &mut self,
        admit: &mut impl FnMut(&LogRows) -> anyhow::Result<bool>,
        offsets_changed: &mut bool,
    ) -> anyhow::Result<bool> {
        // The readers have different batch types but share the same admission
        // protocol. A rejected batch must stay replayable; only accepted input
        // may advance the saved position.
        macro_rules! collect_file {
            (
                $log:ident, $type_id:expr, $format:expr,
                $batch:ident = $read:expr,
                count = $row_count:expr,
                rows = $rows:expr,
                acknowledge = $ack_context:literal
            ) => {{
                let started = Instant::now();
                let type_id = $type_id;
                let format = $format;
                log_collection_start(type_id, format);
                let mut raw_bytes = 0_usize;
                let mut event_rows = 0_usize;
                let mut read_failed = false;
                while MAX_SOURCE_READ_BYTES.saturating_sub(raw_bytes) >= MAX_READ_BYTES {
                    let $batch = match $read {
                        Ok(batch) => batch,
                        Err(error) => {
                            log_collection_failure(type_id, format, &error, started.elapsed());
                            read_failed = true;
                            break;
                        }
                    };
                    raw_bytes = raw_bytes.saturating_add($batch.raw_bytes);
                    event_rows = event_rows.saturating_add($row_count);
                    let at_eof = $batch.at_eof;
                    let made_progress = $batch.raw_bytes != 0 || $batch.needs_ack;
                    if $batch.needs_ack {
                        let accepted = if $batch.events.is_empty() {
                            true
                        } else {
                            let rows = $rows;
                            admit(&rows)?
                        };
                        if !accepted {
                            $log.retry();
                            log_collection_finish(type_id, format, event_rows, started.elapsed());
                            return Ok(false);
                        }
                        let position = $log.acknowledge().context($ack_context)?;
                        self.offsets
                            .set(&$log.path().display().to_string(), position);
                        *offsets_changed = true;
                    }
                    if at_eof || !made_progress {
                        break;
                    }
                }
                if !read_failed {
                    log_collection_finish(type_id, format, event_rows, started.elapsed());
                }
            }};
        }

        for source in &mut self.postgres {
            let log = &mut source.log;
            collect_file!(
                log,
                PG_LOG_TYPE_ID,
                log.format().as_str(),
                batch = log.read_batch(
                    || crate::clock::unix_now_us().map_err(std::io::Error::other),
                    SECTION_WRITE_BATCH_ROWS,
                    self.pg_log_max_lag_secs,
                ),
                count = batch.events.rows(),
                rows = LogRows {
                    postgres: vec![PostgresBatch {
                        system_identifier: source.system_identifier,
                        source_file: log.path().display().to_string(),
                        events: batch.events,
                    }],
                    pgbouncer: Vec::new(),
                },
                acknowledge = "acknowledge the admitted PostgreSQL log batch"
            );
        }
        for log in &mut self.pgbouncer {
            collect_file!(
                log,
                PGBOUNCER_TYPE_ID,
                "pgbouncer",
                batch = log.read_batch(SECTION_WRITE_BATCH_ROWS),
                count = batch.events.len(),
                rows = LogRows {
                    postgres: Vec::new(),
                    pgbouncer: vec![PgBouncerBatch {
                        source_file: log.path().display().to_string(),
                        events: batch.events,
                    }],
                },
                acknowledge = "acknowledge the admitted PgBouncer log batch"
            );
        }
        Ok(true)
    }
}
