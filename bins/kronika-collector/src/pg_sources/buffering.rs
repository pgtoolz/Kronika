//! Interning and buffering one retained `PostgreSQL` batch.

use anyhow::Result;
use kronika_registry::StrId;
use kronika_source_pg::activity::{self, ActivityRow, ActivityVersion};
use kronika_source_pg::archiver;
use kronika_source_pg::bgwriter::BgwriterSnapshot;
use kronika_source_pg::checkpointer::CheckpointerSnapshot;
use kronika_source_pg::database::{self, DatabaseRow, DatabaseVersion};
use kronika_source_pg::io::{self, IoRow, IoVersion};
use kronika_source_pg::locks::{self, LockRow, LocksVersion};
use kronika_source_pg::prepared_xacts;
use kronika_source_pg::progress_vacuum::{self, ProgressVacuumRow};
use kronika_source_pg::settings::{self, SettingsRow};
use kronika_source_pg::statements::{self, StatementsRow, StatementsVersion};
use kronika_source_pg::store_plans;
use kronika_source_pg::user_indexes::{self, UserIndexesRow, UserIndexesVersion};
use kronika_source_pg::user_tables::{self, UserTablesRow, UserTablesVersion};
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
        PgBatch::PreparedXacts(rows) => {
            for row in rows {
                buffer_row(
                    buffers,
                    prepared_xacts::to_prepared_xacts(row, intern(interner))?,
                )?;
            }
            Ok(())
        }
        PgBatch::Database(version, rows) => push_database(buffers, interner, *version, rows),
        PgBatch::Io(version, rows) => push_io(buffers, interner, *version, rows),
        PgBatch::Activity(version, rows) => push_activity(buffers, interner, *version, rows),
        PgBatch::Locks(version, rows) => push_locks(buffers, interner, *version, rows),
        PgBatch::ProgressVacuum(rows) => {
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
        PgBatch::Statements(version, rows) => push_statements(buffers, interner, *version, rows),
        PgBatch::StatementsInfo(row) => buffer_row(buffers, *row),
        PgBatch::StorePlansOssc(rows) => {
            for row in rows {
                buffer_row(buffers, store_plans::to_ossc(row, intern(interner))?)?;
            }
            Ok(())
        }
        PgBatch::StorePlansDatasentinel(rows) => {
            for row in rows {
                buffer_row(
                    buffers,
                    store_plans::to_datasentinel(row, intern(interner))?,
                )?;
            }
            Ok(())
        }
        PgBatch::StorePlansVadv(rows) => {
            for row in rows {
                buffer_row(buffers, store_plans::to_vadv(row, intern(interner))?)?;
            }
            Ok(())
        }
        PgBatch::StorePlansInfo(row) => buffer_row(buffers, *row),
        PgBatch::UserTables(version, rows) => push_user_tables(buffers, interner, *version, rows),
        PgBatch::UserIndexes(version, rows) => push_user_indexes(buffers, interner, *version, rows),
    }
}

fn push_locks(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    version: LocksVersion,
    rows: &[LockRow],
) -> Result<()> {
    for row in rows {
        match version {
            LocksVersion::V1 => buffer_row(buffers, locks::to_v1(row, intern(interner))?)?,
            LocksVersion::V2 => buffer_row(buffers, locks::to_v2(row, intern(interner))?)?,
        }
    }
    Ok(())
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

fn push_database(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    version: DatabaseVersion,
    rows: &[DatabaseRow],
) -> Result<()> {
    for row in rows {
        match version {
            DatabaseVersion::V1 => buffer_row(buffers, database::to_v1(row, intern(interner))?)?,
            DatabaseVersion::V2 => buffer_row(buffers, database::to_v2(row, intern(interner))?)?,
            DatabaseVersion::V3 => buffer_row(buffers, database::to_v3(row, intern(interner))?)?,
            DatabaseVersion::V4 => buffer_row(buffers, database::to_v4(row, intern(interner))?)?,
        }
    }
    Ok(())
}

fn push_io(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    version: IoVersion,
    rows: &[IoRow],
) -> Result<()> {
    for row in rows {
        match version {
            IoVersion::V1 => buffer_row(buffers, io::to_v1(row, intern(interner))?)?,
            IoVersion::V2 => buffer_row(buffers, io::to_v2(row, intern(interner))?)?,
        }
    }
    Ok(())
}

fn push_activity(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    version: ActivityVersion,
    rows: &[ActivityRow],
) -> Result<()> {
    for row in rows {
        match version {
            ActivityVersion::V1 => buffer_row(buffers, activity::to_v1(row, intern(interner))?)?,
            ActivityVersion::V2 => buffer_row(buffers, activity::to_v2(row, intern(interner))?)?,
            ActivityVersion::V3 => buffer_row(buffers, activity::to_v3(row, intern(interner))?)?,
        }
    }
    Ok(())
}

fn push_statements(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    version: StatementsVersion,
    rows: &[StatementsRow],
) -> Result<()> {
    for row in rows {
        match version {
            StatementsVersion::V1 => {
                buffer_row(buffers, statements::to_v1(row, intern(interner))?)?;
            }
            StatementsVersion::V2 => {
                buffer_row(buffers, statements::to_v2(row, intern(interner))?)?;
            }
            StatementsVersion::V3 => {
                buffer_row(buffers, statements::to_v3(row, intern(interner))?)?;
            }
            StatementsVersion::V4 => {
                buffer_row(buffers, statements::to_v4(row, intern(interner))?)?;
            }
            StatementsVersion::V5 => {
                buffer_row(buffers, statements::to_v5(row, intern(interner))?)?;
            }
            StatementsVersion::V6 => {
                buffer_row(buffers, statements::to_v6(row, intern(interner))?)?;
            }
        }
    }
    Ok(())
}

fn push_user_tables(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    version: UserTablesVersion,
    rows: &[UserTablesRow],
) -> Result<()> {
    for row in rows {
        match version {
            UserTablesVersion::V1 => {
                buffer_row(buffers, user_tables::to_v1(row, intern(interner))?)?;
            }
            UserTablesVersion::V2 => {
                buffer_row(buffers, user_tables::to_v2(row, intern(interner))?)?;
            }
            UserTablesVersion::V3 => {
                buffer_row(buffers, user_tables::to_v3(row, intern(interner))?)?;
            }
            UserTablesVersion::V4 => {
                buffer_row(buffers, user_tables::to_v4(row, intern(interner))?)?;
            }
        }
    }
    Ok(())
}

fn push_user_indexes(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    version: UserIndexesVersion,
    rows: &[UserIndexesRow],
) -> Result<()> {
    for row in rows {
        match version {
            UserIndexesVersion::V1 => {
                buffer_row(buffers, user_indexes::to_v1(row, intern(interner))?)?;
            }
            UserIndexesVersion::V2 => {
                buffer_row(buffers, user_indexes::to_v2(row, intern(interner))?)?;
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
