//! POSIX adapter for immutable finished-segment discovery and bytes.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use kronika_format::ReadAt;
use kronika_layout::{LayoutError, LimitKind};

use crate::local::summary_allocation_bytes;
use crate::source::FinalUnit;
use crate::{
    ImmutableSegmentSource, LocalDir, LocalScan, ResourceCatalog, ResourceError,
    ResourceFailureKind, ResourceIdentity, ResourceListing, ResourceWarning, SegmentResource,
    StoreWarning,
};

#[cfg(test)]
std::thread_local! {
    static LISTING_RESERVE_ATTEMPTS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

const fn metadata_limit(limit: usize) -> ResourceError {
    ResourceError::MetadataLimit { limit }
}

fn vector_bytes<T>(capacity: usize, limit: usize) -> Result<usize, ResourceError> {
    capacity
        .checked_mul(size_of::<T>())
        .ok_or_else(|| metadata_limit(limit))
}

fn admitted_total(parts: &[usize], limit: usize) -> Result<usize, ResourceError> {
    let total = parts
        .iter()
        .try_fold(0_usize, |total, part| total.checked_add(*part))
        .ok_or_else(|| metadata_limit(limit))?;
    if total > limit {
        Err(metadata_limit(limit))
    } else {
        Ok(total)
    }
}

fn try_reserve_listing<T>(
    values: &mut Vec<T>,
    additional: usize,
    limit: usize,
) -> Result<(), ResourceError> {
    #[cfg(test)]
    LISTING_RESERVE_ATTEMPTS.with(|attempts| attempts.set(attempts.get().saturating_add(1)));
    values
        .try_reserve_exact(additional)
        .map_err(|_error| metadata_limit(limit))
}

fn resource_scan_error(error: io::Error) -> ResourceError {
    let metadata_limit = error.get_ref().and_then(|source| {
        let mut current: &(dyn std::error::Error + 'static) = source;
        loop {
            if let Some(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::MetadataBytes,
                limit,
            }) = current.downcast_ref::<LayoutError>()
            {
                return Some(*limit);
            }
            current = current.source()?;
        }
    });
    metadata_limit.map_or_else(
        || error.into(),
        |limit| ResourceError::MetadataLimit { limit },
    )
}

/// POSIX-backed catalog and immutable segment source.
///
/// Filesystem traversal and local handles remain private to this adapter. The
/// resource and byte types exposed through the storage traits are opaque.
#[derive(Clone)]
pub struct PosixSource {
    dir: LocalDir,
    source_id: Arc<()>,
}

impl PosixSource {
    /// Open a POSIX data directory as an immutable segment source.
    ///
    /// This opens only the root directory descriptor. Catalog discovery is
    /// deferred until [`ResourceCatalog::resources`] is called.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if `root` is not a directory or cannot be opened.
    pub fn open(root: &Path) -> io::Result<Self> {
        Ok(Self {
            dir: LocalDir::open(root)?,
            source_id: Arc::new(()),
        })
    }

    fn checked_resource<'a>(
        &self,
        resource: &'a SegmentResource<PosixResource>,
    ) -> Result<&'a FinalUnit, ResourceError> {
        let handle = resource.token();
        if !Arc::ptr_eq(&handle.source_id, &self.source_id)
            || resource.identity() != ResourceIdentity::finished(handle.unit.address.id)
            || resource.captured_bytes() != handle.unit.identity.len
            || resource.summary() != handle.unit.summary.as_ref()
        {
            return Err(ResourceError::ForeignResource);
        }
        Ok(&handle.unit)
    }

    fn listing_from_scan(
        &self,
        scan: LocalScan,
        limit: usize,
    ) -> Result<ResourceListing<PosixResource>, ResourceError> {
        let resource_count = scan.finished.len();
        let warning_count = scan
            .warnings
            .iter()
            .copied()
            .filter_map(ResourceWarning::from_store)
            .count();
        let stored_warning_bytes = vector_bytes::<StoreWarning>(scan.warnings.capacity(), limit)?;
        let scan_retained = admitted_total(&[scan.metadata_bytes, stored_warning_bytes], limit)?;
        let requested_resource_bytes =
            vector_bytes::<SegmentResource<PosixResource>>(resource_count, limit)?;
        let requested_warning_bytes = vector_bytes::<ResourceWarning>(warning_count, limit)?;
        let retained_summary_bytes = resource_count
            .checked_mul(summary_allocation_bytes())
            .ok_or_else(|| metadata_limit(limit))?;

        // Admit both the conversion peak and the returned listing before
        // requesting either output allocation.
        admitted_total(
            &[
                scan_retained,
                requested_resource_bytes,
                requested_warning_bytes,
            ],
            limit,
        )?;
        admitted_total(
            &[
                retained_summary_bytes,
                requested_resource_bytes,
                requested_warning_bytes,
            ],
            limit,
        )?;

        let mut resources = Vec::new();
        try_reserve_listing(&mut resources, resource_count, limit)?;
        let resource_bytes =
            vector_bytes::<SegmentResource<PosixResource>>(resources.capacity(), limit)?;
        admitted_total(
            &[scan_retained, resource_bytes, requested_warning_bytes],
            limit,
        )?;

        let mut warnings = Vec::new();
        try_reserve_listing(&mut warnings, warning_count, limit)?;
        let warning_bytes = vector_bytes::<ResourceWarning>(warnings.capacity(), limit)?;
        admitted_total(&[scan_retained, resource_bytes, warning_bytes], limit)?;
        admitted_total(
            &[retained_summary_bytes, resource_bytes, warning_bytes],
            limit,
        )?;

        let LocalScan {
            finished,
            warnings: stored_warnings,
            ..
        } = scan;
        let finished = Arc::try_unwrap(finished)
            .map_err(|_shared| ResourceError::Unavailable(ResourceFailureKind::Other))?;
        for unit in finished {
            resources.push(SegmentResource::new(
                ResourceIdentity::finished(unit.address.id),
                unit.identity.len,
                Arc::clone(&unit.summary),
                PosixResource {
                    unit,
                    source_id: Arc::clone(&self.source_id),
                },
            ));
        }
        warnings.extend(
            stored_warnings
                .into_iter()
                .filter_map(ResourceWarning::from_store),
        );
        Ok(ResourceListing {
            resources,
            warnings,
        })
    }
}

impl std::fmt::Debug for PosixSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosixSource").finish_non_exhaustive()
    }
}

/// Opaque token for one finished segment listed by a [`PosixSource`].
#[derive(Clone)]
pub struct PosixResource {
    unit: FinalUnit,
    source_id: Arc<()>,
}

impl std::fmt::Debug for PosixResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosixResource")
            .field(
                "identity",
                &ResourceIdentity::finished(self.unit.address.id),
            )
            .finish_non_exhaustive()
    }
}

/// Opaque positional byte reader for one opened POSIX segment.
pub struct PosixSegmentBytes {
    file: File,
    source_id: Arc<()>,
}

impl std::fmt::Debug for PosixSegmentBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosixSegmentBytes").finish_non_exhaustive()
    }
}

impl ReadAt for PosixSegmentBytes {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.file.read_exact_at(buf, offset)
    }

    fn byte_len(&self) -> io::Result<u64> {
        self.file.byte_len()
    }
}

impl ResourceCatalog for PosixSource {
    type Resource = PosixResource;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError> {
        let scan = self
            .dir
            .scan_finished_catalogs()
            .map_err(resource_scan_error)?;
        self.listing_from_scan(scan, self.dir.metadata_limit())
    }
}

impl ImmutableSegmentSource for PosixSource {
    type Bytes = PosixSegmentBytes;

    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, ResourceError> {
        let unit = self.checked_resource(resource)?;
        Ok(PosixSegmentBytes {
            file: self.dir.open_finished(unit)?,
            source_id: Arc::clone(&self.source_id),
        })
    }

    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), ResourceError> {
        let unit = self.checked_resource(resource)?;
        if !Arc::ptr_eq(&bytes.source_id, &self.source_id) {
            return Err(ResourceError::ForeignResource);
        }
        self.dir
            .validate_finished_file(&bytes.file, unit)
            .map_err(ResourceError::from)
    }
}

#[cfg(test)]
#[path = "posix/tests.rs"]
mod tests;
