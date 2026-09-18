use kronika_format::{crc32c, validate_part};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::pg_locks::PgLocksV2;
use kronika_registry::{Bytes, SECTION_WRITE_BATCH_ROWS, StrId, Ts, VerifiedSection, decode_any};

use super::SectionBuffers;

fn loadavg(ts: i64) -> OsLoadavg {
    OsLoadavg {
        ts: Ts(ts),
        load1: 1.5,
        load5: 1.0,
        load15: 0.5,
        running: 2,
        total: 345,
        scope: 0,
    }
}

fn instance(ts: i64) -> InstanceMetadata {
    InstanceMetadata {
        ts: Ts(ts),
        hostname: StrId(1),
        kernel_version: StrId(3),
        environment: Environment::Machine.as_u8(),
        clock_ticks_per_sec: 100,
        page_size_bytes: 4096,
        boot_id: StrId(4),
        btime: Ts(ts - 1_000),
        postgresql_enabled: false,
        postgresql_interval_seconds: 30,
        postgresql_effective_cpus: None,
    }
}

#[test]
fn buffers_many_types_and_flushes_one_part() {
    let mut buffers = SectionBuffers::new();
    assert!(buffers.is_empty());
    buffers.push(loadavg(1_000)).expect("buffer not full");
    buffers.push(loadavg(2_000)).expect("buffer not full");
    buffers.push(instance(1_500)).expect("buffer not full");
    assert!(!buffers.is_empty());

    let part = buffers
        .flush(&[])
        .expect("flush encodes the buffered rows")
        .expect("buffered rows produce a part");
    assert!(buffers.is_empty(), "flush clears the window");

    let catalog = validate_part(&part).expect("the part is a valid container");
    assert_eq!(catalog.entries.len(), 2, "one section per buffered type");
    assert_eq!(
        (catalog.min_ts, catalog.max_ts),
        (1_000, 2_000),
        "time range spans both loadavg rows"
    );

    // Decode through the registry with the production CRC function.
    let decode_rows = |type_id: u32| -> usize {
        let entry = *catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == type_id)
            .expect("the type's entry is present");
        let start = usize::try_from(entry.offset).expect("offset fits usize");
        let len = usize::try_from(entry.len).expect("len fits usize");
        let body = Bytes::copy_from_slice(&part[start..start + len]);
        let verified =
            VerifiedSection::verify(body, entry.crc32c, crc32c).expect("catalog crc matches");
        decode_any(type_id, verified).expect("decode").stats.rows
    };
    assert_eq!(decode_rows(1_105_001), 2);
    assert_eq!(decode_rows(1_021_002), 1);
}

#[test]
fn flush_with_summary_reports_section_rows_and_bytes() {
    let mut buffers = SectionBuffers::new();
    buffers.push(loadavg(1_000)).expect("buffer not full");
    buffers.push(loadavg(2_000)).expect("buffer not full");
    buffers.push(instance(1_500)).expect("buffer not full");

    let flushed = buffers
        .flush_with_summary(&[])
        .expect("flush encodes the buffered rows")
        .expect("buffered rows produce a part");
    assert_eq!(flushed.summary.part_bytes, flushed.body.len());
    assert_eq!(flushed.summary.sections.len(), 2);
    let loadavg = flushed
        .summary
        .sections
        .iter()
        .find(|section| section.type_id == 1_105_001)
        .expect("loadavg section summary");
    assert_eq!(loadavg.rows, 2);
    assert!(loadavg.body_bytes > 0);
    assert_eq!(loadavg.list_i32_child_value_count, 0);
    let instance = flushed
        .summary
        .sections
        .iter()
        .find(|section| section.type_id == 1_021_002)
        .expect("instance section summary");
    assert_eq!(instance.rows, 1);
    assert!(instance.body_bytes > 0);
    assert_eq!(instance.list_i32_child_value_count, 0);
    assert!(buffers.is_empty(), "flush clears the window");
}

fn lock_row(ts: i64, pid: i32, blocked_by: Vec<i32>) -> PgLocksV2 {
    PgLocksV2 {
        ts: Ts(ts),
        pid,
        blocked_by,
        datid: 16_384,
        datname: StrId(1),
        usename: Some(StrId(2)),
        application_name: StrId(3),
        client_addr: StrId(4),
        backend_type: StrId(5),
        state: Some(StrId(6)),
        wait_event_type: None,
        wait_event: None,
        query: StrId(7),
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Some(Ts(ts - 60_000_000)),
        xact_start: Some(Ts(ts - 5_000_000)),
        query_start: Some(Ts(ts - 1_000_000)),
        state_change: Some(Ts(ts - 1_000_000)),
        lock_locktype: None,
        lock_mode: None,
        lock_database: None,
        lock_relation: None,
        lock_relname: None,
        lock_page: None,
        lock_tuple: None,
        lock_virtualxid: None,
        lock_transactionid: None,
        lock_classid: None,
        lock_objid: None,
        lock_objsubid: None,
        lock_target: None,
        waitstart: None,
    }
}

#[test]
fn flush_summary_counts_all_list_i32_child_values() {
    let mut buffers = SectionBuffers::new();
    buffers
        .push(lock_row(1, 10, vec![1, 2, 3, 4]))
        .expect("buffer not full");
    buffers
        .push(lock_row(2, 11, vec![5, 6, 7, 8, 9]))
        .expect("buffer not full");

    let flushed = buffers
        .flush_with_summary(&[])
        .expect("flush encodes rows")
        .expect("rows produce a part");
    assert_eq!(flushed.summary.sections.len(), 1);
    assert_eq!(flushed.summary.sections[0].rows, 2);
    assert_eq!(flushed.summary.sections[0].list_i32_child_value_count, 9);
}

#[test]
fn flushing_an_empty_window_yields_no_part() {
    let mut buffers = SectionBuffers::new();
    assert!(buffers.flush(&[]).expect("flush ok").is_none());
}

#[test]
fn flush_summary_includes_dictionary_sections() {
    let mut buffers = SectionBuffers::new();
    let dict_sections = [crate::dict::DictSection {
        type_id: kronika_registry::DICT_STRINGS_TYPE_ID,
        rows: 3,
        body: vec![1, 2, 3, 4],
    }];

    let flushed = buffers
        .flush_with_summary(&dict_sections)
        .expect("flush ok")
        .expect("dictionary-only part is still written");
    assert_eq!(flushed.summary.sections.len(), 1);
    assert_eq!(
        flushed.summary.sections[0],
        super::SectionFlushSummary {
            type_id: kronika_registry::DICT_STRINGS_TYPE_ID,
            rows: 3,
            body_bytes: 4,
            list_i32_child_value_count: 0,
        }
    );
    assert_eq!(flushed.summary.part_bytes, flushed.body.len());
}

#[test]
fn push_bounces_a_row_when_the_type_buffer_is_full() {
    let mut buffers = SectionBuffers::new();
    for _ in 0..SECTION_WRITE_BATCH_ROWS {
        buffers.push(loadavg(0)).expect("under the cap");
    }
    // A full buffer holds one section's worth; the next row comes back for
    // the caller to flush and retry, so memory stays bounded before a flush.
    assert!(buffers.push(loadavg(0)).is_err());
}
