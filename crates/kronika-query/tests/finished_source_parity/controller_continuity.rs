use super::*;

#[expect(
    dead_code,
    reason = "shared report fixture also provides PostgreSQL and unified cgroup cases"
)]
#[path = "../../../../bins/kronika-report/tests/support/collection_modes.rs"]
mod collection_modes;

#[test]
fn memory_controller_continuity_agrees_in_native_embedded_lanes_and_index() {
    use collection_modes::{Collection, END, START};

    let bytes = collection_modes::encoded(Collection::SeparatedControllers);
    let directory = tempfile::tempdir().expect("recording directory");
    let id = SegmentId::new(START).expect("segment id");
    let path = finished_path(directory.path(), id);
    std::fs::create_dir_all(path.parent().expect("recording day")).expect("create recording day");
    std::fs::write(&path, &bytes).expect("write production fixture");
    let reader = Reader::open(directory.path()).expect("reader");
    let segments = reader.segments(..).expect("segments");
    let reference = segments.segments.first().expect("segment reference");
    let segment = reader.open_segment(reference).expect("segment");
    let index = build_from_reader(&reader, reference, &segment).expect("production index");
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

    let native: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(
        PosixSource::open(directory.path()).expect("native source"),
    ));
    let embedded: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(
        EmbeddedSource::from_owned(id, bytes.clone(), bytes.len() as u64).expect("embedded source"),
    ));
    let request = HourRequest {
        window: Window {
            from: Some(START),
            to: Some(END - 1),
        },
        series: None,
        part: HourPart::Lanes,
        segments: Some(vec![START]),
        active: None,
    };
    let native = hour_bytes(native, request.clone());
    assert_eq!(native, hour_bytes(embedded, request));
    let oom = ndjson(&native)
        .into_iter()
        .filter(|row| row["record"] == "lane" && row["lane"] == "cg_oom")
        .map(|row| row["value"].as_f64())
        .collect::<Vec<_>>();
    assert_eq!(
        oom,
        [None, Some(0.0), Some(0.0), Some(0.0), Some(1.0), None]
    );
}
