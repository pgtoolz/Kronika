//! Environment-only configuration, validated before the listener starts.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use tokio::sync::Semaphore;

const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

pub(crate) use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};
const SUPPORTED_SOURCES: u32 = SOURCE_OS | SOURCE_POSTGRESQL;

/// The validated server contract.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    /// Data root containing journal, segments, and derived indexes.
    pub(crate) data_root: PathBuf,
    /// Address to listen on.
    pub(crate) listen: SocketAddr,
    /// Required account used by centralized authentication.
    pub(crate) account: Account,
    /// Whether API and browser session authentication is enforced.
    pub(crate) authentication_required: bool,
    /// Source-family configuration reported by the catalog.
    pub(crate) sources: u32,
    /// Whether the server exposes the bundled synthetic demo dataset.
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

impl Config {
    /// Read and validate the environment contract.
    ///
    /// # Errors
    ///
    /// Returns an error when the data root, either credential, or the source
    /// bitset is absent, or when a configured value is invalid.
    pub(crate) fn from_env() -> Result<Self> {
        let data_root: PathBuf = std::env::var("KRONIKA_STORAGE_DIR")
            .context("KRONIKA_STORAGE_DIR is not set")?
            .into();
        let raw_listen =
            std::env::var("KRONIKA_WEB_LISTEN").unwrap_or_else(|_unset| DEFAULT_LISTEN.to_owned());
        let listen = raw_listen.parse().with_context(|| {
            format!("KRONIKA_WEB_LISTEN={raw_listen:?} is not an address and port")
        })?;
        let account = account(
            std::env::var("KRONIKA_WEB_USER").ok(),
            std::env::var("KRONIKA_WEB_PASSWORD").ok(),
        )?;
        let authentication_required =
            authentication_required(std::env::var("KRONIKA_WEB_AUTH").ok().as_deref())?;
        let sources = source_set(std::env::var("KRONIKA_WEB_SOURCES").ok())?;
        let synthetic_demo = synthetic_demo(std::env::var("KRONIKA_WEB_DEMO").ok().as_deref())?;
        Ok(Self {
            data_root,
            listen,
            account,
            authentication_required,
            sources,
            synthetic_demo,
            export_gate: Arc::new(Semaphore::new(1)),
        })
    }
}

fn synthetic_demo(raw: Option<&str>) -> Result<bool> {
    match raw {
        None => Ok(false),
        Some("synthetic") => Ok(true),
        Some(value) => anyhow::bail!("KRONIKA_WEB_DEMO={value:?} is not synthetic"),
    }
}

fn authentication_required(raw: Option<&str>) -> Result<bool> {
    match raw {
        None | Some("required") => Ok(true),
        Some("disabled") => Ok(false),
        Some(value) => anyhow::bail!("KRONIKA_WEB_AUTH={value:?} is not required or disabled"),
    }
}

fn account(user: Option<String>, password: Option<String>) -> Result<Account> {
    let user = user.context("KRONIKA_WEB_USER is not set")?;
    let password = password.context("KRONIKA_WEB_PASSWORD is not set")?;
    if user.is_empty() {
        anyhow::bail!("KRONIKA_WEB_USER is empty");
    }
    if password.is_empty() {
        anyhow::bail!("KRONIKA_WEB_PASSWORD is empty");
    }
    Ok(Account { user, password })
}

fn source_set(raw: Option<String>) -> Result<u32> {
    let raw = raw.context("KRONIKA_WEB_SOURCES is not set")?;
    let sources = raw
        .parse::<u32>()
        .with_context(|| format!("KRONIKA_WEB_SOURCES={raw:?} is not a u32 bitset"))?;
    if sources & !SUPPORTED_SOURCES != 0 {
        anyhow::bail!("KRONIKA_WEB_SOURCES={raw:?} contains unsupported source bits");
    }
    Ok(sources)
}

#[cfg(test)]
mod tests;
