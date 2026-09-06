//! Validated controls for the bounded system workload.

use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};

pub(super) const ENABLED_ENV: &str = "KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED";
pub(super) const DIRECTORY_ENV: &str = "KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR";
pub(super) const CPU_ENV: &str = "KRONIKA_DEMO_SYSTEM_CPU_PERCENT";
pub(super) const MEMORY_ENV: &str = "KRONIKA_DEMO_SYSTEM_MEMORY_MIB";
pub(super) const FILE_ENV: &str = "KRONIKA_DEMO_SYSTEM_FILE_MIB";
pub(super) const DISK_RATE_ENV: &str = "KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S";
pub(super) const NETWORK_RATE_ENV: &str = "KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S";
pub(super) const FLUSH_ENV: &str = "KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S";

const DEFAULT_CPU_PERCENT: u64 = 12;
const DEFAULT_MEMORY_MIB: u64 = 32;
const DEFAULT_FILE_MIB: u64 = 8;
const DEFAULT_DISK_KIB_PER_S: u64 = 32;
const DEFAULT_NETWORK_KIB_PER_S: u64 = 32;
const DEFAULT_FLUSH_INTERVAL_S: u64 = 5;

const MIB: u64 = 1024 * 1024;
const KIB: u64 = 1024;

/// Resource limits and paths for one system-workload run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemActivityConfig {
    pub(super) directory: PathBuf,
    pub(super) storage_directory: PathBuf,
    pub(super) cpu_percent: u64,
    pub(super) memory_mib: u64,
    pub(super) file_mib: u64,
    pub(super) disk_kib_per_s: u64,
    pub(super) network_kib_per_s: u64,
    pub(super) flush_interval_s: u64,
}

impl SystemActivityConfig {
    /// Read the environment. `false` disables the complete system workload.
    pub(crate) fn from_env(root: &Path, storage_directory: &Path) -> Result<Option<Self>> {
        Self::from_lookup(root, storage_directory, |key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_value)) => {
                anyhow::bail!("{key} is not valid UTF-8")
            }
        })
    }

    fn from_lookup<F>(root: &Path, storage_directory: &Path, mut lookup: F) -> Result<Option<Self>>
    where
        F: FnMut(&str) -> Result<Option<String>>,
    {
        let enabled = match lookup(ENABLED_ENV)? {
            None => true,
            Some(raw) if raw == "true" => true,
            Some(raw) if raw == "false" => false,
            Some(raw) => anyhow::bail!("{ENABLED_ENV}={raw:?} is not true or false"),
        };
        if !enabled {
            return Ok(None);
        }

        let directory = match lookup(DIRECTORY_ENV)? {
            None => root.join("system-activity"),
            Some(raw) => {
                anyhow::ensure!(!raw.trim().is_empty(), "{DIRECTORY_ENV} must not be blank");
                PathBuf::from(raw)
            }
        };
        let config = Self {
            directory,
            storage_directory: storage_directory.to_owned(),
            cpu_percent: bounded(&mut lookup, CPU_ENV, DEFAULT_CPU_PERCENT, 1, 25)?,
            memory_mib: bounded(&mut lookup, MEMORY_ENV, DEFAULT_MEMORY_MIB, 8, 128)?,
            file_mib: bounded(&mut lookup, FILE_ENV, DEFAULT_FILE_MIB, 1, 32)?,
            disk_kib_per_s: bounded(&mut lookup, DISK_RATE_ENV, DEFAULT_DISK_KIB_PER_S, 1, 256)?,
            network_kib_per_s: bounded(
                &mut lookup,
                NETWORK_RATE_ENV,
                DEFAULT_NETWORK_KIB_PER_S,
                1,
                256,
            )?,
            flush_interval_s: bounded(&mut lookup, FLUSH_ENV, DEFAULT_FLUSH_INTERVAL_S, 1, 10)?,
        };
        config.validate_paths()?;
        let bytes_per_flush = config
            .disk_kib_per_s
            .checked_mul(KIB)
            .and_then(|rate| rate.checked_mul(config.flush_interval_s))
            .context("the configured disk rate and flush interval overflow u64")?;
        anyhow::ensure!(
            bytes_per_flush <= config.file_bytes()?,
            "{DISK_RATE_ENV} times {FLUSH_ENV} must not exceed {FILE_ENV}"
        );
        Ok(Some(config))
    }

    pub(super) fn memory_bytes(&self) -> Result<usize> {
        bytes_from_mib(self.memory_mib, MEMORY_ENV)
    }

    pub(super) fn file_bytes(&self) -> Result<u64> {
        self.file_mib
            .checked_mul(MIB)
            .with_context(|| format!("{FILE_ENV} overflows bytes"))
    }

    fn validate_paths(&self) -> Result<()> {
        let directory = normalized_absolute(&self.directory)?;
        let storage = normalized_absolute(&self.storage_directory)?;
        anyhow::ensure!(
            paths_are_separate(&directory, &storage),
            "{DIRECTORY_ENV} must be separate from KRONIKA_STORAGE_DIR"
        );
        Ok(())
    }
}

fn bounded<F>(lookup: &mut F, key: &str, default: u64, minimum: u64, maximum: u64) -> Result<u64>
where
    F: FnMut(&str) -> Result<Option<String>>,
{
    let Some(raw) = lookup(key)? else {
        return Ok(default);
    };
    let value: u64 = raw
        .parse()
        .with_context(|| format!("{key}={raw:?} is not a u64"))?;
    anyhow::ensure!(
        (minimum..=maximum).contains(&value),
        "{key} must be between {minimum} and {maximum}"
    );
    Ok(value)
}

fn bytes_from_mib(value: u64, key: &str) -> Result<usize> {
    let bytes = value
        .checked_mul(MIB)
        .with_context(|| format!("{key} overflows bytes"))?;
    usize::try_from(bytes).with_context(|| format!("{key} does not fit usize"))
}

fn normalized_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .context("read the current directory")?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

pub(super) fn paths_are_separate(left: &Path, right: &Path) -> bool {
    !left.starts_with(right) && !right.starts_with(left)
}

#[cfg(test)]
mod tests;
