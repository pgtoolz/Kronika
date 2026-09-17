use super::*;

#[derive(Debug, PartialEq)]
struct FinishedProductSnapshot {
    identity: ResourceIdentity,
    summary: CatalogSummary,
    kind: SegmentKind,
    captured_bytes: u64,
    window_count: u32,
    sections: Vec<(u32, crate::Section)>,
    topology_rows: Vec<kronika_registry::Row>,
    cpu_rows: Vec<kronika_registry::Row>,
    projected_topology: Vec<(u64, kronika_registry::Row)>,
    model_name: Vec<u8>,
}

fn finished_product_snapshot<S: ImmutableSegmentSource>(
    reader: &FinishedReader<S>,
    model_id: StrId,
) -> FinishedProductSnapshot {
    let listing = reader.resources().expect("discover immutable resources");
    assert!(listing.warnings.is_empty(), "unexpected resource notices");
    assert_eq!(listing.resources.len(), 1, "one immutable resource");
    let resource = &listing.resources[0];
    assert_eq!(
        resource.identity().kind(),
        ResourceKind::FinishedSegment,
        "the immutable source must expose a finished segment"
    );
    let identity = resource.identity();
    let summary = *resource.summary();
    let segment = reader
        .open_segment(resource)
        .expect("open immutable product segment");
    let mut projected_topology = Vec::new();
    segment
        .visit_rows(
            OsTopology::CONTRACT.type_id.get(),
            &["cpu_id", "model_name"],
            0,
            usize::MAX,
            |ordinal, row| {
                projected_topology.push((ordinal, row));
                true
            },
        )
        .expect("project topology rows");
    let dictionary = segment.dictionary().expect("decode product dictionary");
    let model_name = match dictionary.resolve(model_id.get()).expect("model name") {
        Resolved::Str(bytes) => bytes.to_vec(),
        Resolved::Blob(blob) => blob.stored_bytes.to_vec(),
    };
    FinishedProductSnapshot {
        identity,
        summary,
        kind: segment.kind(),
        captured_bytes: segment.captured_bytes(),
        window_count: segment.window_count(),
        sections: segment.sections().collect(),
        topology_rows: segment
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("topology rows"),
        cpu_rows: segment
            .rows(OsCpu::CONTRACT.type_id.get())
            .expect("CPU rows"),
        projected_topology,
        model_name,
    }
}

#[test]
fn finished_sources_match_for_catalog_and_product_reads() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let segment_address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let model_id = append_text_window(
        &mut journal,
        segment_address.id,
        100,
        b"storage-boundary-model",
    );
    append_cpu_window(&mut journal, segment_address.id, 200);
    let written = write_segment(&journal, &owner, segment_address).expect("publish segment");
    journal.reset().expect("leave no active segment");

    let payload =
        std::fs::read(zms_path(directory.path(), segment_address)).expect("read embedded fixture");
    assert_eq!(payload.len() as u64, written.bytes);
    let embedded = EmbeddedSource::from_owned(segment_address.id, payload, written.bytes)
        .expect("embedded source");

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_snapshot = finished_product_snapshot(&FinishedReader::new(posix), model_id);
    let embedded_snapshot = finished_product_snapshot(&FinishedReader::new(embedded), model_id);
    assert_eq!(posix_snapshot, embedded_snapshot);
}

#[test]
fn embedded_product_uses_supplied_identity_and_rejects_foreign_resource() {
    static ZMS: &[u8] = include_bytes!("../../../../kronika-format/tests/fixtures/minimal.zms");
    let first_id = SegmentId::new(42).expect("first id");
    let second_id = SegmentId::new(43).expect("second id");
    let first = FinishedReader::new(
        EmbeddedSource::from_owned(first_id, ZMS.to_vec(), ZMS.len() as u64).expect("first source"),
    );
    let second = FinishedReader::new(
        EmbeddedSource::from_owned(second_id, ZMS.to_vec(), ZMS.len() as u64)
            .expect("second source"),
    );
    let listing = first.resources().expect("first resources");
    assert_eq!(listing.resources[0].identity().segment_id(), first_id);
    assert_eq!(
        first
            .open_segment(&listing.resources[0])
            .expect("first segment")
            .id(),
        42
    );
    let error = second
        .open_segment(&listing.resources[0])
        .expect_err("resource token belongs to its source");
    assert!(matches!(
        error,
        ReaderError::Resource(ResourceError::ForeignResource)
    ));
}

#[derive(Debug)]
struct ChangedAfterCatalogFailure {
    identity: ResourceIdentity,
    summary: CatalogSummary,
}

impl ResourceCatalog for ChangedAfterCatalogFailure {
    type Resource = ();

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError> {
        Ok(ResourceListing {
            resources: vec![SegmentResource::new(
                self.identity,
                0,
                Arc::new(self.summary),
                (),
            )],
            warnings: Vec::new(),
        })
    }
}

impl ImmutableSegmentSource for ChangedAfterCatalogFailure {
    type Bytes = &'static [u8];

    fn open_resource(
        &self,
        _resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, ResourceError> {
        Ok(b"")
    }

    fn validate_opened(
        &self,
        _resource: &SegmentResource<Self::Resource>,
        _bytes: &Self::Bytes,
    ) -> Result<(), ResourceError> {
        Err(ResourceError::Changed)
    }
}

#[test]
fn opened_identity_failure_wins_over_catalog_failure() {
    static ZMS: &[u8] = include_bytes!("../../../../kronika-format/tests/fixtures/minimal.zms");
    let embedded = EmbeddedSource::from_owned(
        SegmentId::new(47).expect("segment id"),
        ZMS.to_vec(),
        ZMS.len() as u64,
    )
    .expect("embedded source");
    let summary = *embedded.resources().expect("resources").resources[0].summary();
    let reader = FinishedReader::new(ChangedAfterCatalogFailure {
        identity: ResourceIdentity::finished(SegmentId::new(47).expect("segment id")),
        summary,
    });
    let listing = reader.resources().expect("resources");

    let error = reader
        .open_segment(&listing.resources[0])
        .expect_err("changed identity must replace the catalog error");
    assert!(matches!(
        error,
        ReaderError::Resource(ResourceError::Changed)
    ));
}

#[derive(Debug)]
struct ReverseCatalog {
    summary: CatalogSummary,
    ids: [i64; 2],
}

impl ResourceCatalog for ReverseCatalog {
    type Resource = i64;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError> {
        let resources = self
            .ids
            .into_iter()
            .map(|raw_id| {
                SegmentResource::new(
                    ResourceIdentity::finished(SegmentId::new(raw_id).expect("segment id")),
                    0,
                    Arc::new(self.summary),
                    raw_id,
                )
            })
            .collect();
        Ok(ResourceListing {
            resources,
            warnings: Vec::new(),
        })
    }
}

#[test]
fn immutable_resources_are_normalized_by_identity() {
    static ZMS: &[u8] = include_bytes!("../../../../kronika-format/tests/fixtures/minimal.zms");
    let embedded = EmbeddedSource::from_owned(
        SegmentId::new(42).expect("segment id"),
        ZMS.to_vec(),
        ZMS.len() as u64,
    )
    .expect("embedded source");
    let summary = *embedded.resources().expect("resources").resources[0].summary();
    let listing = FinishedReader::new(ReverseCatalog {
        summary,
        ids: [2, 1],
    })
    .resources()
    .expect("normalized resources");

    assert_eq!(
        listing
            .resources
            .iter()
            .map(|resource| resource.identity().segment_id().get())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn immutable_resources_reject_duplicate_identities() {
    static ZMS: &[u8] = include_bytes!("../../../../kronika-format/tests/fixtures/minimal.zms");
    let embedded = EmbeddedSource::from_owned(
        SegmentId::new(42).expect("segment id"),
        ZMS.to_vec(),
        ZMS.len() as u64,
    )
    .expect("embedded source");
    let summary = *embedded.resources().expect("resources").resources[0].summary();
    let error = FinishedReader::new(ReverseCatalog {
        summary,
        ids: [1, 1],
    })
    .resources()
    .expect_err("duplicate identity");

    assert!(matches!(
        error,
        ReaderError::Resource(ResourceError::DuplicateIdentity(identity))
            if identity.segment_id().get() == 1
    ));
}
