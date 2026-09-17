//! Storage-neutral discovery and opening of immutable segments.

use std::sync::Arc;

use kronika_store::{
    ImmutableSegmentSource, ResourceCatalog, ResourceError, ResourceListing, SegmentResource,
    read_resource_catalog,
};

use crate::{ReaderError, Segment};

/// Product reader for immutable segments from one storage source.
///
/// Catalog discovery stays separate from opening positional bytes. The source
/// decides how an object is prepared; decoding remains synchronous.
#[derive(Debug)]
pub struct FinishedReader<S> {
    source: S,
}

impl<S> FinishedReader<S> {
    /// Bind a product reader to one immutable source.
    #[must_use]
    pub const fn new(source: S) -> Self {
        Self { source }
    }
}

impl<S: ResourceCatalog> FinishedReader<S> {
    /// Discover immutable identities and compact catalogs.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the bounded catalog pass cannot complete.
    pub fn resources(&self) -> Result<ResourceListing<S::Resource>, ReaderError> {
        let mut listing = self.source.resources()?;
        listing
            .resources
            .sort_unstable_by_key(SegmentResource::identity);
        if let Some(identity) = listing
            .resources
            .windows(2)
            .find(|pair| pair[0].identity() == pair[1].identity())
            .map(|pair| pair[0].identity())
        {
            return Err(ResourceError::DuplicateIdentity(identity).into());
        }
        Ok(listing)
    }
}

impl<S: ImmutableSegmentSource> FinishedReader<S> {
    /// Open one discovered resource through the production row decoder.
    ///
    /// # Errors
    ///
    /// Returns an error for a foreign or changed resource, an unreadable
    /// object, or an invalid full catalog.
    pub fn open_segment(
        &self,
        resource: &SegmentResource<S::Resource>,
    ) -> Result<Segment, ReaderError> {
        let bytes = self.source.open_resource(resource)?;
        let catalog = read_resource_catalog(&bytes);
        self.source.validate_opened(resource, &bytes)?;
        let catalog = Arc::new(catalog?);
        Ok(Segment::open_finished(
            bytes,
            catalog,
            resource.identity().segment_id().get(),
            resource.captured_bytes(),
            resource.summary(),
            format!("segment:{}", resource.identity().segment_id().get()),
        ))
    }
}
