//! Validated controls for the bounded system workload.

use crate::config::{integer, option, seconds, size_units, text};
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
    pub(crate) fn from_matches(
        matches: &clap::ArgMatches,
        root: &Path,
        storage_directory: &Path,
    ) -> Result<Option<Self>> {
        let enabled = match text(matches, ENABLED_ENV)? {
            "true" => true,
            "false" => false,
            _ => anyhow::bail!("{ENABLED_ENV} must be true or false"),
        };
        if !enabled {
            return Ok(None);
        }
        let directory = if matches.contains_id(DIRECTORY_ENV) {
            let raw = text(matches, DIRECTORY_ENV)?;
            anyhow::ensure!(!raw.trim().is_empty(), "{DIRECTORY_ENV} must not be blank");
            PathBuf::from(raw)
        } else {
            root.join("system-activity")
        };
        let config = Self {
            directory,
            storage_directory: storage_directory.to_owned(),
            cpu_percent: integer(matches, CPU_ENV)?,
            memory_mib: size_units(matches, MEMORY_ENV, MIB)?,
            file_mib: size_units(matches, FILE_ENV, MIB)?,
            disk_kib_per_s: size_units(matches, DISK_RATE_ENV, KIB)?,
            network_kib_per_s: size_units(matches, NETWORK_RATE_ENV, KIB)?,
            flush_interval_s: seconds(matches, FLUSH_ENV)?,
        };
        for (key, value, minimum, maximum) in [
            (CPU_ENV, config.cpu_percent, 1, 25),
            (MEMORY_ENV, config.memory_mib, 8, 128),
            (FILE_ENV, config.file_mib, 1, 32),
            (DISK_RATE_ENV, config.disk_kib_per_s, 1, 256),
            (NETWORK_RATE_ENV, config.network_kib_per_s, 1, 256),
            (FLUSH_ENV, config.flush_interval_s, 1, 10),
        ] {
            anyhow::ensure!(
                (minimum..=maximum).contains(&value),
                "{key} must be between {minimum} and {maximum}"
            );
        }
        config.validate_paths()?;
        // These products cannot overflow after the hard resource bounds above.
        let bytes_per_flush = config.disk_kib_per_s * KIB * config.flush_interval_s;
        anyhow::ensure!(
            bytes_per_flush <= config.file_bytes()?,
            "{DISK_RATE_ENV} times {FLUSH_ENV} must not exceed {FILE_ENV}"
        );
        Ok(Some(config))
    }

    pub(super) fn memory_bytes(&self) -> Result<usize> {
        let bytes = self
            .memory_mib
            .checked_mul(MIB)
            .with_context(|| format!("{MEMORY_ENV} overflows bytes"))?;
        usize::try_from(bytes).with_context(|| format!("{MEMORY_ENV} does not fit usize"))
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

pub(crate) fn args() -> impl Iterator<Item = clap::Arg> {
    [
        option(
            "system-workload-enabled",
            ENABLED_ENV,
            Some("true"),
            "Enable bounded CPU, memory, disk, and loopback activity: true or false",
        ),
        option(
            "system-workload-dir",
            DIRECTORY_ENV,
            None,
            "Scratch directory separate from storage [default: <dir>/system-activity]",
        )
        .value_name("DIR"),
        option(
            "system-cpu-percent",
            CPU_ENV,
            Some("12"),
            "Peak percentage of one CPU core (1..25)",
        ),
        option(
            "system-memory-mib",
            MEMORY_ENV,
            Some("32"),
            "Anonymous memory in MiB (8..128), or an explicit byte size",
        ),
        option(
            "system-file-mib",
            FILE_ENV,
            Some("8"),
            "Fixed scratch-file size in MiB (1..32), or an explicit byte size",
        ),
        option(
            "system-disk-kib-per-s",
            DISK_RATE_ENV,
            Some("32"),
            "Peak read and write rate in KiB/s (1..256)",
        ),
        option(
            "system-network-kib-per-s",
            NETWORK_RATE_ENV,
            Some("32"),
            "Peak loopback payload rate per direction in KiB/s (1..256)",
        ),
        option(
            "system-flush-interval-s",
            FLUSH_ENV,
            Some("5"),
            "Scratch-file flush interval in seconds (1..10)",
        ),
    ]
    .into_iter()
    .map(|arg| arg.help_heading("System workload"))
}

#[cfg(test)]
#[path = "../tests/system_activity/config.rs"]
mod tests;
