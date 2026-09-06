//! The type contract: schema, column classes, sort key, and collection
//! semantics attached to every `type_id` (README.md, "Type Contract").
//!
//! A section codec must match its contract; codec tests check that directly.
//! The registry linter checks rules that span contracts.

use std::fmt;

use crate::TypeId;

/// The role a column plays for the diff and chart machinery
/// (README.md, "Column Classes").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnClass {
    /// Cumulative: a monotonic counter; rates are deltas over time.
    Cumulative,
    /// Gauge: an instantaneous value.
    Gauge,
    /// Label: identity or an attribute of the entity.
    Label,
    /// Timestamp: `i64` unix microseconds.
    Timestamp,
}

/// The on-disk type of a column value.
///
/// The set is the base types of the registry: a column uses the narrowest
/// type that fits its source so the section stays small (a `pid` is `I32`,
/// not `I64`). `Ts` is an `i64` unix-microsecond timestamp; `StrId` is a `u64`
/// reference into the segment string dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    /// Signed 8-bit integer.
    I8,
    /// Signed 16-bit integer.
    I16,
    /// Signed 32-bit integer.
    I32,
    /// Signed 64-bit integer.
    I64,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 32-bit integer.
    U32,
    /// Unsigned 64-bit integer.
    U64,
    /// 32-bit float.
    F32,
    /// 64-bit float.
    F64,
    /// Boolean.
    Bool,
    /// Timestamp, `i64` unix microseconds.
    Ts,
    /// A `u64` reference into the segment string dictionary (the bytes live in
    /// the dictionary, not the section).
    StrId,
    /// A list of `i32` (Arrow `List<Int32>`); an empty list is not NULL.
    ListI32,
}

impl ColumnType {
    /// The code the API and generated interface registry use for this type.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::Ts => "timestamp_us",
            Self::StrId => "dictionary_value",
            Self::ListI32 => "list_i32",
        }
    }
}

/// A unix-microseconds timestamp, as carried in a `Ts` column.
///
/// A distinct type so a timestamp column is `Ts`, not a bare `i64`, whatever
/// its [`ColumnClass`] — the collection time is class [`ColumnClass::Timestamp`],
/// but `postmaster_start_time` and friends are `Ts` gauges.
///
/// `#[repr(transparent)]` guarantees the same ABI as `i64`, so a future column
/// build can `cast` a `&[Ts]` to `&[i64]` instead of mapping `.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Ts(pub i64);

/// A reference into the segment string dictionary, as carried in a `StrId`
/// column. The dictionary (`kronika-format`) holds the bytes; the section
/// stores only this id.
///
/// `#[repr(transparent)]` guarantees the same ABI as `u64` (see [`Ts`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct StrId(pub u64);

/// What a column's numbers count.
///
/// Declared per column so a reader never has to infer the unit from the name
/// or a doc comment. `TypeContract` is compile-time data and never reaches a
/// segment, so this costs nothing on disk and does not affect any `type_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A bare number of occurrences with no dimension of its own.
    None,
    /// A count of entities (processes, threads, handles, huge pages).
    Count,
    /// Bytes.
    Bytes,
    /// Kibibytes, as the kernel prints them in `/proc/meminfo` and friends.
    Kib,
    /// Memory pages, of `instance_metadata.page_size_bytes` each.
    Pages,
    /// 512-byte disk sectors, as `/proc/diskstats` reports them.
    Sectors,
    /// Seconds.
    Seconds,
    /// Milliseconds.
    Milliseconds,
    /// Microseconds.
    Microseconds,
    /// Nanoseconds.
    Nanoseconds,
    /// Scheduler ticks, of `instance_metadata.clock_ticks_per_sec` per second.
    Jiffies,
    /// Hertz.
    Hertz,
    /// Megabits per second.
    MegabitsPerSecond,
    /// Megabytes per second, as `PostgreSQL` prints autovacuum I/O rates.
    MegabytesPerSecond,
    /// A percentage, 0 to 100.
    Percent,
    /// Degrees Celsius.
    Celsius,
}

impl Unit {
    /// The code the API sends for this unit.
    ///
    /// A code, not a word: the interface holds the words for both languages it
    /// ships, and the server holds none of them.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Count => "count",
            Self::Bytes => "bytes",
            Self::Kib => "kibibytes",
            Self::Pages => "pages",
            Self::Sectors => "sectors",
            Self::Seconds => "seconds",
            Self::Milliseconds => "milliseconds",
            Self::Microseconds => "microseconds",
            Self::Nanoseconds => "nanoseconds",
            Self::Jiffies => "jiffies",
            Self::Hertz => "hertz",
            Self::MegabitsPerSecond => "megabits_per_second",
            Self::MegabytesPerSecond => "megabytes_per_second",
            Self::Percent => "percent",
            Self::Celsius => "celsius",
        }
    }
}

impl ColumnClass {
    /// The code the API sends for this class.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cumulative => "cumulative",
            Self::Gauge => "gauge",
            Self::Label => "label",
            Self::Timestamp => "timestamp",
        }
    }
}

/// One column of a typed section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// Column name, matching the registry schema and the codec.
    pub name: &'static str,
    /// On-disk value type.
    pub ty: ColumnType,
    /// The column's role.
    pub class: ColumnClass,
    /// Whether the column may be `NULL`.
    pub nullable: bool,
    /// What the column's numbers count. Required on a counter or gauge,
    /// absent on a label or timestamp.
    pub unit: Option<Unit>,
}

/// How a source is collected (README.md, "Collection Semantics").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantics {
    /// Every regular collection writes all rows of the source.
    SnapshotFull,
    /// A full snapshot, written only on the ticks where a collection
    /// condition holds.
    ConditionalFull,
    /// All events seen in the segment interval.
    EventStream,
    /// Only rows whose cumulative counters changed, plus a baseline.
    Changed,
    /// Written on change or after a periodic refresh interval.
    OnChange,
}

impl Semantics {
    /// The code generated interface metadata uses for collection semantics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SnapshotFull => "snapshot_full",
            Self::ConditionalFull => "conditional_full",
            Self::EventStream => "event_stream",
            Self::Changed => "changed",
            Self::OnChange => "on_change",
        }
    }
}

/// The full contract of one `type_id`.
///
/// A plain public record with `pub` fields — a caller can build one directly
/// (for a test, or to feed [`lint`]). Only the [`TypeId`] is valid by
/// construction (its constructor is crate-private); the rest of a contract
/// (sort key names match columns, a `Changed` type has `is_baseline`, …) is
/// checked by [`lint`], not the type system, so a hand-built contract can be
/// internally inconsistent until it passes the linter. The registry table is
/// [`registry`](crate::registry).
#[derive(Debug, Clone, Copy)]
pub struct TypeContract {
    /// The type id this contract describes.
    pub type_id: TypeId,
    /// Human-readable source name, e.g. the originating `/proc` file or view.
    pub name: &'static str,
    /// Collection semantics.
    pub semantics: Semantics,
    /// Columns in schema order.
    pub columns: &'static [Column],
    /// Sort-key column names, in order. Every name must be a column.
    pub sort_key: &'static [&'static str],
    /// Identity columns for the diff layer: the tuple that identifies one series
    /// across snapshots. Every name must be a `Label` column. Empty until the
    /// section is wired for diffing.
    pub identity: &'static [&'static str],
    /// Whether the type is retired (kept in the registry, no longer written).
    pub deprecated: bool,
}

impl TypeContract {
    /// Find a column by name.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }
}

/// Registry lint error (README.md, "Registry Linter").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintError {
    /// Two contracts share a type id.
    DuplicateTypeId {
        /// The repeated raw id.
        type_id: u32,
    },
    /// A sort-key name is not a column of the type.
    SortKeyColumnMissing {
        /// The raw id whose sort key failed validation.
        type_id: u32,
        /// The sort-key name with no matching column.
        column: &'static str,
    },
    /// A `Changed` type lacks the required `is_baseline` column.
    MissingBaseline {
        /// The raw id whose `Changed` contract lacks a baseline.
        type_id: u32,
    },
    /// A column is marked [`ColumnClass::Timestamp`] but is not the
    /// non-nullable `ts` required by readers.
    BadTimestampColumn {
        /// The raw id whose timestamp column failed validation.
        type_id: u32,
        /// The column that failed validation.
        column: &'static str,
    },
    /// An identity name is not a column of the type.
    IdentityColumnMissing {
        /// The raw id whose identity failed validation.
        type_id: u32,
        /// The identity name with no matching column.
        column: &'static str,
    },
    /// An identity name is a column but not of class [`ColumnClass::Label`].
    IdentityColumnNotLabel {
        /// The raw id whose identity failed validation.
        type_id: u32,
        /// The identity name whose column is not a `Label`.
        column: &'static str,
    },
    /// A counter or gauge column declares no unit.
    MissingUnit {
        /// The raw id whose column failed validation.
        type_id: u32,
        /// The column with no declared unit.
        column: &'static str,
    },
}

impl fmt::Display for LintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTypeId { type_id } => {
                write!(f, "type_id {type_id} is declared more than once")
            }
            Self::SortKeyColumnMissing { type_id, column } => {
                write!(
                    f,
                    "type_id {type_id} sorts by {column:?}, which is not a column"
                )
            }
            Self::MissingBaseline { type_id } => {
                write!(
                    f,
                    "type_id {type_id} is `changed` but has no `is_baseline` column"
                )
            }
            Self::BadTimestampColumn { type_id, column } => {
                write!(
                    f,
                    "type_id {type_id} column {column:?} is class Timestamp but not a non-nullable ts"
                )
            }
            Self::IdentityColumnMissing { type_id, column } => {
                write!(
                    f,
                    "type_id {type_id} identity uses {column:?}, which is not a column"
                )
            }
            Self::IdentityColumnNotLabel { type_id, column } => {
                write!(
                    f,
                    "type_id {type_id} identity uses {column:?}, which is not a Label column"
                )
            }
            Self::MissingUnit { type_id, column } => {
                write!(
                    f,
                    "type_id {type_id} column {column:?} is a counter or gauge with no declared unit"
                )
            }
        }
    }
}

fn lint_contract(contract: &TypeContract, out: &mut Vec<LintError>) {
    let raw = contract.type_id.get();

    for &name in contract.sort_key {
        if contract.column(name).is_none() {
            out.push(LintError::SortKeyColumnMissing {
                type_id: raw,
                column: name,
            });
        }
    }

    for &name in contract.identity {
        match contract.column(name) {
            None => out.push(LintError::IdentityColumnMissing {
                type_id: raw,
                column: name,
            }),
            Some(column) if !matches!(column.class, ColumnClass::Label) => {
                out.push(LintError::IdentityColumnNotLabel {
                    type_id: raw,
                    column: name,
                });
            }
            Some(_) => {}
        }
    }

    if matches!(contract.semantics, Semantics::Changed) && contract.column("is_baseline").is_none()
    {
        out.push(LintError::MissingBaseline { type_id: raw });
    }

    for column in contract.columns {
        // Readers key the time axis on a column literally named `ts`, so a
        // timestamp column must be a non-nullable `Ts` *and* carry that name.
        if matches!(column.class, ColumnClass::Timestamp)
            && (column.nullable || !matches!(column.ty, ColumnType::Ts) || column.name != "ts")
        {
            out.push(LintError::BadTimestampColumn {
                type_id: raw,
                column: column.name,
            });
        }
        if matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge)
            && column.unit.is_none()
        {
            out.push(LintError::MissingUnit {
                type_id: raw,
                column: column.name,
            });
        }
    }
}

/// Check every contract and the cross-type uniqueness of ids.
///
/// # Errors
///
/// Returns every [`LintError`] found across `contracts`; an empty registry
/// and a fully valid one both return `Ok`.
pub fn lint(contracts: &[TypeContract]) -> Result<(), Vec<LintError>> {
    let mut out = Vec::new();

    for (i, contract) in contracts.iter().enumerate() {
        lint_contract(contract, &mut out);
        // Quadratic, but the registry is a small fixed table.
        if contracts[..i]
            .iter()
            .any(|earlier| earlier.type_id == contract.type_id)
        {
            out.push(LintError::DuplicateTypeId {
                type_id: contract.type_id.get(),
            });
        }
    }

    if out.is_empty() { Ok(()) } else { Err(out) }
}

#[cfg(test)]
mod tests;
