//! Batches retained until the collector admits them to the WAL.

use kronika_registry::{
    PgWalStorage, pg_stat_statements_info::PgStatStatementsInfo,
    pg_store_plans_info::PgStorePlansInfo,
};
use kronika_source_pg::{
    activity::{ActivityRow, ActivityVersion},
    archiver::ArchiverRow,
    bgwriter::BgwriterSnapshot,
    checkpointer::CheckpointerSnapshot,
    database::{DatabaseRow, DatabaseVersion},
    io::{IoRow, IoVersion},
    locks::{LockRow, LocksVersion},
    prepared_xacts::PreparedXactsRow,
    progress_vacuum::ProgressVacuumRow,
    settings::SettingsRow,
    statements::{StatementsRow, StatementsVersion},
    store_plans::{DatasentinelRow, OsscRow, VadvRow},
    user_indexes::{UserIndexesRow, UserIndexesVersion},
    user_tables::{UserTablesRow, UserTablesVersion},
    wal::WalSnapshot,
};
use std::sync::Arc;

/// One bounded `PostgreSQL` batch retained until it reaches the WAL.
#[derive(Debug)]
pub(crate) enum PgBatch {
    Settings(Arc<[SettingsRow]>),
    Archiver(ArchiverRow),
    Bgwriter(BgwriterSnapshot),
    Checkpointer(CheckpointerSnapshot),
    Wal(WalSnapshot),
    WalStorage(PgWalStorage),
    PreparedXacts(Vec<PreparedXactsRow>),
    Database(DatabaseVersion, Vec<DatabaseRow>),
    Io(IoVersion, Vec<IoRow>),
    Activity(ActivityVersion, Vec<ActivityRow>),
    Locks(LocksVersion, Vec<LockRow>),
    ProgressVacuum(Vec<ProgressVacuumRow>),
    Statements(StatementsVersion, Vec<StatementsRow>),
    StatementsInfo(PgStatStatementsInfo),
    StorePlansOssc(Vec<OsscRow>),
    StorePlansDatasentinel(Vec<DatasentinelRow>),
    StorePlansVadv(Vec<VadvRow>),
    StorePlansInfo(PgStorePlansInfo),
    UserTables(UserTablesVersion, Vec<UserTablesRow>),
    UserIndexes(UserIndexesVersion, Vec<UserIndexesRow>),
}
