//! Reading a finished segment: its catalog, its checksums, and why one is
//! rejected.

use std::fs::File;
use std::io;

use kronika_format::{Catalog, ReadAt};
use kronika_layout::FileIdentity;

use crate::catalog_summary::CatalogSummary;
use crate::source::{
    InvalidZmsReason, StoreError, StoreIoFailure, StoreIoOperation, StoreObject, StoreWarning,
    StoreWarningReason,
};
pub(crate) use crate::zms::FinishedValidation;
use crate::zms::ZmsError;

#[cfg(test)]
use super::CATALOG_SUMMARY_READS;
use super::budget::{metadata_limit_store, store_io};

pub(super) enum ZmsOpen {
    Open(File),
    Invalid(StoreIoFailure),
}

pub(super) const fn invalid_zms_warning(
    address: kronika_layout::SegmentAddress,
    identity: FileIdentity,
    reason: InvalidZmsReason,
    failure: Option<StoreIoFailure>,
) -> StoreWarning {
    StoreWarning {
        affected: StoreObject::Segment(address),
        reason: StoreWarningReason::InvalidZms(reason),
        identity: Some(identity),
        failure,
    }
}

pub(super) fn stale_finished_zms(
    address: kronika_layout::SegmentAddress,
    phase: &str,
) -> io::Error {
    io::Error::new(
        io::ErrorKind::Interrupted,
        format!(
            "finished segment {} changed {phase}; retry the scan",
            address.zms_name()
        ),
    )
}

#[cfg(test)]
pub(super) fn read_validated_zms_summary<R: ReadAt>(
    reader: &R,
    retained_metadata: usize,
    metadata_limit: usize,
) -> Result<CatalogSummary, StoreError> {
    read_zms_summary(
        reader,
        retained_metadata,
        metadata_limit,
        FinishedValidation::Complete,
    )
}

pub(crate) fn read_zms_summary<R: ReadAt>(
    reader: &R,
    retained_metadata: usize,
    metadata_limit: usize,
    validation: FinishedValidation,
) -> Result<CatalogSummary, StoreError> {
    #[cfg(test)]
    CATALOG_SUMMARY_READS.with(|reads| reads.set(reads.get().saturating_add(1)));
    crate::zms::read_zms_summary(reader, retained_metadata, metadata_limit, validation)
        .map_err(store_zms_error)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ZmsInvalid {
    pub(super) reason: InvalidZmsReason,
    pub(super) failure: Option<StoreIoFailure>,
}

pub(super) fn classify_zms_validation(
    file: &File,
    expected: FileIdentity,
    address: kronika_layout::SegmentAddress,
    validation: Result<CatalogSummary, StoreError>,
) -> io::Result<Result<CatalogSummary, ZmsInvalid>> {
    // Identity instability always wins over a stable-invalid verdict. A
    // replacement or in-place rewrite must be retried from discovery instead
    // of excluding whichever generation happened to be read.
    match FileIdentity::from_file(file) {
        Ok(actual) if actual == expected => {}
        Ok(_changed) => {
            return Err(stale_finished_zms(address, "during complete validation"));
        }
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof
            ) =>
        {
            return Err(stale_finished_zms(address, "during complete validation"));
        }
        Err(source) if source.kind() == io::ErrorKind::OutOfMemory => return Err(source),
        Err(source) => {
            return Ok(Err(ZmsInvalid {
                reason: InvalidZmsReason::Io,
                failure: Some(StoreIoFailure::from_error(
                    StoreIoOperation::Metadata,
                    &source,
                )),
            }));
        }
    }
    match validation {
        Ok(summary) => Ok(Ok(summary)),
        Err(StoreError::Io(source)) if source.kind() == io::ErrorKind::UnexpectedEof => {
            Ok(Err(ZmsInvalid {
                reason: InvalidZmsReason::Truncated,
                failure: Some(StoreIoFailure::from_error(StoreIoOperation::Read, &source)),
            }))
        }
        Err(StoreError::Io(source)) if source.kind() == io::ErrorKind::OutOfMemory => Err(source),
        Err(StoreError::Io(source)) => Ok(Err(ZmsInvalid {
            reason: InvalidZmsReason::Io,
            failure: Some(StoreIoFailure::from_error(StoreIoOperation::Read, &source)),
        })),
        Err(StoreError::TooSmall) => Ok(invalid_zms(InvalidZmsReason::TooSmall)),
        Err(StoreError::BadMagic) => Ok(invalid_zms(InvalidZmsReason::BadMagic)),
        Err(StoreError::TailIndex(_)) => Ok(invalid_zms(InvalidZmsReason::TailIndex)),
        Err(StoreError::UnsupportedFormat { .. }) => {
            Ok(invalid_zms(InvalidZmsReason::UnsupportedFormat))
        }
        Err(StoreError::BadCatalogLen) => Ok(invalid_zms(InvalidZmsReason::BadCatalogLength)),
        Err(StoreError::Catalog(_)) => Ok(invalid_zms(InvalidZmsReason::Catalog)),
        Err(StoreError::SectionLayout(_) | StoreError::OutOfBounds) => {
            Ok(invalid_zms(InvalidZmsReason::CanonicalLayout))
        }
        Err(StoreError::SectionChecksum { .. }) => {
            Ok(invalid_zms(InvalidZmsReason::SectionChecksum))
        }
        Err(error @ (StoreError::Layout(_) | StoreError::ActivePartTooLarge { .. })) => {
            Err(store_io(error))
        }
    }
}

pub(super) const fn invalid_zms(reason: InvalidZmsReason) -> Result<CatalogSummary, ZmsInvalid> {
    Err(ZmsInvalid {
        reason,
        failure: None,
    })
}

fn store_zms_error(error: ZmsError) -> StoreError {
    match error {
        ZmsError::Io(error) => StoreError::Io(error),
        ZmsError::MetadataLimit { limit } => metadata_limit_store(limit),
        ZmsError::TooSmall => StoreError::TooSmall,
        ZmsError::BadMagic => StoreError::BadMagic,
        ZmsError::TailIndex(error) => StoreError::TailIndex(error),
        ZmsError::UnsupportedFormat { version } => StoreError::UnsupportedFormat { version },
        ZmsError::BadCatalogLength => StoreError::BadCatalogLen,
        ZmsError::Catalog(error) => StoreError::Catalog(error),
        ZmsError::SectionLayout(error) => StoreError::SectionLayout(error),
        ZmsError::SectionChecksum { type_id } => StoreError::SectionChecksum { type_id },
    }
}

/// Read and decode the end catalog from any [`ReadAt`] source.
///
/// Reads only the tail index and catalog block; no section bodies are loaded.
/// Directory discovery uses a compact summary instead; this function remains
/// available for callers that explicitly need an owned catalog.
///
/// # Errors
///
/// Returns [`StoreError`] when the source is too small, the magic bytes are
/// wrong, the format version is unsupported, the catalog block cannot be
/// located, the catalog bytes are corrupt, or a catalog entry points outside
/// the section area.
pub fn read_catalog<R: ReadAt>(reader: &R) -> Result<Catalog, StoreError> {
    crate::zms::read_catalog(reader).map_err(store_zms_error)
}
