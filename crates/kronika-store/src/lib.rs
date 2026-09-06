//! Segment storage abstractions.
//!
//! [`LocalDir`] provides read-only access to a local directory of ZMS segments.
//! It lists finished `.zms` files, validates their end catalogs without retaining
//! per-entry tables, and streams valid parts from the `active.wal` journal
//! without loading the whole journal.
//!
//! Each discovered finished unit retains its exact [`kronika_layout::FileIdentity`]
//! and an `Arc`-shared, fixed-size [`CatalogSummary`]. The summary includes a
//! 512-bit Bloom filter for section types with non-zero rows. The filter has no
//! false negatives; a positive result is only a hint, and consumers must confirm
//! it against the full catalog opened lazily under their typed work budget.
//!
//! Typed filesystem traversal comes from `kronika-layout`; byte framing comes
//! from `kronika-format`. Section decoding lives in `kronika-reader`.
//!
//! Catalog, part, and retained-metadata sizes are checked before allocation.
//! Discovery checks the opened ZMS identity before and after reading its catalog.
//! Unchanged scans and cloned scan results share the finished collection and
//! summaries. Directory traversal, journal metadata, retained collections, and
//! summaries use the same 128 MiB `LayoutLimits::max_metadata_bytes` ceiling.
//! A cached finished or live reference may become stale after publication or reset,
//! so consumers must refresh instead of treating changed bytes as the original
//! unit.
//!
//! Journal v1 is the journal format shipped in Kronika 1.0.0. There is no
//! alternate-version reader or migration path.

// Cargo exposes Unix-only dev dependencies to every host test build.
#[cfg(all(test, not(feature = "posix"), unix))]
use tempfile as _;

mod catalog_summary;
mod embedded;
#[cfg(feature = "posix")]
mod local;
#[cfg(feature = "posix")]
mod posix;
mod resource;
#[cfg(feature = "posix")]
mod source;
mod zms;

pub use catalog_summary::{
    CatalogDigest, CatalogLayoutDigest, CatalogSummary, CatalogSummaryError, catalog_digests,
};
pub use embedded::{EmbeddedResource, EmbeddedSource, SharedSegmentBytes};
#[cfg(feature = "posix")]
pub use local::{LocalDir, is_active_journal_scan_error, read_catalog};
#[cfg(feature = "posix")]
pub use posix::{PosixResource, PosixSegmentBytes, PosixSource};
pub use resource::{
    ImmutableSegmentSource, ResourceCatalog, ResourceError, ResourceFailureKind, ResourceIdentity,
    ResourceKind, ResourceListing, ResourceWarning, ResourceWarningSubject, SegmentResource,
    read_resource_catalog,
};
#[cfg(feature = "posix")]
pub use source::{
    ActiveJournalWarningReason, ActivePart, ActiveSnapshot, FinalUnit, InvalidZmsReason,
    JournalScan, LocalScan, StoreError, StoreIoFailure, StoreIoOperation, StoreObject,
    StoreWarning, StoreWarningReason,
};
pub use zms::validate_finished_zms;
