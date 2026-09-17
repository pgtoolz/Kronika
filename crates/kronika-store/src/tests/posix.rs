use std::fs;
use std::io;

use kronika_format::ReadAt as _;
use kronika_layout::{LayoutError, LimitKind, SegmentAddress, SegmentId};

use super::{LISTING_RESERVE_ATTEMPTS, PosixSource, resource_scan_error};
use crate::{
    ImmutableSegmentSource as _, ResourceCatalog as _, ResourceIdentity, ResourceKind,
    ResourceWarningSubject, SegmentResource, read_resource_catalog,
};

const FIXTURE: &[u8] = include_bytes!("../../../kronika-format/tests/fixtures/minimal.zms");

fn write_segment(root: &std::path::Path, raw_id: i64) -> std::path::PathBuf {
    let address =
        SegmentAddress::new(SegmentId::new(raw_id).expect("segment id")).expect("segment address");
    let day = root
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    fs::create_dir_all(&day).expect("create day directory");
    let path = day.join(address.zms_name());
    fs::write(&path, FIXTURE).expect("write fixture");
    path
}

#[test]
fn posix_catalog_opens_opaque_positional_bytes_without_retaining_payload() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    let source = PosixSource::open(dir.path()).expect("POSIX source");
    LISTING_RESERVE_ATTEMPTS.with(|attempts| attempts.set(0));

    let listing = source.resources().expect("POSIX catalog");
    LISTING_RESERVE_ATTEMPTS.with(|attempts| assert_eq!(attempts.get(), 2));
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.resources.len(), 1);
    let resource = &listing.resources[0];
    assert_eq!(resource.identity().segment_id().get(), 1_000);
    assert_eq!(resource.identity().kind(), ResourceKind::FinishedSegment);
    assert_eq!(resource.captured_bytes(), FIXTURE.len() as u64);

    let bytes = source.open_resource(resource).expect("opened bytes");
    assert_eq!(bytes.byte_len().expect("byte length"), FIXTURE.len() as u64);
    assert_eq!(
        read_resource_catalog(&bytes).expect("POSIX catalog bytes"),
        read_resource_catalog(&FIXTURE).expect("fixture catalog")
    );
    source
        .validate_opened(resource, &bytes)
        .expect("stable opened bytes");
}

#[test]
fn posix_resource_and_bytes_are_bound_to_the_source_that_opened_them() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    let first = PosixSource::open(dir.path()).expect("first source");
    let second = PosixSource::open(dir.path()).expect("second source");
    let listing = first.resources().expect("first catalog");
    let resource = &listing.resources[0];
    let bytes = first.open_resource(resource).expect("first bytes");

    let open_error = second
        .open_resource(resource)
        .expect_err("foreign resource must fail");
    assert!(matches!(open_error, crate::ResourceError::ForeignResource));
    let validation_error = second
        .validate_opened(resource, &bytes)
        .expect_err("foreign bytes must fail");
    assert!(matches!(
        validation_error,
        crate::ResourceError::ForeignResource
    ));
}

#[test]
fn posix_resource_rejects_forged_length_and_summary() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    let source = PosixSource::open(dir.path()).expect("POSIX source");
    let listing = source.resources().expect("catalog");
    let resource = &listing.resources[0];
    let forged_length = SegmentResource::new(
        resource.identity(),
        resource.captured_bytes() + 1,
        std::sync::Arc::new(*resource.summary()),
        resource.token().clone(),
    );
    assert!(matches!(
        source.open_resource(&forged_length),
        Err(crate::ResourceError::ForeignResource)
    ));

    let mut altered_summary = *resource.summary();
    altered_summary.max_ts = altered_summary.max_ts.saturating_add(1);
    let forged_summary = SegmentResource::new(
        resource.identity(),
        resource.captured_bytes(),
        std::sync::Arc::new(altered_summary),
        resource.token().clone(),
    );
    assert!(matches!(
        source.open_resource(&forged_summary),
        Err(crate::ResourceError::ForeignResource)
    ));
}

#[test]
fn posix_post_catalog_validation_rechecks_the_opened_identity() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = write_segment(dir.path(), 1_000);
    let source = PosixSource::open(dir.path()).expect("POSIX source");
    let listing = source.resources().expect("POSIX catalog");
    let resource = &listing.resources[0];
    let bytes = source.open_resource(resource).expect("opened bytes");
    drop(read_resource_catalog(&bytes).expect("catalog bytes"));

    let mut changed = FIXTURE.to_vec();
    changed[4] ^= 1;
    fs::write(path, changed).expect("change opened fixture");

    let error = source
        .validate_opened(resource, &bytes)
        .expect_err("changed identity must fail");
    assert!(matches!(error, crate::ResourceError::Changed));
}

#[test]
fn posix_immutable_listing_ignores_foreign_entries() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    fs::write(dir.path().join("unsupported-entry"), b"private").expect("foreign entry");
    let source = PosixSource::open(dir.path()).expect("POSIX source");

    let listing = source.resources().expect("POSIX catalog");
    assert_eq!(listing.resources.len(), 1);
    assert!(listing.warnings.is_empty());
}

#[test]
fn posix_immutable_listing_does_not_read_a_corrupt_active_journal() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    fs::write(dir.path().join("active.wal"), b"not a journal").expect("active journal");
    let source = PosixSource::open(dir.path()).expect("POSIX source");
    crate::local::ACTIVE_JOURNAL_SCANS.with(|scans| scans.set(0));

    let listing = source.resources().expect("POSIX catalog");
    assert_eq!(listing.resources.len(), 1);
    assert!(listing.warnings.is_empty());
    crate::local::ACTIVE_JOURNAL_SCANS.with(|scans| assert_eq!(scans.get(), 0));
}

#[test]
fn posix_immutable_listing_keeps_finished_segment_warnings() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    let invalid = write_segment(dir.path(), 2_000);
    fs::write(invalid, b"bad").expect("invalid finished segment");
    let source = PosixSource::open(dir.path()).expect("POSIX source");

    let listing = source.resources().expect("POSIX catalog");
    assert_eq!(listing.resources.len(), 1);
    let warning = listing.warnings.first().expect("finished warning");
    assert_eq!(
        warning.subject(),
        ResourceWarningSubject::FinishedSegment(ResourceIdentity::finished(
            SegmentId::new(2_000).expect("segment id")
        ))
    );
    assert!(warning.code().starts_with("invalid_zms_"));
}

#[test]
fn posix_listing_conversion_rejects_the_peak_before_reserving_outputs() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 1_000);
    let source = PosixSource::open(dir.path()).expect("POSIX source");
    let scan = source
        .dir
        .scan_finished_catalogs()
        .expect("finished catalog scan");
    let injected_limit = scan.metadata_bytes;
    LISTING_RESERVE_ATTEMPTS.with(|attempts| attempts.set(0));

    let error = source
        .listing_from_scan(scan, injected_limit)
        .expect_err("output allocation must exceed the injected limit");

    assert!(matches!(
        error,
        crate::ResourceError::MetadataLimit { limit } if limit == injected_limit
    ));
    LISTING_RESERVE_ATTEMPTS.with(|attempts| assert_eq!(attempts.get(), 0));
}

#[test]
fn posix_listing_retains_ascending_resource_identity_order() {
    let dir = tempfile::tempdir().expect("temporary directory");
    write_segment(dir.path(), 2_000);
    write_segment(dir.path(), 1_000);
    let source = PosixSource::open(dir.path()).expect("POSIX source");

    let listing = source.resources().expect("POSIX catalog");
    let ids = listing
        .resources
        .iter()
        .map(|resource| resource.identity().segment_id().get())
        .collect::<Vec<_>>();
    assert_eq!(ids, [1_000, 2_000]);
}

#[test]
fn posix_maps_only_metadata_traversal_limits_to_the_neutral_limit() {
    let error = io::Error::new(
        io::ErrorKind::InvalidData,
        LayoutError::TraversalLimitExceeded {
            kind: LimitKind::MetadataBytes,
            limit: 17,
        },
    );

    assert!(matches!(
        resource_scan_error(error),
        crate::ResourceError::MetadataLimit { limit: 17 }
    ));

    let nested = io::Error::new(
        io::ErrorKind::InvalidData,
        crate::StoreError::Layout(LayoutError::TraversalLimitExceeded {
            kind: LimitKind::MetadataBytes,
            limit: 23,
        }),
    );
    assert!(matches!(
        resource_scan_error(nested),
        crate::ResourceError::MetadataLimit { limit: 23 }
    ));
}
