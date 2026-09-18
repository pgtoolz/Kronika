use std::collections::BTreeMap;

use kronika_format::{DictLimits, StrId as DictStrId};
use kronika_registry::pg_stat_progress_vacuum::{PgStatProgressVacuumV1, PgStatProgressVacuumV3};
use kronika_registry::{PgPreparedXacts, PgSettings, SECTION_WRITE_BATCH_ROWS, Section, StrId, Ts};
use kronika_source_pg::prepared_xacts::PreparedXactsRow;
use kronika_source_pg::progress_vacuum::{
    ProgressVacuumRow, ProgressVacuumV1Row, ProgressVacuumV3Row,
};
use kronika_source_pg::settings::SettingsRow;
use kronika_writer::{Interner, SectionBuffers};

use super::{PgBatch, push_pg_batch};

fn setting(name: &str) -> SettingsRow {
    SettingsRow {
        ts: 7,
        datid: 1,
        datname: "a".to_owned(),
        usesysid: 2,
        usename: "a".to_owned(),
        name: name.to_owned(),
        setting: "a".to_owned(),
        unit: None,
        source: "a".to_owned(),
        sourcefile: None,
        sourceline: None,
        pending_restart: false,
        context: "a".to_owned(),
        vartype: "a".to_owned(),
        boot_val: None,
        reset_val: None,
    }
}

fn prepared(database: &str) -> PreparedXactsRow {
    PreparedXactsRow {
        ts: 7,
        datname: database.to_owned(),
        prepared_count: 1,
        max_age_us: 2,
        max_xid_age_tx: 3,
    }
}

fn section_rows(mut buffers: SectionBuffers) -> BTreeMap<u32, u32> {
    buffers
        .flush_with_summary(&[])
        .expect("encode buffered rows")
        .expect("nonempty buffered prefix")
        .summary
        .sections
        .into_iter()
        .map(|section| (section.type_id, section.rows))
        .collect()
}

#[test]
fn a_settings_batch_does_not_duplicate_or_intern_opening_settings() {
    let mut buffers = SectionBuffers::new();
    let mut interner = Interner::new(DictLimits::default());
    let batch = PgBatch::Settings(vec![setting("batch")].into());

    push_pg_batch(
        &mut buffers,
        &mut interner,
        &batch,
        &[setting("opening-only")],
    )
    .expect("buffer settings batch");

    assert_eq!(
        section_rows(buffers),
        BTreeMap::from([(PgSettings::CONTRACT.type_id.get(), 1)])
    );
    assert!(!interner.is_interned(DictStrId::of(b"opening-only").expect("nonzero string ID")));
}

#[test]
fn dictionary_failure_keeps_the_prefix_and_stops_before_later_rows() {
    for fail_in_opening in [true, false] {
        let mut buffers = SectionBuffers::new();
        let limits = DictLimits::new(1, 1)
            .expect("valid string limits")
            .with_max_total_bytes(1)
            .expect("one string fits");
        let mut interner = Interner::new(limits);
        let opening = if fail_in_opening {
            vec![setting("a"), setting("b"), setting("a")]
        } else {
            vec![setting("a")]
        };
        let batch = PgBatch::PreparedXacts(vec![prepared("a"), prepared("b"), prepared("a")]);

        let error = push_pg_batch(&mut buffers, &mut interner, &batch, &opening)
            .expect_err("dictionary capacity stops the first unencodable row");

        assert!(error.to_string().contains("intern a PostgreSQL string"));
        let mut expected = BTreeMap::from([(PgSettings::CONTRACT.type_id.get(), 1)]);
        if !fail_in_opening {
            expected.insert(PgPreparedXacts::CONTRACT.type_id.get(), 1);
        }
        assert_eq!(section_rows(buffers), expected);
    }
}

#[test]
fn buffer_exhaustion_keeps_the_last_fitting_row_without_converting_the_rest() {
    let mut buffers = SectionBuffers::new();
    let mut interner = Interner::new(DictLimits::default());
    let database = interner.intern(b"a").expect("seed database name");
    let row = PgPreparedXacts {
        ts: Ts(1),
        datname: StrId(database.get()),
        prepared_count: 1,
        max_age_us: 2,
        max_xid_age_tx: 3,
    };
    for _ in 0..SECTION_WRITE_BATCH_ROWS - 1 {
        buffers.push(row).expect("leave capacity for one row");
    }
    let batch = PgBatch::PreparedXacts(vec![
        prepared("first"),
        prepared("blocked"),
        prepared("untouched"),
    ]);

    let error = push_pg_batch(&mut buffers, &mut interner, &batch, &[setting("a")])
        .expect_err("second row exceeds section capacity");

    assert!(error.to_string().contains("section buffer is full"));
    assert!(interner.is_interned(DictStrId::of(b"blocked").expect("nonzero string ID")));
    assert!(!interner.is_interned(DictStrId::of(b"untouched").expect("nonzero string ID")));
    assert_eq!(
        section_rows(buffers),
        BTreeMap::from([
            (PgSettings::CONTRACT.type_id.get(), 1),
            (
                PgPreparedXacts::CONTRACT.type_id.get(),
                u32::try_from(SECTION_WRITE_BATCH_ROWS).expect("row cap fits u32")
            ),
        ])
    );
}

#[test]
fn mixed_vacuum_versions_keep_their_own_section_layouts() {
    let batch = PgBatch::ProgressVacuum(vec![
        ProgressVacuumRow::V1(ProgressVacuumV1Row {
            ts: 7,
            pid: 10,
            datid: 1,
            datname: "db".to_owned(),
            relid: 20,
            schemaname: None,
            relname: None,
            is_autovacuum: false,
            phase: "scanning heap".to_owned(),
            heap_blks_total: 100,
            heap_blks_scanned: 7,
            heap_blks_vacuumed: 0,
            index_vacuum_count: 0,
            max_dead_tuples: 30,
            num_dead_tuples: 3,
        }),
        ProgressVacuumRow::V3(ProgressVacuumV3Row {
            ts: 8,
            pid: 11,
            datid: 1,
            datname: "db".to_owned(),
            relid: 21,
            schemaname: None,
            relname: None,
            is_autovacuum: true,
            phase: "vacuuming indexes".to_owned(),
            heap_blks_total: 100,
            heap_blks_scanned: 100,
            heap_blks_vacuumed: 0,
            index_vacuum_count: 1,
            max_dead_tuple_bytes: 300,
            dead_tuple_bytes: 30,
            num_dead_item_ids: 3,
            indexes_total: 2,
            indexes_processed: 1,
            delay_time: 1.5,
        }),
    ]);
    let mut buffers = SectionBuffers::new();
    let mut interner = Interner::new(DictLimits::default());

    push_pg_batch(&mut buffers, &mut interner, &batch, &[]).expect("buffer mixed versions");

    assert_eq!(
        section_rows(buffers),
        BTreeMap::from([
            (PgStatProgressVacuumV1::CONTRACT.type_id.get(), 1),
            (PgStatProgressVacuumV3::CONTRACT.type_id.get(), 1),
        ])
    );
}
