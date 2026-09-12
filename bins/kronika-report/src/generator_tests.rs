use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kronika_layout::SegmentId;
use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};
use kronika_reader::FinishedReader;
use kronika_store::EmbeddedSource;

use super::{
    HtmlReportInput, ReportTimeRange, isolated_index, write_html, write_html_from_file,
    write_html_from_file_with_segment_id,
};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const VISIBLE_TO: i64 = SEGMENT_ID + 1_000_001;
const ZMS: &[u8] = include_bytes!("../tests/fixtures/standalone.zms");
const IDX: &[u8] = include_bytes!("../tests/fixtures/standalone.idx");

#[test]
fn report_range_requires_positive_javascript_safe_microseconds() {
    const MAX_SAFE: i64 = 9_007_199_254_740_991;

    assert_eq!(
        ReportTimeRange::new(1, MAX_SAFE),
        Some(ReportTimeRange {
            from: 1,
            to_exclusive: MAX_SAFE,
        })
    );
    for (from, to_exclusive) in [
        (0, 1),
        (-1, 1),
        (1, 1),
        (2, 1),
        (1, MAX_SAFE + 1),
        (MAX_SAFE, MAX_SAFE + 1),
    ] {
        assert_eq!(ReportTimeRange::new(from, to_exclusive), None);
    }
}

fn script_blocks(html: &str) -> Option<usize> {
    let mut tail = html;
    let mut count = 0;
    while let Some((_head, body)) = tail.split_once("<script>") {
        let (_script, after) = body.split_once("</script>")?;
        count += 1;
        tail = after;
    }
    Some(count)
}

#[test]
fn isolated_builder_produces_the_committed_canonical_index() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("fixture segment id");
    let source = EmbeddedSource::from_owned(segment_id, ZMS.to_vec(), ZMS.len() as u64)
        .expect("embedded fixture");
    let reader = FinishedReader::new(source);
    let listing = reader.resources().expect("fixture resources");
    let (index, configured_sources) =
        isolated_index(&reader, &listing.resources[0]).expect("build isolated index");
    assert_eq!(index, IDX);
    assert_eq!(configured_sources, SOURCE_OS | SOURCE_POSTGRESQL);
    assert_eq!(super::configured_sources([]), SOURCE_OS);
    assert_eq!(
        super::configured_sources([1_001_001]),
        SOURCE_OS | SOURCE_POSTGRESQL
    );
}

#[test]
fn raw_postgresql_sections_enable_postgresql_without_a_health_block() {
    assert_eq!(
        super::configured_sources([1_020_001, 3_001_001]),
        SOURCE_OS | SOURCE_POSTGRESQL
    );
}

#[test]
fn file_and_vec_writers_produce_identical_html() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    std::fs::write(&path, ZMS).expect("write fixture ZMS");

    let mut from_vec = Vec::new();
    write_html(
        HtmlReportInput {
            segment_id: SegmentId::new(SEGMENT_ID).expect("fixture segment id"),
            zms: ZMS.to_vec(),
            max_zms_bytes: ZMS.len() as u64,
            visible_range: ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO)
                .expect("fixture report range"),
        },
        &mut from_vec,
    )
    .expect("write report from Vec");

    let mut from_file = Vec::new();
    let summary = write_html_from_file(
        std::fs::File::open(path).expect("open fixture ZMS"),
        ZMS.len() as u64,
        &mut from_file,
    )
    .expect("write report from file");

    assert_eq!(summary.segment_id.get(), SEGMENT_ID);
    assert_eq!(from_file, from_vec);
}

#[test]
fn file_writer_preserves_an_explicit_identity_distinct_from_the_first_row() {
    let segment_id = SegmentId::new(SEGMENT_ID + 1_000_000).expect("explicit segment id");
    let mut from_vec = Vec::new();
    write_html(
        HtmlReportInput {
            segment_id,
            zms: ZMS.to_vec(),
            max_zms_bytes: ZMS.len() as u64,
            visible_range: ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO)
                .expect("fixture report range"),
        },
        &mut from_vec,
    )
    .expect("write report from Vec");

    let mut file = tempfile::tempfile().expect("temporary ZMS");
    std::io::Write::write_all(&mut file, ZMS).expect("write fixture ZMS");
    let mut from_file = Vec::new();
    let summary = write_html_from_file_with_segment_id(
        segment_id,
        file,
        ZMS.len() as u64,
        ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO).expect("fixture report range"),
        &mut from_file,
    )
    .expect("write report from file with explicit identity");

    assert_eq!(summary.segment_id, segment_id);
    assert_eq!(from_file, from_vec);
}

#[test]
fn file_writer_applies_the_size_limit_before_output() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    std::fs::write(&path, ZMS).expect("write fixture ZMS");
    let mut output = Vec::new();

    let error = write_html_from_file(
        std::fs::File::open(path).expect("open fixture ZMS"),
        ZMS.len() as u64 - 1,
        &mut output,
    )
    .expect_err("file exceeds limit");

    assert!(matches!(
        error,
        super::HtmlReportError::Resource(kronika_store::ResourceError::TooLarge {
            len,
            max
        }) if len == ZMS.len() as u64 && max == ZMS.len() as u64 - 1
    ));
    assert!(output.is_empty());
}

#[test]
fn file_writer_validates_section_checksums_before_output() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incident.zms");
    let mut damaged = ZMS.to_vec();
    damaged[4] ^= 1;
    std::fs::write(&path, damaged).expect("write damaged fixture ZMS");
    let mut output = Vec::new();

    let error = write_html_from_file(
        std::fs::File::open(path).expect("open fixture ZMS"),
        ZMS.len() as u64,
        &mut output,
    )
    .expect_err("section checksum must fail");

    assert!(matches!(
        error,
        super::HtmlReportError::Resource(kronika_store::ResourceError::SectionChecksum { .. })
    ));
    assert!(output.is_empty());
}

#[test]
fn report_is_self_contained_and_deterministic() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("fixture segment id");
    let mut first = Vec::new();
    let mut second = Vec::new();
    for output in [&mut first, &mut second] {
        write_html(
            HtmlReportInput {
                segment_id,
                zms: ZMS.to_vec(),
                max_zms_bytes: ZMS.len() as u64,
                visible_range: ReportTimeRange::new(SEGMENT_ID, VISIBLE_TO)
                    .expect("fixture report range"),
            },
            output,
        )
        .expect("write report");
    }
    assert_eq!(first, second);
    assert!(first.starts_with(b"<!doctype html>"));
    let html = std::str::from_utf8(&first).expect("report HTML is UTF-8");
    assert_eq!(script_blocks(html), Some(2));
    for external in [
        "src=\"http:",
        "src=\"https:",
        "src=\"//",
        "src='http:",
        "src='https:",
        "src='//",
        "href=\"http:",
        "href=\"https:",
        "href=\"//",
        "href='http:",
        "href='https:",
        "href='//",
    ] {
        assert!(!html.contains(external), "external asset {external}");
    }
    assert!(
        !first
            .windows(super::RUNTIME_MARKER.len())
            .any(|bytes| bytes == super::RUNTIME_MARKER)
    );
    assert!(
        first
            .windows(b"__KRONIKA_REPORT_RUNTIME__".len())
            .any(|bytes| bytes == b"__KRONIKA_REPORT_RUNTIME__")
    );
    assert!(html.contains("m=await WebAssembly.compile(r)"));
    assert!(html.contains("await KronikaReportWasm.initEmbedded(m)"));
    assert!(html.contains(&format!(
        "new KronikaReportWasm.ReportSession(\"{SEGMENT_ID}\""
    )));
    assert!(html.contains(&format!("visibleFrom:\"{SEGMENT_ID}\"")));
    assert!(html.contains(&format!("visibleToExclusive:\"{VISIBLE_TO}\"")));
    assert!(!html.contains("KronikaReportWasm.initSync"));
    assert!(!html.contains("new WebAssembly.Module"));
    for encoded in [STANDARD.encode(ZMS), STANDARD.encode(IDX)] {
        assert!(
            first
                .windows(encoded.len())
                .any(|bytes| bytes == encoded.as_bytes()),
            "embedded artifact"
        );
    }
}

#[path = "../tests/support/collection_modes.rs"]
mod collection_modes;

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "encoded collection contracts share one production report assertion path"
)]
fn recorded_collection_modes_generate_matching_report_artifacts() {
    use crate::{ReportEngine, ReportInput};
    use collection_modes::{Collection, END, START};
    use kronika_query::{HourPart, HourRequest, QueryRequest, QuerySink, Window};

    #[derive(Default)]
    struct Records(Vec<u8>);
    impl QuerySink for Records {
        fn record(&mut self, bytes: Vec<u8>) -> bool {
            self.0.extend(bytes);
            true
        }
        fn cancelled(&self) -> bool {
            false
        }
    }
    for (name, collection, sources, expected) in [
        (
            "postgresql-unknown",
            Collection::Postgresql(None),
            SOURCE_POSTGRESQL,
            None,
        ),
        (
            "postgresql-explicit",
            Collection::Postgresql(Some(2)),
            SOURCE_POSTGRESQL,
            Some(80),
        ),
        ("selected-cgroup", Collection::Cgroup, SOURCE_OS, None),
        ("all-cgroups", Collection::AllCgroups, SOURCE_OS, None),
        (
            "all-cgroups-machine",
            Collection::AllCgroupsMachine,
            SOURCE_OS,
            None,
        ),
        (
            "separated-controllers",
            Collection::SeparatedControllers,
            SOURCE_OS,
            None,
        ),
    ] {
        let zms = collection_modes::encoded(collection);
        let segment_id = SegmentId::new(START).expect("segment identity");
        let reader = FinishedReader::new(
            EmbeddedSource::from_owned(segment_id, zms.clone(), zms.len() as u64).expect("source"),
        );
        let resources = reader.resources().expect("resources");
        let (idx, bits) =
            isolated_index(&reader, &resources.resources[0]).expect("production isolated index");
        assert_eq!(bits, sources);
        let mut html = Vec::new();
        let summary = write_html(
            HtmlReportInput {
                segment_id,
                zms: zms.clone(),
                max_zms_bytes: zms.len() as u64,
                visible_range: ReportTimeRange::new(START, END).expect("range"),
            },
            &mut html,
        )
        .expect("production report");
        assert_eq!(summary.configured_sources, sources);
        let html_text = std::str::from_utf8(&html).expect("HTML");
        assert!(html_text.contains(&STANDARD.encode(&idx)));
        assert!(html_text.contains(&STANDARD.encode(&zms)));
        let engine = ReportEngine::new(ReportInput {
            segment_id,
            zms: zms.clone(),
            idx: idx.clone(),
            configured_sources: sources,
            max_zms_bytes: zms.len() as u64,
        })
        .expect("report engine");
        let mut records = Records::default();
        engine
            .execute(
                QueryRequest::Hour(HourRequest {
                    window: Window {
                        from: Some(START),
                        to: Some(END - 1),
                    },
                    series: None,
                    part: HourPart::Lanes,
                    segments: Some(vec![START]),
                    active: None,
                }),
                &mut records,
            )
            .expect("report hour");
        let values = records
            .0
            .split(|byte| *byte == b'\n')
            .filter(|row| !row.is_empty())
            .map(|row| serde_json::from_slice::<serde_json::Value>(row).expect("record"))
            .collect::<Vec<_>>();
        let context = values
            .iter()
            .find(|row| row["record"] == "lane_context")
            .expect("recorded context");
        assert_eq!(context["os_enabled"], sources == SOURCE_OS);
        assert_eq!(context["postgresql_processes_shared"], false);
        if sources == SOURCE_POSTGRESQL {
            let segment = reader
                .open_segment(&resources.resources[0])
                .expect("segment");
            assert!(segment.type_ids().all(|id| {
                !kronika_registry::logical_section_name(id)
                    .is_some_and(|name| name.starts_with("os_"))
            }));
            let index = kronika_index::Index::decode(&idx).expect("decode generated index");
            let mut pg = None;
            let mut overall = None;
            for block in index.blocks {
                match block {
                    kronika_index::SeriesBlock::OsHealth(_) => panic!("PG-only OS Health"),
                    kronika_index::SeriesBlock::PostgresHealth(points) => {
                        pg = Some(points.into_iter().map(|p| p.value).collect::<Vec<_>>());
                    }
                    kronika_index::SeriesBlock::OverallHealth(points) => {
                        overall = Some(points.into_iter().map(|p| p.value).collect::<Vec<_>>());
                    }
                    _ => {}
                }
            }
            assert_eq!(pg, Some(vec![expected]));
            assert_eq!(overall, pg);
        }
        if matches!(
            collection,
            Collection::AllCgroups | Collection::AllCgroupsMachine
        ) {
            let segment = reader
                .open_segment(&resources.resources[0])
                .expect("all-group segment");
            for type_id in [1_206_001, 1_207_001, 1_208_001, 1_209_001, 1_210_001] {
                let expected_rows = if type_id == 1_210_001 { 3164 } else { 1582 };
                assert_eq!(segment.rows_of(type_id), Some(expected_rows));
                assert_eq!(
                    u64::try_from(segment.rows(type_id).expect("decode all groups").len())
                        .expect("row count"),
                    expected_rows
                );
            }
            assert_all_group_snapshots(&engine);
            if matches!(collection, Collection::AllCgroupsMachine) {
                assert_eq!(context["environment"], 0);
                assert!(segment.rows_of(1_205_002).is_none());
                assert!(segment.rows_of(1_201_003).is_none());
            }
        }
        if matches!(collection, Collection::SeparatedControllers) {
            let oom = values
                .iter()
                .filter(|row| row["record"] == "lane" && row["lane"] == "cg_oom")
                .map(|row| row["value"].as_f64())
                .collect::<Vec<_>>();
            assert_eq!(
                oom,
                [None, Some(0.0), Some(0.0), Some(0.0), Some(1.0), None]
            );
            let index = kronika_index::Index::decode(&idx).expect("decode index");
            let hits = index
                .blocks
                .iter()
                .filter_map(|block| match block {
                    kronika_index::SeriesBlock::Findings(block) if block.type_id == 1_202_003 => {
                        Some(&block.findings)
                    }
                    _ => None,
                })
                .flatten()
                .map(|finding| finding.timestamp)
                .collect::<Vec<_>>();
            assert_eq!(hits, [START + 4_000_000]);
        }
        if let Some(output) = std::env::var_os("KRONIKA_REPORT_TEST_OUTPUT") {
            let directory = std::path::PathBuf::from(output).join(name);
            std::fs::create_dir_all(&directory).expect("fixture output directory");
            for (name, bytes) in [
                ("recording.zms", zms),
                ("recording.idx", idx),
                ("report.html", html),
            ] {
                std::fs::write(directory.join(name), bytes).expect("fixture artifact");
            }
            std::fs::write(directory.join("sources"), sources.to_string()).expect("family bits");
        }
    }
}

fn assert_all_group_snapshots(engine: &crate::ReportEngine) {
    use kronika_query::{Order, QueryRequest, QuerySink, SnapshotRequest, StatementScope};

    #[derive(Default)]
    struct Records(Vec<serde_json::Value>);
    impl QuerySink for Records {
        fn record(&mut self, bytes: Vec<u8>) -> bool {
            self.0
                .push(serde_json::from_slice(&bytes).expect("snapshot record"));
            true
        }
        fn cancelled(&self) -> bool {
            false
        }
    }

    for (offset, expected) in [(3, 262), (5, 264)] {
        for section in [
            "os_cgroup_v2_group",
            "os_cgroup_v2_cpu",
            "os_cgroup_v2_memory",
            "os_cgroup_v2_pids",
            "os_cgroup_v2_io",
        ] {
            let mut records = Records::default();
            engine
                .execute(
                    QueryRequest::Snapshot(SnapshotRequest {
                        segment_id: collection_modes::START,
                        at: collection_modes::START + offset * 1_000_000,
                        sections: vec![section.to_owned()],
                        fields: vec!["cgroup_path".to_owned(), "cgroup_identity".to_owned()],
                        by: Vec::new(),
                        direction: Order::Asc,
                        group: None,
                        page_size: None,
                        cursor: None,
                        search: None,
                        first_match: false,
                        text: None,
                        filters: Vec::new(),
                        type_id: None,
                        row_ordinal: None,
                        scope: StatementScope::All,
                    }),
                    &mut records,
                )
                .expect("all-group report snapshot");
            let rows = records
                .0
                .iter()
                .filter(|row| row["record"] == "row")
                .collect::<Vec<_>>();
            let devices = if section == "os_cgroup_v2_io" { 2 } else { 1 };
            assert_eq!(rows.len(), expected * devices, "{section} at +{offset}s");
            for path in [
                "/visible",
                "/visible/api",
                "/visible/database",
                "/visible/jobs",
            ] {
                assert!(rows.iter().any(|row| row["values"][0] == path), "{path}");
            }
            let timestamp = (collection_modes::START + offset * 1_000_000).to_string();
            assert!(rows.iter().all(|row| row["timestamp"] == timestamp));
        }
    }
}
