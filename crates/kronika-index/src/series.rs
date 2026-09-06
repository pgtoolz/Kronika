//! The small presentation series allowed in an IDX.

use crate::decode::{i64_at, u32_at, u64_at};
use crate::detect::finding_layout;
use crate::file::IndexError;
use crate::findings::FindingBlock;

/// Stable kind stored in the IDX table of contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum SeriesKind {
    /// OS PSI health, including an unknown first recorded point.
    OsHealth = 1,
    /// OS health after subtracting every configured service penalty.
    OverallHealth = 2,
    /// `PostgreSQL` health derived from instantaneous active backends.
    PostgresHealth = 3,
    /// Per-database committed plus rolled-back transactions per second.
    PgTransactionsPerSecond = 4,
    /// `PostgreSQL` backends whose instantaneous state is `active`.
    PgActiveBackends = 5,
    /// Sparse source locators for one physical section.
    Findings = 6,
}

impl SeriesKind {
    pub(crate) const fn from_raw(raw: u32) -> Result<Self, IndexError> {
        match raw {
            1 => Ok(Self::OsHealth),
            2 => Ok(Self::OverallHealth),
            3 => Ok(Self::PostgresHealth),
            4 => Ok(Self::PgTransactionsPerSecond),
            5 => Ok(Self::PgActiveBackends),
            6 => Ok(Self::Findings),
            _ => Err(IndexError::BadLayout),
        }
    }
}

/// One independently addressable IDX block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeriesKey {
    /// Derived series carried by the block.
    pub kind: SeriesKind,
    /// Physical input layout, or zero for a derived OS series.
    pub type_id: u32,
}

impl SeriesKey {
    /// OS PSI health.
    pub const OS_HEALTH: Self = Self {
        kind: SeriesKind::OsHealth,
        type_id: 0,
    };
    /// Overall health after optional service penalties.
    pub const OVERALL_HEALTH: Self = Self {
        kind: SeriesKind::OverallHealth,
        type_id: 0,
    };
    /// `PostgreSQL` health.
    pub const POSTGRES_HEALTH: Self = Self {
        kind: SeriesKind::PostgresHealth,
        type_id: 0,
    };
}

/// One full-resolution OS health point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthPoint {
    /// Snapshot timestamp in unix microseconds.
    pub timestamp: i64,
    /// Derived percent, or `None` when this snapshot has no usable baseline.
    pub value: Option<u8>,
}

/// One per-database TPS point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransactionPoint {
    /// Snapshot timestamp in unix microseconds.
    pub timestamp: i64,
    /// `PostgreSQL` database OID; zero is the shared-objects row.
    pub datid: u32,
    /// Transactions per second since the preceding usable sample.
    pub value: Option<f64>,
}

/// One instantaneous active-backend count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveBackendPoint {
    /// Snapshot timestamp in unix microseconds.
    pub timestamp: i64,
    /// Rows whose recorded state is exactly `active`.
    pub count: u32,
}

/// One allowlisted presentation block.
#[derive(Debug, Clone, PartialEq)]
pub enum SeriesBlock {
    /// Full-resolution OS health points.
    OsHealth(Vec<HealthPoint>),
    /// Overall health points.
    OverallHealth(Vec<HealthPoint>),
    /// `PostgreSQL` health points; absent when collection is disabled.
    PostgresHealth(Vec<HealthPoint>),
    /// Full-resolution per-database TPS points from one physical layout.
    PgTransactions {
        /// Physical `pg_stat_database` layout.
        type_id: u32,
        /// Points ordered by database OID and timestamp.
        points: Vec<TransactionPoint>,
    },
    /// Full-resolution active-backend counts from one physical layout.
    PgActiveBackends {
        /// Physical `pg_stat_activity` layout.
        type_id: u32,
        /// Points ordered by timestamp.
        points: Vec<ActiveBackendPoint>,
    },
    /// Sparse locators into one physical section.
    Findings(FindingBlock),
}

impl SeriesBlock {
    /// Exact TOC identity of this block.
    #[must_use]
    pub const fn key(&self) -> SeriesKey {
        match self {
            Self::OsHealth(_) => SeriesKey::OS_HEALTH,
            Self::OverallHealth(_) => SeriesKey::OVERALL_HEALTH,
            Self::PostgresHealth(_) => SeriesKey::POSTGRES_HEALTH,
            Self::PgTransactions { type_id, .. } => SeriesKey {
                kind: SeriesKind::PgTransactionsPerSecond,
                type_id: *type_id,
            },
            Self::PgActiveBackends { type_id, .. } => SeriesKey {
                kind: SeriesKind::PgActiveBackends,
                type_id: *type_id,
            },
            Self::Findings(block) => SeriesKey {
                kind: SeriesKind::Findings,
                type_id: block.type_id,
            },
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, IndexError> {
        match self {
            Self::OsHealth(points) | Self::OverallHealth(points) | Self::PostgresHealth(points) => {
                encode_health(points)
            }
            Self::PgTransactions { points, .. } => encode_transactions(points),
            Self::PgActiveBackends { points, .. } => encode_active(points),
            Self::Findings(block) => block.encode(),
        }
    }

    pub(crate) fn decode(key: SeriesKey, bytes: &[u8]) -> Result<Self, IndexError> {
        match key.kind {
            SeriesKind::OsHealth if key.type_id == 0 => decode_health(bytes).map(Self::OsHealth),
            SeriesKind::OverallHealth if key.type_id == 0 => {
                decode_health(bytes).map(Self::OverallHealth)
            }
            SeriesKind::PostgresHealth if key.type_id == 0 => {
                decode_health(bytes).map(Self::PostgresHealth)
            }
            SeriesKind::PgTransactionsPerSecond if pg_database_layout(key.type_id) => {
                Ok(Self::PgTransactions {
                    type_id: key.type_id,
                    points: decode_transactions(bytes)?,
                })
            }
            SeriesKind::PgActiveBackends if pg_activity_layout(key.type_id) => {
                Ok(Self::PgActiveBackends {
                    type_id: key.type_id,
                    points: decode_active(bytes)?,
                })
            }
            SeriesKind::Findings if key.type_id == 0 || finding_layout(key.type_id) => {
                FindingBlock::decode(key.type_id, bytes).map(Self::Findings)
            }
            _ => Err(IndexError::BadLayout),
        }
    }
}

pub(crate) const fn pg_database_layout(type_id: u32) -> bool {
    matches!(type_id, 1_005_001..=1_005_004)
}

pub(crate) const fn pg_activity_layout(type_id: u32) -> bool {
    matches!(type_id, 1_001_001 | 1_001_002 | 1_001_004)
}

fn encode_health(points: &[HealthPoint]) -> Result<Vec<u8>, IndexError> {
    let mut out = with_count(points.len(), 10)?;
    let mut previous = None;
    for point in points {
        if previous.is_some_and(|timestamp| timestamp > point.timestamp)
            || point.value.is_some_and(|value| value > 100)
        {
            return Err(IndexError::BadLayout);
        }
        previous = Some(point.timestamp);
        out.extend_from_slice(&point.timestamp.to_le_bytes());
        out.push(u8::from(point.value.is_some()));
        out.push(point.value.unwrap_or(0));
    }
    Ok(out)
}

fn decode_health(bytes: &[u8]) -> Result<Vec<HealthPoint>, IndexError> {
    let (count, body) = count_and_body(bytes, 10)?;
    let mut points = Vec::with_capacity(count);
    let mut previous = None;
    for raw in body.chunks_exact(10) {
        let timestamp = i64_at(raw, 0)?;
        let value = match (raw[8], raw[9]) {
            (0, 0) => None,
            (1, value @ 0..=100) => Some(value),
            _ => return Err(IndexError::BadLayout),
        };
        if previous.is_some_and(|before| before > timestamp) {
            return Err(IndexError::BadLayout);
        }
        previous = Some(timestamp);
        points.push(HealthPoint { timestamp, value });
    }
    Ok(points)
}

fn encode_transactions(points: &[TransactionPoint]) -> Result<Vec<u8>, IndexError> {
    let mut out = with_count(points.len(), 21)?;
    let mut previous = None;
    for point in points {
        let key = (point.datid, point.timestamp);
        if previous.is_some_and(|before| before > key)
            || point
                .value
                .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            return Err(IndexError::BadLayout);
        }
        previous = Some(key);
        out.extend_from_slice(&point.timestamp.to_le_bytes());
        out.extend_from_slice(&point.datid.to_le_bytes());
        out.push(u8::from(point.value.is_some()));
        out.extend_from_slice(&point.value.unwrap_or(0.0).to_bits().to_le_bytes());
    }
    Ok(out)
}

fn decode_transactions(bytes: &[u8]) -> Result<Vec<TransactionPoint>, IndexError> {
    let (count, body) = count_and_body(bytes, 21)?;
    let mut points = Vec::with_capacity(count);
    let mut previous = None;
    for raw in body.chunks_exact(21) {
        let timestamp = i64_at(raw, 0)?;
        let datid = u32_at(raw, 8)?;
        let raw_value = f64::from_bits(u64_at(raw, 13)?);
        let value = match raw[12] {
            0 if raw_value == 0.0 => None,
            1 if raw_value.is_finite() && raw_value >= 0.0 => Some(raw_value),
            _ => return Err(IndexError::BadLayout),
        };
        let key = (datid, timestamp);
        if previous.is_some_and(|before| before > key) {
            return Err(IndexError::BadLayout);
        }
        previous = Some(key);
        points.push(TransactionPoint {
            timestamp,
            datid,
            value,
        });
    }
    Ok(points)
}

fn encode_active(points: &[ActiveBackendPoint]) -> Result<Vec<u8>, IndexError> {
    let mut out = with_count(points.len(), 12)?;
    let mut previous = None;
    for point in points {
        if previous.is_some_and(|timestamp| timestamp > point.timestamp) {
            return Err(IndexError::BadLayout);
        }
        previous = Some(point.timestamp);
        out.extend_from_slice(&point.timestamp.to_le_bytes());
        out.extend_from_slice(&point.count.to_le_bytes());
    }
    Ok(out)
}

fn decode_active(bytes: &[u8]) -> Result<Vec<ActiveBackendPoint>, IndexError> {
    let (count, body) = count_and_body(bytes, 12)?;
    let mut points = Vec::with_capacity(count);
    let mut previous = None;
    for raw in body.chunks_exact(12) {
        let timestamp = i64_at(raw, 0)?;
        if previous.is_some_and(|before| before > timestamp) {
            return Err(IndexError::BadLayout);
        }
        previous = Some(timestamp);
        points.push(ActiveBackendPoint {
            timestamp,
            count: u32_at(raw, 8)?,
        });
    }
    Ok(points)
}

fn with_count(count: usize, point_len: usize) -> Result<Vec<u8>, IndexError> {
    let count = u32::try_from(count).map_err(|_overflow| IndexError::TooLarge)?;
    let capacity = 4_usize
        .checked_add(
            usize::try_from(count)
                .map_err(|_overflow| IndexError::TooLarge)?
                .checked_mul(point_len)
                .ok_or(IndexError::TooLarge)?,
        )
        .ok_or(IndexError::TooLarge)?;
    let mut out = Vec::with_capacity(capacity);
    out.extend_from_slice(&count.to_le_bytes());
    Ok(out)
}

fn count_and_body(bytes: &[u8], point_len: usize) -> Result<(usize, &[u8]), IndexError> {
    let count = usize::try_from(u32_at(bytes, 0)?).map_err(|_overflow| IndexError::TooLarge)?;
    let expected = 4_usize
        .checked_add(count.checked_mul(point_len).ok_or(IndexError::TooLarge)?)
        .ok_or(IndexError::TooLarge)?;
    if bytes.len() != expected {
        return Err(IndexError::BadLayout);
    }
    Ok((count, &bytes[4..]))
}

#[cfg(test)]
mod tests;
