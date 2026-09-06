//! One immutable ZMS segment retained in owned bytes or an owned file handle.

#[cfg(feature = "posix")]
use std::fs::File;
use std::io;
use std::sync::Arc;

use kronika_format::ReadAt;
#[cfg(feature = "posix")]
use kronika_layout::FileIdentity;
use kronika_layout::SegmentId;

use crate::{
    CatalogSummary, ImmutableSegmentSource, ResourceCatalog, ResourceError, ResourceIdentity,
    ResourceListing, SegmentResource, validate_finished_zms,
};

struct OwnedSegment(Vec<u8>);

#[derive(Clone)]
enum SegmentStorage {
    Owned(Arc<OwnedSegment>),
    #[cfg(feature = "posix")]
    File {
        file: Arc<File>,
        identity: FileIdentity,
    },
}

/// Shared positional bytes used by an embedded finished segment.
#[derive(Clone)]
pub struct SharedSegmentBytes {
    storage: SegmentStorage,
    source_id: Arc<()>,
}

impl SharedSegmentBytes {
    fn len(&self) -> u64 {
        match &self.storage {
            SegmentStorage::Owned(bytes) => bytes.0.len() as u64,
            #[cfg(feature = "posix")]
            SegmentStorage::File { identity, .. } => identity.len,
        }
    }

    #[cfg(feature = "posix")]
    fn validate_file_unchanged(&self) -> Result<(), ResourceError> {
        match &self.storage {
            SegmentStorage::Owned(_) => Ok(()),
            SegmentStorage::File { file, identity } => {
                if FileIdentity::from_file(file)? == *identity {
                    Ok(())
                } else {
                    Err(ResourceError::Changed)
                }
            }
        }
    }
}

impl std::fmt::Debug for SharedSegmentBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSegmentBytes")
            .field("len", &self.len())
            .finish()
    }
}

impl ReadAt for SharedSegmentBytes {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        match &self.storage {
            SegmentStorage::Owned(bytes) => bytes.0.read_exact_at(buf, offset),
            #[cfg(feature = "posix")]
            SegmentStorage::File { file, .. } => file.read_exact_at(buf, offset),
        }
    }

    fn byte_len(&self) -> io::Result<u64> {
        match &self.storage {
            SegmentStorage::Owned(_) => Ok(self.len()),
            #[cfg(feature = "posix")]
            SegmentStorage::File { file, .. } => file.byte_len(),
        }
    }
}

/// Opaque token binding a resource to the embedded source that listed it.
#[derive(Debug, Clone)]
pub struct EmbeddedResource(Arc<()>);

/// Catalog and bytes for exactly one explicit finished segment.
#[derive(Debug, Clone)]
pub struct EmbeddedSource {
    identity: ResourceIdentity,
    bytes: SharedSegmentBytes,
    summary: Arc<CatalogSummary>,
    source_id: Arc<()>,
}

impl EmbeddedSource {
    fn resource_is_canonical(&self, resource: &SegmentResource<EmbeddedResource>) -> bool {
        resource.identity() == self.identity
            && resource.captured_bytes() == self.bytes.len()
            && resource.summary() == self.summary.as_ref()
            && Arc::ptr_eq(&resource.token().0, &self.source_id)
    }

    /// Validate exclusively owned ZMS bytes under the supplied segment identity.
    ///
    /// The `Vec` allocation, including its capacity, is moved behind shared
    /// ownership without copying its contents. Source, open, and clone handles
    /// retain that same allocation. Retained payload capacity is the original
    /// `Vec` capacity and excludes the `Arc` control block and compact catalog
    /// metadata. The ZMS format does not contain its segment ID, so the caller
    /// must provide it. `max_segment_bytes` bounds the logical length before
    /// format validation; a caller with a retained-memory budget must account
    /// the vector capacity separately.
    ///
    /// # Errors
    ///
    /// Returns a limit, format, checksum, or bounded-metadata error for invalid
    /// bytes.
    pub fn from_owned(
        segment_id: SegmentId,
        bytes: Vec<u8>,
        max_segment_bytes: u64,
    ) -> Result<Self, ResourceError> {
        let summary = validate_finished_zms(&bytes, max_segment_bytes)?;
        let source_id = Arc::new(());
        Ok(Self {
            identity: ResourceIdentity::finished(segment_id),
            bytes: SharedSegmentBytes {
                storage: SegmentStorage::Owned(Arc::new(OwnedSegment(bytes))),
                source_id: Arc::clone(&source_id),
            },
            summary: Arc::new(summary),
            source_id,
        })
    }

    /// Validate and bind an already-open ZMS file without loading its body.
    ///
    /// # Errors
    ///
    /// Returns a file identity, size, format, layout, or checksum failure.
    #[cfg(feature = "posix")]
    pub fn from_file(
        segment_id: SegmentId,
        file: File,
        max_segment_bytes: u64,
    ) -> Result<Self, ResourceError> {
        let before = FileIdentity::from_file(&file)?;
        let summary = validate_finished_zms(&file, max_segment_bytes)?;
        let after = FileIdentity::from_file(&file)?;
        if before != after {
            return Err(ResourceError::Changed);
        }
        let source_id = Arc::new(());
        Ok(Self {
            identity: ResourceIdentity::finished(segment_id),
            bytes: SharedSegmentBytes {
                storage: SegmentStorage::File {
                    file: Arc::new(file),
                    identity: after,
                },
                source_id: Arc::clone(&source_id),
            },
            summary: Arc::new(summary),
            source_id,
        })
    }
}

impl ResourceCatalog for EmbeddedSource {
    type Resource = EmbeddedResource;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError> {
        #[cfg(feature = "posix")]
        self.bytes.validate_file_unchanged()?;
        Ok(ResourceListing {
            resources: vec![SegmentResource::new(
                self.identity,
                self.bytes.len(),
                Arc::clone(&self.summary),
                EmbeddedResource(Arc::clone(&self.source_id)),
            )],
            warnings: Vec::new(),
        })
    }
}

impl ImmutableSegmentSource for EmbeddedSource {
    type Bytes = SharedSegmentBytes;

    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, ResourceError> {
        if !self.resource_is_canonical(resource) {
            return Err(ResourceError::ForeignResource);
        }
        #[cfg(feature = "posix")]
        self.bytes.validate_file_unchanged()?;
        Ok(self.bytes.clone())
    }

    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), ResourceError> {
        if !self.resource_is_canonical(resource) || !Arc::ptr_eq(&bytes.source_id, &self.source_id)
        {
            return Err(ResourceError::ForeignResource);
        }
        #[cfg(feature = "posix")]
        self.bytes.validate_file_unchanged()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "embedded/tests.rs"]
mod tests;
