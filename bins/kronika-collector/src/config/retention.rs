//! Parse and validate the storage-rotation target in `KRONIKA_RETENTION`.

use anyhow::{Context, Result};

use crate::logging::{LogLevel, field, log_event};

/// Used-fraction target of the `auto` mode when no percentage is given.
const DEFAULT_AUTO_PERCENT: u8 = 80;
/// Fixed rotation target when `KRONIKA_RETENTION` is unset.
const DEFAULT_RETENTION_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Rotation target for the whole `KRONIKA_STORAGE_DIR` tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    variant_size_differences,
    reason = "a 16-byte Copy enum; the byte-budget and percentage arms cannot share a width"
)]
pub(crate) enum RetentionConfig {
    /// Keep the tree at or below this many bytes.
    Fixed(u64),
    /// Keep the backing partition's used fraction at or below this percentage.
    Auto(u8),
}

impl RetentionConfig {
    /// Read, validate and report the storage target before source configuration.
    pub(super) fn from_env(segment_max_bytes: u64) -> Result<Self> {
        let retention = std::env::var("KRONIKA_RETENTION")
            .ok()
            .map(|raw| Self::parse(&raw))
            .transpose()?
            .unwrap_or(Self::Fixed(DEFAULT_RETENTION_BYTES));
        retention.validate(segment_max_bytes)?;
        match retention {
            Self::Fixed(budget) => log_event(
                LogLevel::Info,
                "config_retention",
                &[field("mode", "fixed"), field("budget_bytes", budget)],
            ),
            Self::Auto(percent) => log_event(
                LogLevel::Info,
                "config_retention",
                &[
                    field("mode", "auto"),
                    field("used_percent", u64::from(percent)),
                ],
            ),
        }
        Ok(retention)
    }

    /// Parses `KRONIKA_RETENTION` into a rotation target.
    ///
    /// Accepts a raw byte budget (`<u64>`), `auto` (equivalent to `auto:80`), or
    /// `auto:<P>` with `P` in `1..=99`.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty value, a non-numeric budget, an out-of-range
    /// percentage, or an unrecognized `auto` suffix.
    fn parse(raw: &str) -> Result<Self> {
        let value = raw.trim();
        anyhow::ensure!(!value.is_empty(), "KRONIKA_RETENTION must not be empty");
        if let Some(suffix) = value.strip_prefix("auto") {
            let percent = if suffix.is_empty() {
                DEFAULT_AUTO_PERCENT
            } else {
                let digits = suffix.strip_prefix(':').with_context(|| {
                    format!("KRONIKA_RETENTION must be 'auto' or 'auto:P', got {value:?}")
                })?;
                digits.parse::<u8>().with_context(|| {
                    format!("KRONIKA_RETENTION percentage is not a u8: {digits:?}")
                })?
            };
            anyhow::ensure!(
                (1..=99).contains(&percent),
                "KRONIKA_RETENTION auto percentage must be in 1..=99, got {percent}"
            );
            return Ok(Self::Auto(percent));
        }
        let budget = value.parse::<u64>().with_context(|| {
            format!("KRONIKA_RETENTION must be a byte budget or 'auto[:P]', got {value:?}")
        })?;
        Ok(Self::Fixed(budget))
    }

    /// Rejects a fixed budget that cannot hold the non-deletable minimum plus room
    /// to rotate.
    ///
    /// The floor is `2 × KRONIKA_SEGMENT_MAX_BYTES`: the active journal and the
    /// newest finished segment are never deleted, so a smaller budget could never
    /// converge. `auto` targets a live partition fraction and has no such bound.
    ///
    /// # Errors
    ///
    /// Returns an error naming the budget and the required floor.
    fn validate(self, segment_max_bytes: u64) -> Result<()> {
        if let Self::Fixed(budget) = self {
            let floor = segment_max_bytes.saturating_mul(2);
            anyhow::ensure!(
                budget >= floor,
                "KRONIKA_RETENTION fixed budget {budget} is below 2 × KRONIKA_SEGMENT_MAX_BYTES \
                 ({floor}); a budget that cannot hold two segments cannot converge"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/config/retention.rs"]
mod tests;
