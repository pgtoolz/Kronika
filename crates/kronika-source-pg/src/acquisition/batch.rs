//! Batches retained until the collector admits them to the WAL.

use crate::{
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
use kronika_registry::{
    PgWalStorage, pg_stat_statements_info::PgStatStatementsInfo,
    pg_store_plans_info::PgStorePlansInfo,
};
use std::sync::Arc;

/// One bounded `PostgreSQL` batch retained until it reaches the WAL.
#[derive(Debug)]
pub enum PgBatch {
    /// Current server settings.
    Settings(Arc<[SettingsRow]>),
    /// WAL archiver counters.
    Archiver(ArchiverRow),
    /// Background writer counters.
    Bgwriter(BgwriterSnapshot),
    /// Checkpointer counters.
    Checkpointer(CheckpointerSnapshot),
    /// WAL generation counters.
    Wal(WalSnapshot),
    /// WAL directory capacity.
    WalStorage(PgWalStorage),
    /// Prepared transactions.
    PreparedXacts(Vec<PreparedXactsRow>),
    /// Per-database counters with their section version.
    Database(DatabaseVersion, Vec<DatabaseRow>),
    /// I/O counters with their section version.
    Io(IoVersion, Vec<IoRow>),
    /// Activity rows with their section version.
    Activity(ActivityVersion, Vec<ActivityRow>),
    /// Blocking lock rows with their section version.
    Locks(LocksVersion, Vec<LockRow>),
    /// Vacuum progress rows.
    ProgressVacuum(Vec<ProgressVacuumRow>),
    /// Statement rows with their extension version.
    Statements(StatementsVersion, Vec<StatementsRow>),
    /// Statement extension reset and deallocation counters.
    StatementsInfo(PgStatStatementsInfo),
    /// OSSC-compatible plan rows.
    StorePlansOssc(Vec<OsscRow>),
    /// Datasentinel plan rows.
    StorePlansDatasentinel(Vec<DatasentinelRow>),
    /// Vadv plan rows.
    StorePlansVadv(Vec<VadvRow>),
    /// Plan extension counters.
    StorePlansInfo(PgStorePlansInfo),
    /// Per-table counters with their section version.
    UserTables(UserTablesVersion, Vec<UserTablesRow>),
    /// Per-index counters with their section version.
    UserIndexes(UserIndexesVersion, Vec<UserIndexesRow>),
}
