use super::*;
use kronika_registry::Section;
use kronika_registry::instance_metadata::InstanceMetadataV4;

fn context_with_cadence_metadata(
    metadata: impl Section + 'static,
) -> (tempfile::TempDir, QueryContext) {
    let directory = tempfile::tempdir().expect("temporary fixture root");
    write_heatmap_fixture_with_sharing(
        directory.path(),
        SegmentId::new(SEGMENT_ID).expect("sample segment id"),
        Some(true),
        42,
    );
    write_cadence_metadata(directory.path(), HEATMAP_TO + 1, metadata);
    let source = PosixSource::open(directory.path()).expect("source");
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
    (directory, context)
}

fn write_cadence_metadata(path: &Path, timestamp: i64, metadata: impl Section + 'static) {
    let root = DataRoot::open(path).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let interner = Interner::new(DictLimits::default());
    let dictionary = dict::encode(interner.window()).expect("empty dictionary");
    let mut buffers = SectionBuffers::new();
    assert!(buffers.push(metadata).is_ok(), "metadata fits");
    let part = buffers
        .flush(&dictionary)
        .expect("encode")
        .expect("metadata part");
    let id = SegmentId::new(timestamp).expect("metadata segment id");
    journal.append(id, &part).expect("append metadata");
    write_segment(&journal, &writer, SegmentAddress::new(id).expect("address"))
        .expect("finish metadata segment");
    journal.reset().expect("clear journal");
}

fn assert_finder_expiry(context: &QueryContext, surface: FinderSurface, max_age: i64) {
    for (age, expected_as_of) in [(max_age, Some(HEATMAP_TO)), (max_age + 1, None)] {
        let query = FinderQuery {
            point: SnapshotPoint::At(HEATMAP_TO + age),
            ..finder_query(surface)
        };
        let (row_count, as_of) = match surface {
            FinderSurface::Processes => {
                let result = execute_processes(context, &query, &|| false).expect("process finder");
                (result.rows.len(), result.as_of)
            }
            FinderSurface::Tables | FinderSurface::Indexes => {
                let result = execute_relation(context, &query, &|| false).expect("relation finder");
                (result.rows.len(), result.as_of)
            }
            _ => {
                let result = execute_plain(context, &query, &|| false).expect("plain finder");
                (result.rows.len(), result.as_of)
            }
        };
        assert_eq!(as_of, expected_as_of, "{surface:?} at age {age}");
        assert_eq!(
            row_count > 0,
            expected_as_of.is_some(),
            "{surface:?} at age {age}"
        );
    }
}

const fn family_metadata() -> InstanceMetadataV4 {
    InstanceMetadataV4 {
        ts: Ts(HEATMAP_TO + 1),
        hostname: None,
        kernel_version: None,
        environment: None,
        clock_ticks_per_sec: None,
        page_size_bytes: None,
        boot_id: None,
        btime: None,
        os_enabled: true,
        postgresql_processes_shared: true,
        postgresql_enabled: true,
        postgresql_interval_seconds: 10,
        postgresql_effective_cpus: Some(4),
        postgresql_instance_interval_seconds: 90,
        postgresql_relations_interval_seconds: 480,
        postgresql_statements_interval_seconds: 600,
    }
}

#[test]
fn typed_finders_use_recorded_family_cadences() {
    let (_directory, context) = context_with_cadence_metadata(family_metadata());
    for (surface, max_age) in [
        (FinderSurface::Processes, 20_000_000),
        (FinderSurface::Activity, 25_000_000),
        (FinderSurface::Locks, 25_000_000),
        (FinderSurface::Vacuum, 25_000_000),
        (FinderSurface::Databases, 225_000_000),
        (FinderSurface::Tables, 1_200_000_000),
        (FinderSurface::Indexes, 1_200_000_000),
        (FinderSurface::Statements, 1_500_000_000),
        (FinderSurface::Plans, 1_500_000_000),
    ] {
        assert_finder_expiry(&context, surface, max_age);
    }
}

const fn legacy_metadata() -> InstanceMetadataV3 {
    InstanceMetadataV3 {
        ts: Ts(HEATMAP_TO + 1),
        hostname: None,
        kernel_version: None,
        environment: None,
        clock_ticks_per_sec: None,
        page_size_bytes: None,
        boot_id: None,
        btime: None,
        os_enabled: true,
        postgresql_processes_shared: true,
        postgresql_enabled: true,
        postgresql_interval_seconds: 45,
        postgresql_effective_cpus: Some(4),
    }
}

#[test]
fn typed_finders_preserve_legacy_cadence_fallbacks() {
    let (_directory, context) = context_with_cadence_metadata(legacy_metadata());
    for surface in [
        FinderSurface::Activity,
        FinderSurface::Locks,
        FinderSurface::Vacuum,
        FinderSurface::Databases,
        FinderSurface::Statements,
        FinderSurface::Plans,
    ] {
        assert_finder_expiry(&context, surface, 112_500_000);
    }
    for surface in [FinderSurface::Tables, FinderSurface::Indexes] {
        assert_finder_expiry(&context, surface, 750_000_000);
    }
}

#[test]
fn newer_legacy_metadata_restores_relation_cadence_after_downgrade() {
    let (directory, _context) = context_with_cadence_metadata(family_metadata());
    write_cadence_metadata(
        directory.path(),
        HEATMAP_TO + 2,
        InstanceMetadataV3 {
            ts: Ts(HEATMAP_TO + 2),
            ..legacy_metadata()
        },
    );
    let source = PosixSource::open(directory.path()).expect("source after downgrade");
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
    for surface in [FinderSurface::Tables, FinderSurface::Indexes] {
        assert_finder_expiry(&context, surface, 750_000_000);
    }
}
