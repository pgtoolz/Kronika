//! Wall-clock timestamps for source rows and successive collection windows.

use anyhow::{Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::logging::{LogLevel, field, log_event};

pub(crate) fn collection_timestamp() -> Option<i64> {
    match unix_now_us() {
        Ok(ts) => Some(ts),
        Err(err) => {
            log_event(
                LogLevel::Error,
                "collection_failed",
                &[field("reason", "clock"), field("error", format!("{err:#}"))],
            );
            None
        }
    }
}

pub(crate) fn collection_timestamp_after(previous: Option<i64>) -> Option<i64> {
    let now = collection_timestamp()?;
    previous.map_or(Some(now), |previous| {
        previous.checked_add(1).map(|next| now.max(next))
    })
}

pub(crate) fn unix_now_us() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?;
    i64::try_from(duration.as_micros()).context("system time exceeds i64 microseconds")
}
