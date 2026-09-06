//! Collecting metrics from a `PostgreSQL` server.
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
pub mod user_indexes;
pub mod user_tables;
pub mod wal;
pub mod wal_storage;

pub use pool::{CONNECT_TIMEOUT, ConnectError, MAX_AGE, Pool};
pub use query::Session;

fn intern_opt<E>(
    intern: &mut impl FnMut(&[u8]) -> Result<kronika_registry::StrId, E>,
    value: Option<&str>,
) -> Result<Option<kronika_registry::StrId>, E> {
    value.map(|text| intern(text.as_bytes())).transpose()
}

#[cfg(test)]
#[allow(
    clippy::unnecessary_wraps,
    reason = "matches the fallible interner signature used by row converters"
)]
fn test_intern(bytes: &[u8]) -> Result<kronika_registry::StrId, std::convert::Infallible> {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    Ok(kronika_registry::StrId(hash | 1))
}
