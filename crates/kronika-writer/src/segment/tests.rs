mod content;
mod dictionary;
mod publish;
mod standalone;

use std::fs::FileTimes;
use std::os::unix::fs::FileExt as _;

use kronika_format::{
    BlobEntry, DictLimits, Entry, MAGIC, Resolved, StrId as DictStrId, crc32c, validate_part,
};
use kronika_layout::{
    ACTIVE_JOURNAL_NAME, DataRoot, FileIdentity, LayoutLimits, SegmentAddress, SegmentId,
    WriterOwner,
};
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_topology::OsTopology;
use kronika_registry::{
    Bytes, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, Section, StrId, Ts, VerifiedSection,
    decode_any,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use super::dictionary::{required_binary, required_u64};
use super::{
    FinishedDictionary, FinishedSection, FinishedZmsPlan, MAX_CATALOG_ENTRIES, WriteError,
    arm_after_first_comparison_chunk, checked_catalog_entries, write_finished_zms, write_segment,
};
use crate::{Interner, Journal, JournalConfig, SectionBuffers, dict};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;

fn writer(directory: &tempfile::TempDir) -> WriterOwner {
    DataRoot::open(directory.path())
        .unwrap()
        .acquire_writer(LayoutLimits::default())
        .unwrap()
}

fn address() -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(SEGMENT_ID).unwrap()).unwrap()
}

fn topology(ts: i64) -> OsTopology {
    OsTopology {
        ts: Ts(ts),
        cpu_id: 0,
        model_name: StrId(1),
        mhz_max: Some(3_600.0),
        core_id: 0,
        socket_id: 0,
        numa_node: 0,
        scope: 0,
    }
}

/// One collection window: buffer a topology row and append its part.
fn append_window(journal: &mut Journal, ts: i64) {
    let mut buffers = SectionBuffers::new();
    buffers.push(topology(ts)).expect("buffer not full");
    let part = buffers.flush(&[]).expect("encode").expect("a part");
    journal
        .append(address().id, &part)
        .expect("append under the segment identity");
}

#[derive(Clone, Copy)]
struct FixtureTextIds {
    model_name: StrId,
    comm: StrId,
    cmdline: StrId,
}

fn fixture_dictionary() -> (Interner, FixtureTextIds) {
    let mut interner =
        Interner::new(DictLimits::new(4_096, 4_096).expect("small dictionary limits"));
    let mut intern = |bytes: &[u8]| StrId(interner.intern(bytes).expect("fixture text fits").get());
    let ids = FixtureTextIds {
        model_name: intern(b"AMD EPYC 7763"),
        comm: intern(b"postgres"),
        cmdline: intern(b"postgres -D /var/lib/pgsql/data"),
    };
    (interner, ids)
}

/// A topology row whose nullable `mhz_max` carries an exact bit pattern, or
/// nothing at all when `present` is false.
fn lossless_topology(ids: FixtureTextIds, mhz_bits: u64, present: bool) -> OsTopology {
    OsTopology {
        ts: Ts(42),
        cpu_id: i32::try_from(mhz_bits & 0xff).expect("byte fits i32"),
        model_name: ids.model_name,
        mhz_max: present.then(|| f64::from_bits(mhz_bits)),
        core_id: 0,
        socket_id: 0,
        numa_node: 0,
        scope: 0,
    }
}

/// A process row; `io` toggles the nullable `/proc/PID/io` block and
/// `cmdline` the nullable dictionary reference.
fn lossless_process(ids: FixtureTextIds, pid: i32, io: bool) -> OsProcess {
    OsProcess {
        ts: Ts(84),
        pid,
        starttime: Ts(1_700_000_000_000_000 + i64::from(pid)),
        ppid: 1,
        uid: 26,
        euid: 26,
        gid: 26,
        egid: 26,
        state: b'S',
        num_threads: 3,
        tty: 0,
        comm: ids.comm,
        cmdline: io.then_some(ids.cmdline),
        utime: 100,
        stime: 50,
        nice: 0,
        prio: 20,
        rtprio: 0,
        policy: 0,
        curcpu: 2,
        rundelay_ns: 1_234,
        blkdelay_ticks: 5,
        nvcsw: 9,
        nivcsw: 1,
        minflt: 77,
        majflt: 3,
        vmem_kb: 2_048,
        rmem_kb: 1_024,
        vswap_kb: 0,
        syscr: io.then_some(1),
        syscw: io.then_some(2),
        rchar: io.then_some(3),
        wchar: io.then_some(4),
        read_bytes: io.then_some(5),
        write_bytes: io.then_some(6),
        cancelled_write_bytes: io.then_some(7),
        exit_signal: 17,
        scope: 0,
    }
}

fn append_lossless_part(
    journal: &mut Journal,
    topology_rows: &[OsTopology],
    process_rows: &[OsProcess],
) {
    let (interner, _ids) = fixture_dictionary();
    let dictionary = dict::encode(interner.window()).expect("encode fixture dictionary");
    let mut buffers = SectionBuffers::new();
    for &row in topology_rows {
        buffers.push(row).expect("topology row fits");
    }
    for &row in process_rows {
        buffers.push(row).expect("process row fits");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode fixture part")
        .expect("fixture part has rows");
    journal
        .append(address().id, &part)
        .expect("append fixture part");
}

fn verified_section(segment: &[u8], entry: &Entry) -> VerifiedSection {
    let start = usize::try_from(entry.offset).expect("fixture offset fits");
    let len = usize::try_from(entry.len).expect("fixture length fits");
    VerifiedSection::verify(
        Bytes::copy_from_slice(&segment[start..start + len]),
        entry.crc32c,
        crc32c,
    )
    .expect("fixture section CRC matches")
}

fn decode_string_dictionary(section: VerifiedSection) -> Vec<(u64, Vec<u8>)> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(section.into_bytes())
        .expect("open string dictionary")
        .build()
        .expect("build string dictionary reader");
    let mut entries = Vec::new();
    for batch in reader {
        let batch = batch.expect("decode string dictionary batch");
        let ids = required_u64(&batch, "str_id").expect("dictionary ids");
        let bytes = required_binary(&batch, "bytes").expect("dictionary bytes");
        entries
            .extend((0..batch.num_rows()).map(|row| (ids.value(row), bytes.value(row).to_vec())));
    }
    entries
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end assertion keeps the two equivalent journals and all lossless fields together"
)]
fn writing_is_lossless_across_journal_order_and_partitioning() {
    const NAN_1: u64 = 0x7ff8_0000_0000_0001;
    const NAN_2: u64 = 0x7ff8_0000_0000_0002;
    const NEGATIVE_ZERO: u64 = (-0.0_f64).to_bits();
    const POSITIVE_ZERO: u64 = 0.0_f64.to_bits();
    // Distinct cpu_id so the absent-value row sorts after the others.
    const ABSENT_MHZ: u64 = 0x0000_0000_0000_00ff;

    let first_dir = tempfile::tempdir().expect("first tempdir");
    let second_dir = tempfile::tempdir().expect("second tempdir");
    let first_owner = writer(&first_dir);
    let second_owner = writer(&second_dir);
    let mut first =
        Journal::open(&first_owner, JournalConfig::default()).expect("open first journal");
    let mut second =
        Journal::open(&second_owner, JournalConfig::default()).expect("open second journal");
    let (_interner, ids) = fixture_dictionary();

    let nan_1 = lossless_topology(ids, NAN_1, true);
    let nan_2 = lossless_topology(ids, NAN_2, true);
    let negative_zero = lossless_topology(ids, NEGATIVE_ZERO, true);
    let positive_zero = lossless_topology(ids, POSITIVE_ZERO, true);
    let absent_mhz = lossless_topology(ids, ABSENT_MHZ, false);
    let bare = lossless_process(ids, 10, false);
    let full = lossless_process(ids, 20, true);

    append_lossless_part(&mut first, &[nan_2, negative_zero], &[full]);
    append_lossless_part(
        &mut first,
        &[positive_zero, nan_1, negative_zero, absent_mhz],
        &[bare, full],
    );

    append_lossless_part(
        &mut second,
        &[negative_zero, nan_1, positive_zero, absent_mhz],
        &[full, bare],
    );
    append_lossless_part(&mut second, &[nan_2, negative_zero], &[full]);
    assert_ne!(
        std::fs::read(first_dir.path().join(ACTIVE_JOURNAL_NAME)).expect("read first journal"),
        std::fs::read(second_dir.path().join(ACTIVE_JOURNAL_NAME)).expect("read second journal"),
        "the equivalent input uses different order and part boundaries"
    );

    let first_path = first_owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let second_path = second_owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let first_summary =
        write_segment(&first, &first_owner, address()).expect("write first journal");
    let second_summary =
        write_segment(&second, &second_owner, address()).expect("write second journal");
    let segment = std::fs::read(&first_path).expect("read first segment");
    assert_eq!(first_summary, second_summary);
    assert_eq!(
        segment,
        std::fs::read(&second_path).expect("read second segment"),
        "equivalent journals must produce exact ZMS bytes"
    );
    assert_eq!((first_summary.min_ts, first_summary.max_ts), (42, 84));

    let catalog = validate_part(&segment).expect("finished segment validates");
    assert_eq!(catalog.window_count, 2);
    let topology_entry = catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == OsTopology::CONTRACT.type_id.get())
        .expect("topology section");
    let decoded_topology = OsTopology::decode(verified_section(&segment, topology_entry))
        .expect("decode topology rows");
    assert_eq!(
        decoded_topology
            .iter()
            .map(|row| row.mhz_max.map(f64::to_bits))
            .collect::<Vec<_>>(),
        [
            Some(NEGATIVE_ZERO),
            Some(NEGATIVE_ZERO),
            Some(POSITIVE_ZERO),
            Some(NAN_1),
            Some(NAN_2),
            None,
        ],
        "raw float bits, canonical order, and duplicate rows survive writing"
    );

    let process_entry = catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == OsProcess::CONTRACT.type_id.get())
        .expect("process section");
    let decoded_processes =
        OsProcess::decode(verified_section(&segment, process_entry)).expect("decode process rows");
    assert_eq!(
        decoded_processes,
        [bare, full, full],
        "canonical rows retain the duplicate process observation"
    );
    assert_eq!(decoded_processes[0].cmdline, None);
    assert_eq!(decoded_processes[0].read_bytes, None);
    assert_eq!(decoded_processes[1].cmdline, Some(ids.cmdline));
    assert_eq!(decoded_processes[1].read_bytes, Some(5));

    let dictionary_entry = catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
        .expect("string dictionary section");
    let dictionary = decode_string_dictionary(verified_section(&segment, dictionary_entry));
    let mut expected_dictionary = vec![
        (ids.model_name.0, b"AMD EPYC 7763".to_vec()),
        (ids.comm.0, b"postgres".to_vec()),
        (ids.cmdline.0, b"postgres -D /var/lib/pgsql/data".to_vec()),
    ];
    expected_dictionary.sort_unstable_by_key(|entry| entry.0);
    assert_eq!(dictionary, expected_dictionary);
}
