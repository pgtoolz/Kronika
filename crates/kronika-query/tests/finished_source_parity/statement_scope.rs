use super::*;
use kronika_registry::pg_store_plans::PgStorePlansVadvV1;

#[test]
fn workload_summary_filters_plans_by_statement_identity_in_native_and_embedded_sources() {
    let directory = tempfile::tempdir().expect("scope directory");
    let segment_id = SegmentId::new(SEGMENT_ID).expect("scope segment");
    let payload = write_scope_fixture(directory.path(), segment_id);
    let native: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(
        PosixSource::open(directory.path()).expect("native scope source"),
    ));
    let embedded: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(
        EmbeddedSource::from_owned(segment_id, payload.to_vec(), payload.len() as u64)
            .expect("embedded scope source"),
    ));
    for scope in [StatementScope::All, StatementScope::Workload] {
        let mut request = series_hour_request(
            Window {
                from: Some(HEATMAP_FROM),
                to: Some(HEATMAP_TO),
            },
            "postgresql_summary",
            Vec::new(),
            Vec::new(),
            None,
        );
        request.series.as_mut().expect("summary series").scope = scope;
        let bytes = hour_bytes(Arc::clone(&native), request.clone());
        assert_eq!(bytes, hour_bytes(Arc::clone(&embedded), request));
        let records = ndjson(&bytes);
        let timestamp = HEATMAP_TO.to_string();
        let current: Vec<_> = records
            .iter()
            .filter(|record| {
                record["record"] == "row"
                    && record["timestamp"].as_str() == Some(timestamp.as_str())
            })
            .collect();
        let active = |surface| {
            current
                .iter()
                .find(|record| record["values"][0] == surface)
                .expect("summary surface")["values"][1]
                .clone()
        };
        match scope {
            StatementScope::All => {
                assert_eq!(active(1), serde_json::json!(2.0));
                assert_eq!(active(2), serde_json::json!(9.0));
            }
            StatementScope::Workload => {
                assert_eq!(active(1), serde_json::json!(1.0));
                assert_eq!(active(2), serde_json::json!(7.0));
            }
        }
    }
}

fn write_scope_fixture(root: &Path, segment_id: SegmentId) -> Arc<[u8]> {
    let owner = DataRoot::open(root)
        .expect("scope data root")
        .acquire_writer(LayoutLimits::default())
        .expect("scope writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("scope journal");
    let mut interner = Interner::new(DictLimits::default());
    let collector = fixture_label(&mut interner, b"/* kronika: collector */ select 1");
    let application = fixture_label(&mut interner, b"select * from application");
    let plan_text = fixture_label(&mut interner, b"unrelated plan text");
    let mut buffers = SectionBuffers::new();
    for (timestamp, calls) in [(HEATMAP_FROM, 10), (HEATMAP_TO, 20)] {
        buffers
            .push(parity_statement(timestamp, collector, calls))
            .expect("collector statement");
        let mut user = parity_statement(timestamp, application, calls);
        user.queryid = Some(72);
        buffers.push(user).expect("application statement");
        for (queryid, dbid, userid) in [
            (71, 1, 72),
            (72, 1, 72),
            (71, 2, 72),
            (71, 1, 73),
            (0, 1, 72),
            (999, 1, 72),
        ] {
            let mut plan = parity_plan(timestamp, plan_text, calls);
            plan.queryid = queryid;
            plan.dbid = dbid;
            plan.userid = userid;
            buffers.push(plan).expect("OSSC plan");
        }
        for (queryid, statement_id, planid) in [(999, 71, 1), (71, 72, 2), (71, 0, 3)] {
            let mut plan = vadv_plan(timestamp, plan_text);
            plan.queryid = queryid;
            plan.queryid_stat_statements = statement_id;
            plan.planid = planid;
            plan.calls = calls;
            buffers.push(plan).expect("vadv plan");
        }
    }
    let dictionary = dict::encode(interner.window()).expect("scope dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("scope part")
        .expect("nonempty scope part");
    journal
        .append(segment_id, &part)
        .expect("append scope part");
    write_segment(
        &journal,
        &owner,
        SegmentAddress::new(segment_id).expect("scope address"),
    )
    .expect("write scope segment");
    std::fs::read(finished_path(root, segment_id))
        .expect("scope payload")
        .into()
}

const fn vadv_plan(ts: i64, plan: StrId) -> PgStorePlansVadvV1 {
    PgStorePlansVadvV1 {
        ts: Ts(ts),
        userid: 72,
        dbid: 1,
        queryid: 1,
        planid: -7,
        queryid_stat_statements: 1,
        datname: None,
        usename: None,
        plan: Some(plan),
        calls: 0,
        slow_log_calls: 0,
        total_time: 0.0,
        min_time: 1.0,
        max_time: 2.0,
        mean_time: 1.5,
        stddev_time: 0.5,
        rows: 0,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        first_call: Ts(50),
        last_call: Ts(ts),
        total_plan_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
    }
}
