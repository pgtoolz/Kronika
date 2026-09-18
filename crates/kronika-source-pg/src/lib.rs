//! Collecting metrics from a `PostgreSQL` server.
//!
//! [`PgCollector`] owns connection generations, discovery, capability caches and
//! bounded acquisition. Callers provide a [`Pool`], a [`PgCollectionSelection`]
//! and synchronous batch-admission and observation callbacks. Scheduling,
//! persistence and diagnostic formatting remain outside the library.
//!
//! [`log_discovery`] reads server log metadata using the same connection setup
//! and safe endpoint identities. Its timezone resolver keeps log parser types
//! in the caller, and `PgBouncer` retains its distinct Simple Query Protocol.
macro_rules! marked {
    () => {
        concat!(
            "/* kronika:",
            env!("CARGO_PKG_VERSION"),
            " ",
            file!(),
            " */ "
        )
    };
    ($sql:literal) => {
        concat!(
            "/* kronika:",
            env!("CARGO_PKG_VERSION"),
            " ",
            file!(),
            " */ ",
            $sql,
        )
    };
}

mod acquisition;
mod connection;
pub mod log_discovery;

pub use acquisition::{
    ConnectionObservation, PgBatch, PgCollectionSelection, PgCollector, PgObservation, PgWarning,
    QueryObservation, QueryOutcome,
};

pub mod activity;
pub mod archiver;
pub mod bgwriter;
pub mod checkpointer;
pub mod database;
pub mod databases;
pub mod extension;
pub mod io;
pub mod locks;
mod pool;
pub mod prepared_xacts;
pub mod progress_vacuum;
pub mod query;
pub mod settings;
pub mod statements;
pub mod statements_info;
pub mod store_plans;
pub mod store_plans_info;
pub mod transport;
pub mod user_indexes;
pub mod user_tables;
pub mod wal;
pub mod wal_storage;

pub use pool::{CONNECT_TIMEOUT, ConnectError, MAX_AGE, Pool};
pub use query::Session;
pub use transport::Transport;

fn intern_opt<E>(
    intern: &mut impl FnMut(&[u8]) -> Result<kronika_registry::StrId, E>,
    value: Option<&str>,
) -> Result<Option<kronika_registry::StrId>, E> {
    value.map(|text| intern(text.as_bytes())).transpose()
}

#[cfg(test)]
mod tests;
