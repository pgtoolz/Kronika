use std::sync::Arc;

use kronika_layout::SegmentId;

use super::{EmbeddedSource, SegmentStorage, SharedSegmentBytes};
use crate::{
    ImmutableSegmentSource as _, ResourceCatalog as _, ResourceKind, SegmentResource,
    read_resource_catalog,
};

const FIXTURE: &[u8] = include_bytes!("../../../kronika-format/tests/fixtures/minimal.zms");
const FIXTURE_LIMIT: u64 = FIXTURE.len() as u64;

fn owned(bytes: &SharedSegmentBytes) -> &Vec<u8> {
    match &bytes.storage {
        SegmentStorage::Owned(bytes) => &bytes.0,
        #[cfg(feature = "posix")]
        SegmentStorage::File { .. } => panic!("expected owned bytes"),
    }
}

fn source(id: i64) -> EmbeddedSource {
    EmbeddedSource::from_owned(
        SegmentId::new(id).expect("segment id"),
        FIXTURE.to_vec(),
        FIXTURE_LIMIT,
    )
    .expect("valid embedded fixture")
}

#[test]
fn embedded_catalog_keeps_explicit_identity() {
    let source = source(42);
    let listing = source.resources().expect("embedded catalog");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.resources.len(), 1);
    let resource = &listing.resources[0];
    assert_eq!(resource.identity().segment_id().get(), 42);
    assert_eq!(resource.identity().kind(), ResourceKind::FinishedSegment);
    assert_eq!(resource.captured_bytes(), FIXTURE.len() as u64);

    let opened = source.open_resource(resource).expect("open embedded bytes");
    source
        .validate_opened(resource, &opened)
        .expect("validate embedded bytes");
    assert_eq!(
        read_resource_catalog(&opened).expect("embedded catalog"),
        read_resource_catalog(&FIXTURE).expect("fixture catalog")
    );
}

#[test]
fn embedded_owned_source_keeps_the_vec_allocation_across_clones_and_opens() {
    let mut bytes = Vec::with_capacity(FIXTURE.len() + 257);
    bytes.extend_from_slice(FIXTURE);
    let original_ptr = bytes.as_ptr();
    let original_len = bytes.len();
    let original_capacity = bytes.capacity();
    let source = EmbeddedSource::from_owned(
        SegmentId::new(47).expect("segment id"),
        bytes,
        FIXTURE_LIMIT,
    )
    .expect("valid owned fixture");
    let cloned = source.clone();
    let listing = source.resources().expect("owned catalog");
    let opened = source
        .open_resource(&listing.resources[0])
        .expect("open owned bytes");
    let opened_clone = opened.clone();

    assert_eq!(owned(&source.bytes).as_ptr(), original_ptr);
    assert_eq!(owned(&cloned.bytes).as_ptr(), original_ptr);
    assert_eq!(owned(&opened).as_ptr(), original_ptr);
    assert_eq!(owned(&opened_clone).as_ptr(), original_ptr);
    assert_eq!(owned(&source.bytes).capacity(), original_capacity);
    assert_eq!(owned(&cloned.bytes).capacity(), original_capacity);
    assert_eq!(opened.len(), original_len as u64);
    assert_eq!(listing.resources[0].identity().segment_id().get(), 47);
    drop(source);
    drop(cloned);
    drop(opened);
    assert_eq!(owned(&opened_clone).as_ptr(), original_ptr);
    assert_eq!(opened_clone.len(), original_len as u64);
    assert_eq!(
        read_resource_catalog(&opened_clone).expect("catalog after source drop"),
        read_resource_catalog(&FIXTURE).expect("fixture catalog")
    );
}

#[test]
fn embedded_owned_source_keeps_validation_and_limit_failures() {
    let error = EmbeddedSource::from_owned(
        SegmentId::new(48).expect("segment id"),
        FIXTURE.to_vec(),
        FIXTURE_LIMIT - 1,
    )
    .expect_err("fixture must exceed the limit");
    assert!(matches!(
        error,
        crate::ResourceError::TooLarge {
            len: FIXTURE_LIMIT,
            max
        } if max == FIXTURE_LIMIT - 1
    ));

    let error = EmbeddedSource::from_owned(
        SegmentId::new(48).expect("segment id"),
        b"not a ZMS".to_vec(),
        64,
    )
    .expect_err("invalid owned bytes");
    assert!(matches!(
        error,
        crate::ResourceError::TailIndex(_) | crate::ResourceError::TooSmall
    ));
}

#[test]
fn identical_zms_bytes_keep_each_supplied_segment_identity() {
    let first = EmbeddedSource::from_owned(
        SegmentId::new(45).expect("segment id"),
        FIXTURE.to_vec(),
        FIXTURE_LIMIT,
    )
    .expect("first source");
    let second = EmbeddedSource::from_owned(
        SegmentId::new(46).expect("segment id"),
        FIXTURE.to_vec(),
        FIXTURE_LIMIT,
    )
    .expect("second source");

    assert_eq!(
        first.resources().expect("first catalog").resources[0]
            .identity()
            .segment_id()
            .get(),
        45
    );
    assert_eq!(
        second.resources().expect("second catalog").resources[0]
            .identity()
            .segment_id()
            .get(),
        46
    );
}

#[test]
fn embedded_resource_is_bound_to_the_source_that_listed_it() {
    let first = source(42);
    let second = source(42);
    let listing = first.resources().expect("first catalog");
    let error = second
        .open_resource(&listing.resources[0])
        .expect_err("foreign resource must fail");
    assert!(matches!(error, crate::ResourceError::ForeignResource));
}

#[test]
fn embedded_resource_rejects_forged_length_and_summary() {
    let source = source(42);
    let listing = source.resources().expect("catalog");
    let resource = &listing.resources[0];
    let forged_length = SegmentResource::new(
        resource.identity(),
        resource.captured_bytes() + 1,
        Arc::new(*resource.summary()),
        resource.token().clone(),
    );
    assert!(matches!(
        source.open_resource(&forged_length),
        Err(crate::ResourceError::ForeignResource)
    ));

    let mut altered_summary = *resource.summary();
    altered_summary.min_ts = altered_summary.min_ts.saturating_add(1);
    let forged_summary = SegmentResource::new(
        resource.identity(),
        resource.captured_bytes(),
        Arc::new(altered_summary),
        resource.token().clone(),
    );
    assert!(matches!(
        source.open_resource(&forged_summary),
        Err(crate::ResourceError::ForeignResource)
    ));
}

#[test]
fn embedded_constructor_rejects_invalid_zms_without_deriving_an_id() {
    let error = EmbeddedSource::from_owned(
        SegmentId::new(77).expect("segment id"),
        b"not a ZMS".to_vec(),
        64,
    )
    .expect_err("invalid bytes");
    assert!(matches!(
        error,
        crate::ResourceError::TailIndex(_) | crate::ResourceError::TooSmall
    ));
}

#[test]
fn storage_neutral_api_compiles_without_posix_types() {
    fn assert_api<S>(source: &S)
    where
        S: crate::ResourceCatalog + crate::ImmutableSegmentSource,
    {
        let listing = source.resources().expect("catalog");
        let resource = listing.resources.first().expect("resource");
        let bytes = source.open_resource(resource).expect("bytes");
        drop(read_resource_catalog(&bytes).expect("catalog bytes"));
        source
            .validate_opened(resource, &bytes)
            .expect("opened bytes");
    }

    assert_api(&source(42));
}

#[test]
#[cfg(feature = "posix")]
fn embedded_file_source_reads_the_validated_open_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    std::fs::write(&path, FIXTURE).expect("write fixture");
    let source = EmbeddedSource::from_file(
        SegmentId::new(49).expect("segment id"),
        std::fs::File::open(path).expect("open fixture"),
        FIXTURE_LIMIT,
    )
    .expect("valid file source");

    let listing = source.resources().expect("file catalog");
    let resource = &listing.resources[0];
    assert_eq!(resource.captured_bytes(), FIXTURE_LIMIT);
    let opened = source.open_resource(resource).expect("open file bytes");
    source
        .validate_opened(resource, &opened)
        .expect("file remains unchanged");
    assert_eq!(
        read_resource_catalog(&opened).expect("file catalog bytes"),
        read_resource_catalog(&FIXTURE).expect("fixture catalog")
    );
}

#[test]
#[cfg(feature = "posix")]
fn embedded_file_source_rejects_changes_after_validation() {
    use std::io::Write as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    std::fs::write(&path, FIXTURE).expect("write fixture");
    let source = EmbeddedSource::from_file(
        SegmentId::new(50).expect("segment id"),
        std::fs::File::open(&path).expect("open fixture"),
        FIXTURE_LIMIT,
    )
    .expect("valid file source");
    let listing = source.resources().expect("file catalog");
    let resource = &listing.resources[0];
    let opened = source.open_resource(resource).expect("open file bytes");

    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open fixture for append")
        .write_all(b"changed")
        .expect("append fixture");

    assert!(matches!(
        source.validate_opened(resource, &opened),
        Err(crate::ResourceError::Changed)
    ));
}

#[test]
#[cfg(feature = "posix")]
fn embedded_file_source_rejects_same_length_rewrites() {
    use std::os::unix::fs::FileExt as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    std::fs::write(&path, FIXTURE).expect("write fixture");
    let source = EmbeddedSource::from_file(
        SegmentId::new(51).expect("segment id"),
        std::fs::File::open(&path).expect("open fixture"),
        FIXTURE_LIMIT,
    )
    .expect("valid file source");
    let listing = source.resources().expect("file catalog");
    let resource = &listing.resources[0];
    let opened = source.open_resource(resource).expect("open file bytes");

    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open fixture for rewrite");
    file.write_all_at(b"X", 4).expect("rewrite fixture byte");
    file.sync_all().expect("sync rewritten fixture");

    assert!(matches!(
        source.validate_opened(resource, &opened),
        Err(crate::ResourceError::Changed)
    ));
}
