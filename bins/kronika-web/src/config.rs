//! CLI arguments and environment fallbacks, validated before runtime startup.

use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum};
use tokio::sync::Semaphore;

pub(crate) use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};
// --sources describes configured families in the catalog; it does not filter recorded data.
const SUPPORTED_SOURCES: u32 = SOURCE_OS | SOURCE_POSTGRESQL;

/// The validated server contract.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    /// Data root containing journal, segments, and derived indexes.
    pub(crate) data_root: PathBuf,
    /// Address to listen on.
    pub(crate) listen: SocketAddr,
    /// Optional account enabling browser, API, and MCP authentication.
    pub(crate) account: Option<Account>,
    /// Source-family configuration reported by the catalog.
    pub(crate) sources: u32,
    /// Label this recording as synthetic demo data in the catalog.
    pub(crate) synthetic_demo: bool,
    /// Process-wide admission for standalone export preparation.
    pub(crate) export_gate: Arc<Semaphore>,
}

/// Credentials accepted directly and used to derive browser sessions.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Account {
    /// User name.
    pub(crate) user: String,
    /// Password.
    pub(crate) password: String,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Account { credentials: [redacted] }")
    }
}

/// Browse a Kronika recording and serve its HTTP API and MCP tools.
#[derive(Parser)]
#[command(name = "kronika-web", version, after_long_help = crate::help::EXAMPLES)]
struct Args {
    /// Existing collector recording directory; needs read/write access for indexes.
    #[arg(
        long,
        env = "KRONIKA_STORAGE_DIR",
        value_name = "DIR",
        hide_env_values = true
    )]
    storage_dir: PathBuf,
    /// IP address and port for plain HTTP; hostnames are not accepted.
    #[arg(
        long,
        env = "KRONIKA_WEB_LISTEN",
        default_value = "127.0.0.1:8080",
        value_name = "IP:PORT",
        hide_env_values = true
    )]
    listen: SocketAddr,
    /// Configured sources: none, os, postgresql, all (legacy bitsets 0..3 also work).
    #[arg(long, env = "KRONIKA_WEB_SOURCES", default_value = "all", value_name = "SOURCES", value_parser = source_set, hide_env_values = true)]
    sources: u32,
    /// Authentication user; set together with --password, or leave both unset.
    #[arg(long, env = "KRONIKA_WEB_USER", hide_env_values = true)]
    user: Option<String>,
    /// Authentication password; set together with --user, or leave both unset.
    #[arg(long, env = "KRONIKA_WEB_PASSWORD", hide_env_values = true)]
    password: Option<String>,
    /// Mark the recording as generated demo data.
    #[arg(long, env = "KRONIKA_WEB_DEMO", value_enum, hide_env_values = true)]
    demo: Option<Demo>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Demo {
    Synthetic,
}

/// Parse and validate without starting runtime threads or touching storage/network.
pub(crate) fn parse_from(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
) -> Result<Config, clap::Error> {
    let mut command = Args::command();
    let matches = command.try_get_matches_from_mut(args)?;
    let args = Args::from_arg_matches(&matches)?;
    let account = account(args.user, args.password).map_err(|error| {
        command.error(clap::error::ErrorKind::ValueValidation, error.to_string())
    })?;
    Ok(Config {
        data_root: args.storage_dir,
        listen: args.listen,
        account,
        sources: args.sources,
        synthetic_demo: args.demo.is_some(),
        export_gate: Arc::new(Semaphore::new(1)),
    })
}

fn account(user: Option<String>, password: Option<String>) -> Result<Option<Account>> {
    if user.is_none() && password.is_none() {
        return Ok(None);
    }
    let user = user.context(
        "--user / KRONIKA_WEB_USER is not set; set both credentials or leave both unset",
    )?;
    let password = password.context(
        "--password / KRONIKA_WEB_PASSWORD is not set; set both credentials or leave both unset",
    )?;
    if user.is_empty() {
        anyhow::bail!("--user / KRONIKA_WEB_USER is empty");
    }
    if password.is_empty() {
        anyhow::bail!("--password / KRONIKA_WEB_PASSWORD is empty");
    }
    Ok(Some(Account { user, password }))
}

fn source_set(raw: &str) -> Result<u32, String> {
    let sources = match raw {
        "none" => 0,
        "os" => SOURCE_OS,
        "postgresql" => SOURCE_POSTGRESQL,
        "all" => SUPPORTED_SOURCES,
        value => value.parse::<u32>().map_err(|_error| {
            "expected none, os, postgresql, all, or a source bitset 0..3".to_owned()
        })?,
    };
    if sources & !SUPPORTED_SOURCES != 0 {
        return Err("source bitset must contain only OS (1) and PostgreSQL (2) bits".to_owned());
    }
    Ok(sources)
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
