//! Storage-neutral identities and immutable segment access.

use std::fmt::Debug;
use std::io;
use std::sync::Arc;

use kronika_format::{Catalog, CatalogLayoutError, DecodeError, ReadAt};
use kronika_layout::SegmentId;

use crate::CatalogSummary;
use crate::zms::ZmsError;
#[cfg(feature = "posix")]
use crate::{StoreObject, StoreWarning};

/// The product-visible kind of one stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ResourceKind {
    /// One immutable, self-contained ZMS segment.
    FinishedSegment,
}

/// Stable product identity of one stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceIdentity {
    segment_id: SegmentId,
    kind: ResourceKind,
}

impl ResourceIdentity {
    /// Identity of one immutable finished segment.
    #[must_use]
    pub const fn finished(segment_id: SegmentId) -> Self {
        Self {
            segment_id,
            kind: ResourceKind::FinishedSegment,
        }
    }

    /// Explicit segment identity supplied by the catalog source.
    #[must_use]
    pub const fn segment_id(self) -> SegmentId {
        self.segment_id
    }

    /// Product-visible resource kind.
    #[must_use]
    pub const fn kind(self) -> ResourceKind {
        self.kind
    }
}

/// One resource returned by catalog discovery.
///
/// `R` is an adapter-owned open token. Product code uses the identity and
/// catalog summary and passes the complete value back to its source to open
/// bytes; it does not interpret the token.
#[derive(Debug, Clone)]
pub struct SegmentResource<R> {
    identity: ResourceIdentity,
    captured_bytes: u64,
    summary: Arc<CatalogSummary>,
    handle: R,
}

impl<R> SegmentResource<R> {
    /// Build a listed resource around an adapter-owned open token.
    ///
    /// The token is transported unchanged back to the adapter. Storage
    /// implementations can keep keys opaque by using a public type with
    /// private fields. An adapter must compare the identity, captured length,
    /// and summary with the canonical values bound to that token before it
    /// opens or validates bytes.
    #[must_use]
    pub const fn new(
        identity: ResourceIdentity,
        captured_bytes: u64,
        summary: Arc<CatalogSummary>,
        handle: R,
    ) -> Self {
        Self {
            identity,
            captured_bytes,
            summary,
            handle,
        }
    }

    /// Stable product identity and kind.
    #[must_use]
    pub const fn identity(&self) -> ResourceIdentity {
        self.identity
    }

    /// Complete immutable object length in bytes.
    #[must_use]
    pub const fn captured_bytes(&self) -> u64 {
        self.captured_bytes
    }

    /// Compact validated catalog metadata.
    #[must_use]
    pub fn summary(&self) -> &CatalogSummary {
        &self.summary
    }

    /// Adapter-owned token used to open this exact listed resource.
    #[must_use]
    pub const fn token(&self) -> &R {
        &self.handle
    }
}

/// Product-visible subject of a non-fatal resource notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceWarningSubject {
    /// One immutable finished segment.
    FinishedSegment(ResourceIdentity),
}

/// Storage-neutral, payload-free notice from resource discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceWarning {
    subject: ResourceWarningSubject,
    code: &'static str,
}

impl ResourceWarning {
    /// Build a notice from a neutral subject and stable low-cardinality code.
    #[must_use]
    pub const fn new(subject: ResourceWarningSubject, code: &'static str) -> Self {
        Self { subject, code }
    }

    #[cfg(feature = "posix")]
    pub(crate) const fn from_store(warning: StoreWarning) -> Option<Self> {
        match warning.affected {
            StoreObject::Segment(address) => Some(Self::new(
                ResourceWarningSubject::FinishedSegment(ResourceIdentity::finished(address.id)),
                warning.reason.code(),
            )),
            StoreObject::ActiveJournal | StoreObject::Foreign(_) => None,
        }
    }

    /// Product-visible object class affected by the notice.
    #[must_use]
    pub const fn subject(self) -> ResourceWarningSubject {
        self.subject
    }

    /// Stable low-cardinality notice code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        self.code
    }
}

/// Storage-neutral class of an unavailable immutable resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceFailureKind {
    /// No object exists under the listed identity.
    NotFound,
    /// The source denied access.
    PermissionDenied,
    /// The source did not respond within its deadline.
    TimedOut,
    /// The source is temporarily busy.
    Busy,
    /// A bounded allocation could not be made.
    OutOfMemory,
    /// Positional bytes ended before the requested range.
    UnexpectedEof,
    /// The source returned invalid data outside ZMS framing.
    InvalidData,
    /// Another source-specific failure occurred.
    Other,
}

impl ResourceFailureKind {
    pub(crate) const fn from_io_kind(kind: io::ErrorKind) -> Self {
        match kind {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            io::ErrorKind::TimedOut => Self::TimedOut,
            io::ErrorKind::WouldBlock => Self::Busy,
            io::ErrorKind::OutOfMemory => Self::OutOfMemory,
            io::ErrorKind::UnexpectedEof => Self::UnexpectedEof,
            io::ErrorKind::InvalidData => Self::InvalidData,
            _ => Self::Other,
        }
    }
}

/// Storage-neutral failure from immutable catalog or byte access.
#[derive(Debug)]
#[non_exhaustive]
pub enum ResourceError {
    /// A token was not listed by this source instance.
    ForeignResource,
    /// Listed object identity changed while it was opened or decoded.
    Changed,
    /// A catalog listed the same immutable identity more than once.
    DuplicateIdentity(ResourceIdentity),
    /// The source could not make the immutable object available.
    Unavailable(ResourceFailureKind),
    /// Complete immutable bytes exceed the source's explicit bound.
    TooLarge {
        /// Complete resource length, bytes.
        len: u64,
        /// Maximum accepted resource length, bytes.
        max: u64,
    },
    /// The source is too short to contain a tail index.
    TooSmall,
    /// The first four bytes are not the ZMS magic.
    BadMagic,
    /// The trailing ZMS tail index failed to decode.
    TailIndex(DecodeError),
    /// The catalog declares a format version this build does not support.
    UnsupportedFormat {
        /// The `format_version` found in the catalog.
        version: u32,
    },
    /// `catalog_len` does not fit between the magic and tail index.
    BadCatalogLength,
    /// The catalog bytes failed to decode.
    Catalog(DecodeError),
    /// The catalog does not describe the canonical physical section layout.
    SectionLayout(CatalogLayoutError),
    /// A complete section body does not match its catalog checksum.
    SectionChecksum {
        /// Section type whose CRC32C did not match.
        type_id: u32,
    },
    /// A bounded catalog operation exceeded its retained-metadata limit.
    MetadataLimit {
        /// Configured retained-metadata limit.
        limit: usize,
    },
}

impl ResourceError {
    pub(crate) fn from_io(error: &io::Error) -> Self {
        if error.kind() == io::ErrorKind::Interrupted {
            Self::Changed
        } else {
            Self::Unavailable(ResourceFailureKind::from_io_kind(error.kind()))
        }
    }

    pub(crate) fn from_zms(error: ZmsError) -> Self {
        match error {
            ZmsError::Io(error) => Self::from_io(&error),
            ZmsError::MetadataLimit { limit } => Self::MetadataLimit { limit },
            ZmsError::TooSmall => Self::TooSmall,
            ZmsError::BadMagic => Self::BadMagic,
            ZmsError::TailIndex(error) => Self::TailIndex(error),
            ZmsError::UnsupportedFormat { version } => Self::UnsupportedFormat { version },
            ZmsError::BadCatalogLength => Self::BadCatalogLength,
            ZmsError::Catalog(error) => Self::Catalog(error),
            ZmsError::SectionLayout(error) => Self::SectionLayout(error),
            ZmsError::SectionChecksum { type_id } => Self::SectionChecksum { type_id },
        }
    }
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForeignResource => write!(f, "resource belongs to another source"),
            Self::Changed => write!(f, "resource changed while it was read"),
            Self::DuplicateIdentity(identity) => write!(
                f,
                "resource identity {} was listed more than once",
                identity.segment_id().get()
            ),
            Self::Unavailable(kind) => write!(f, "resource is unavailable: {kind:?}"),
            Self::TooLarge { len, max } => {
                write!(f, "resource of {len} bytes exceeds the byte limit of {max}")
            }
            Self::TooSmall => write!(f, "source is too small for a ZMS segment"),
            Self::BadMagic => write!(f, "source does not start with ZMS1 magic"),
            Self::TailIndex(error) => write!(f, "tail index decode failed: {error}"),
            Self::UnsupportedFormat { version } => {
                write!(f, "unsupported format version {version}")
            }
            Self::BadCatalogLength => write!(f, "catalog_len does not fit in the source"),
            Self::Catalog(error) => write!(f, "catalog decode failed: {error}"),
            Self::SectionLayout(error) => write!(f, "invalid section layout: {error}"),
            Self::SectionChecksum { type_id } => write!(
                f,
                "section {type_id} body checksum does not match its catalog"
            ),
            Self::MetadataLimit { limit } => {
                write!(f, "catalog metadata exceeds the byte limit of {limit}")
            }
        }
    }
}

impl std::error::Error for ResourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TailIndex(error) | Self::Catalog(error) => Some(error),
            Self::SectionLayout(error) => Some(error),
            Self::ForeignResource
            | Self::Changed
            | Self::DuplicateIdentity(_)
            | Self::Unavailable(_)
            | Self::TooLarge { .. }
            | Self::TooSmall
            | Self::BadMagic
            | Self::UnsupportedFormat { .. }
            | Self::BadCatalogLength
            | Self::SectionChecksum { .. }
            | Self::MetadataLimit { .. } => None,
        }
    }
}

impl From<io::Error> for ResourceError {
    fn from(error: io::Error) -> Self {
        Self::from_io(&error)
    }
}

/// Read an owned ZMS catalog through the storage-neutral error boundary.
///
/// # Errors
///
/// Returns a resource error when framing, catalog layout, or positional I/O is
/// invalid.
pub fn read_resource_catalog<R: ReadAt>(reader: &R) -> Result<Catalog, ResourceError> {
    crate::zms::read_catalog(reader).map_err(ResourceError::from_zms)
}

/// Immutable resources and non-fatal discovery diagnostics from one catalog pass.
#[derive(Debug)]
pub struct ResourceListing<R> {
    /// Unique immutable resources in ascending [`ResourceIdentity`] order.
    pub resources: Vec<SegmentResource<R>>,
    /// Objects excluded by catalog discovery.
    pub warnings: Vec<ResourceWarning>,
}

/// Discovers immutable segment identities and compact catalogs.
///
/// Discovery is deliberately separate from opening section bytes. A future
/// remote adapter can prepare an object range or local cache before returning
/// its synchronous [`ReadAt`] implementation.
pub trait ResourceCatalog {
    /// Adapter-owned token carried by a discovered resource.
    type Resource: Clone + Debug + Send + Sync + 'static;

    /// List immutable resources without retaining their complete bytes.
    ///
    /// # Errors
    ///
    /// Returns a resource error when a bounded catalog pass cannot complete.
    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError>;
}

/// Opens positional bytes for one immutable resource returned by a catalog.
pub trait ImmutableSegmentSource: ResourceCatalog {
    /// Synchronous byte reader handed to the existing decoder.
    type Bytes: ReadAt + Send + Sync + 'static;

    /// Open exactly one listed immutable object.
    ///
    /// # Errors
    ///
    /// Returns a resource error when the identity is foreign, stale, or unreadable.
    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, ResourceError>;

    /// Confirm that opened bytes still name the object captured by discovery.
    ///
    /// Call this after decoding the catalog and before publishing a product
    /// view. Sources with versioned immutable bytes may only need to confirm
    /// provenance; POSIX sources also recheck the opened file identity.
    ///
    /// # Errors
    ///
    /// Returns a resource error when the opened object no longer matches the
    /// listed resource.
    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), ResourceError>;
}

#[cfg(test)]
#[path = "resource/tests.rs"]
mod tests;
