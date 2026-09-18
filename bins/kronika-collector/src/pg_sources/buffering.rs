//! Interning and buffering one retained `PostgreSQL` batch.

use anyhow::Result;
use kronika_registry::StrId;
use kronika_source_pg::activity::{self, ActivityVersion};
use kronika_source_pg::archiver;
use kronika_source_pg::bgwriter::BgwriterSnapshot;
use kronika_source_pg::checkpointer::CheckpointerSnapshot;
use kronika_source_pg::database::{self, DatabaseVersion};
use kronika_source_pg::io::{self, IoVersion};
use kronika_source_pg::locks::{self, LocksVersion};
use kronika_source_pg::prepared_xacts;
use kronika_source_pg::progress_vacuum::{self, ProgressVacuumRow};
use kronika_source_pg::settings::{self, SettingsRow};
use kronika_source_pg::statements::{self, StatementsVersion};
use kronika_source_pg::store_plans;
use kronika_source_pg::user_indexes::{self, UserIndexesVersion};
use kronika_source_pg::user_tables::{self, UserTablesVersion};
use kronika_source_pg::wal::WalSnapshot;
use kronika_writer::{Interner, SectionBuffers};

use super::PgBatch;
use crate::buffering::buffer_row;

/// Move one retained `PostgreSQL` batch into a window.
///
/// `opening_settings` is included only when this batch opens a segment. A
/// settings batch carries the same rows itself and is never duplicated.
pub(crate) fn push_pg_batch(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    batch: &PgBatch,
    opening_settings: &[SettingsRow],
) -> Result<()> {
    if !matches!(batch, PgBatch::Settings(_)) {
        push_settings(buffers, interner, opening_settings)?;
    }
    // Convert and buffer each row before touching the next one; stop on the first error.
    macro_rules! push_rows {
        ($rows:expr, $convert:path) => {{
            for row in $rows {
                buffer_row(buffers, $convert(row, intern(interner))?)?;
            }
            Ok(())
        }};
    }

    match batch {
        PgBatch::Settings(rows) => push_settings(buffers, interner, rows),
        PgBatch::Archiver(row) => {
            buffer_row(buffers, archiver::to_archiver(row, intern(interner))?)
        }
        PgBatch::Bgwriter(BgwriterSnapshot::V1(row)) => buffer_row(buffers, *row),
        PgBatch::Bgwriter(BgwriterSnapshot::V2(row)) => buffer_row(buffers, *row),
        PgBatch::Checkpointer(CheckpointerSnapshot::V1(row)) => buffer_row(buffers, *row),
        PgBatch::Checkpointer(CheckpointerSnapshot::V2(row)) => buffer_row(buffers, *row),
        PgBatch::Wal(WalSnapshot::V1(row)) => buffer_row(buffers, *row),
        PgBatch::Wal(WalSnapshot::V2(row)) => buffer_row(buffers, *row),
        PgBatch::WalStorage(row) => buffer_row(buffers, *row),
        PgBatch::PreparedXacts(rows) => push_rows!(rows, prepared_xacts::to_prepared_xacts),
        PgBatch::Database(version, rows) => match version {
            DatabaseVersion::V1 => push_rows!(rows, database::to_v1),
            DatabaseVersion::V2 => push_rows!(rows, database::to_v2),
            DatabaseVersion::V3 => push_rows!(rows, database::to_v3),
            DatabaseVersion::V4 => push_rows!(rows, database::to_v4),
        },
        PgBatch::Io(version, rows) => match version {
            IoVersion::V1 => push_rows!(rows, io::to_v1),
            IoVersion::V2 => push_rows!(rows, io::to_v2),
        },
        PgBatch::Activity(version, rows) => match version {
            ActivityVersion::V1 => push_rows!(rows, activity::to_v1),
            ActivityVersion::V2 => push_rows!(rows, activity::to_v2),
            ActivityVersion::V3 => push_rows!(rows, activity::to_v3),
        },
        PgBatch::Locks(version, rows) => match version {
            LocksVersion::V1 => push_rows!(rows, locks::to_v1),
            LocksVersion::V2 => push_rows!(rows, locks::to_v2),
        },
        PgBatch::ProgressVacuum(rows) => push_progress_vacuum(buffers, interner, rows),
        PgBatch::Statements(version, rows) => match version {
            StatementsVersion::V1 => push_rows!(rows, statements::to_v1),
            StatementsVersion::V2 => push_rows!(rows, statements::to_v2),
            StatementsVersion::V3 => push_rows!(rows, statements::to_v3),
            StatementsVersion::V4 => push_rows!(rows, statements::to_v4),
            StatementsVersion::V5 => push_rows!(rows, statements::to_v5),
            StatementsVersion::V6 => push_rows!(rows, statements::to_v6),
        },
        PgBatch::StatementsInfo(row) => buffer_row(buffers, *row),
        PgBatch::StorePlansOssc(rows) => push_rows!(rows, store_plans::to_ossc),
        PgBatch::StorePlansDatasentinel(rows) => push_rows!(rows, store_plans::to_datasentinel),
        PgBatch::StorePlansVadv(rows) => push_rows!(rows, store_plans::to_vadv),
        PgBatch::StorePlansInfo(row) => buffer_row(buffers, *row),
        PgBatch::UserTables(version, rows) => match version {
            UserTablesVersion::V1 => push_rows!(rows, user_tables::to_v1),
            UserTablesVersion::V2 => push_rows!(rows, user_tables::to_v2),
            UserTablesVersion::V3 => push_rows!(rows, user_tables::to_v3),
            UserTablesVersion::V4 => push_rows!(rows, user_tables::to_v4),
        },
        PgBatch::UserIndexes(version, rows) => match version {
            UserIndexesVersion::V1 => push_rows!(rows, user_indexes::to_v1),
            UserIndexesVersion::V2 => push_rows!(rows, user_indexes::to_v2),
        },
    }
}

pub(crate) fn push_settings(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &[SettingsRow],
) -> Result<()> {
    for row in rows {
        buffer_row(buffers, settings::to_section(row, intern(interner))?)?;
    }
    Ok(())
}

fn push_progress_vacuum(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &[ProgressVacuumRow],
) -> Result<()> {
    // A batch may contain several layouts; preserve its row and dictionary order.
    for row in rows {
        match row {
            ProgressVacuumRow::V1(row) => {
                buffer_row(buffers, progress_vacuum::to_v1(row, intern(interner))?)?;
            }
            ProgressVacuumRow::V2(row) => {
                buffer_row(buffers, progress_vacuum::to_v2(row, intern(interner))?)?;
            }
            ProgressVacuumRow::V3(row) => {
                buffer_row(buffers, progress_vacuum::to_v3(row, intern(interner))?)?;
            }
        }
    }
    Ok(())
}

fn intern(interner: &mut Interner) -> impl FnMut(&[u8]) -> Result<StrId> + '_ {
    move |value: &[u8]| {
        interner
            .intern(value)
            .map(|id| StrId(id.get()))
            .map_err(|err| anyhow::anyhow!("intern a PostgreSQL string: {err}"))
    }
}

#[cfg(test)]
#[path = "../tests/pg_sources/buffering.rs"]
mod tests;
