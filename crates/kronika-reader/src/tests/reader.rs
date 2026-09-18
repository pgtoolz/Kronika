use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::Int32Array;
use kronika_format::{DEFAULT_BLOB_THRESHOLD, DEFAULT_TRUNCATE_LIMIT, DictLimits, Resolved, StrId};
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_topology::OsTopology;
use kronika_registry::{Cell, Section as _, StrId as RegistryStrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use sha2::{Digest as _, Sha256};

use super::{Dictionary, FinishedReader, Reader, ReaderError, Segment, SegmentKind};

use kronika_store::{
    CatalogSummary, EmbeddedSource, ImmutableSegmentSource, PosixSource, ResourceCatalog,
    ResourceError, ResourceIdentity, ResourceKind, ResourceListing, SegmentResource,
};

#[test]
fn only_replaced_io_sources_are_reopenable() {
    assert!(
        ReaderError::Io(std::io::Error::from(std::io::ErrorKind::Interrupted))
            .source_changed_during_read()
    );
    for kind in [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::Interrupted,
        std::io::ErrorKind::UnexpectedEof,
    ] {
        assert!(
            ReaderError::Store(kronika_store::StoreError::Io(std::io::Error::from(kind)))
                .source_changed_during_read(),
            "segment I/O kind {kind:?}"
        );
    }
    for kind in [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::UnexpectedEof,
        std::io::ErrorKind::InvalidData,
    ] {
        assert!(
            !ReaderError::Io(std::io::Error::from(kind)).source_changed_during_read(),
            "directory I/O kind {kind:?}"
        );
    }
    assert!(!ReaderError::Store(kronika_store::StoreError::BadMagic).source_changed_during_read());
    assert!(ReaderError::Resource(ResourceError::Changed).source_changed_during_read());
    assert!(
        ReaderError::Resource(ResourceError::Unavailable(
            kronika_store::ResourceFailureKind::NotFound,
        ))
        .source_changed_during_read()
    );
}

const SEGMENT_ID: i64 = 1_709_164_800_000_000;

fn writer(directory: &tempfile::TempDir) -> WriterOwner {
    DataRoot::open(directory.path())
        .expect("open data root")
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer")
}

fn address(raw_id: i64) -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(raw_id).expect("positive segment id"))
        .expect("segment address")
}

fn zms_path(root: &Path, address: SegmentAddress) -> PathBuf {
    root.join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name())
}

const fn topology(ts: i64, cpu_id: i32, model_name: StrId) -> OsTopology {
    OsTopology {
        ts: Ts(ts),
        cpu_id,
        model_name: RegistryStrId(model_name.get()),
        mhz_max: Some(3_600.0),
        core_id: cpu_id,
        socket_id: 0,
        numa_node: 0,
        scope: 0,
    }
}

fn append_text_window(journal: &mut Journal, segment_id: SegmentId, ts: i64, text: &[u8]) -> StrId {
    let mut interner = Interner::new(DictLimits::default());
    let id = interner.intern(text).expect("intern fixture text");
    let dictionary = dict::encode(interner.window()).expect("encode dictionary delta");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(topology(ts, i32::try_from(ts).unwrap_or(0), id))
        .expect("buffer topology row");
    let part = buffers
        .flush(&dictionary)
        .expect("encode part")
        .expect("part has one row");
    journal
        .append(segment_id, &part)
        .expect("append current window");
    id
}

fn append_cpu_window(journal: &mut Journal, segment_id: SegmentId, ts: i64) {
    let mut buffers = SectionBuffers::new();
    buffers
        .push(OsCpu {
            ts: Ts(ts),
            cpu_id: -1,
            user: 1,
            nice: 0,
            system: 1,
            idle: 1,
            iowait: 0,
            irq: 0,
            softirq: 0,
            steal: 0,
            guest: 0,
            guest_nice: 0,
            scope: 0,
        })
        .expect("buffer CPU row");
    let part = buffers
        .flush(&[])
        .expect("encode CPU part")
        .expect("part has one row");
    journal
        .append(segment_id, &part)
        .expect("append CPU window");
}

fn one_segment(reader: &Reader) -> Segment {
    let listing = reader.segments(..).expect("list segments");
    assert!(listing.warnings.is_empty(), "unexpected warnings");
    assert_eq!(listing.segments.len(), 1, "one logical segment");
    reader
        .open_segment(&listing.segments[0])
        .expect("open logical segment")
}

fn assert_model_names_resolve(segment: &Segment, dictionary: &Dictionary) {
    for row in segment
        .rows(OsTopology::CONTRACT.type_id.get())
        .expect("decode rows")
    {
        let model_name = row.get("model_name");
        assert!(
            matches!(model_name, Some(Cell::StrId(_))),
            "model_name must be a StrId"
        );
        let Some(Cell::StrId(id)) = model_name else {
            continue;
        };
        assert!(
            dictionary.resolve(*id).is_some(),
            "model_name must resolve through the segment dictionary"
        );
    }
}

#[path = "reader/active.rs"]
mod active;
#[path = "reader/dictionary.rs"]
mod dictionary;
#[path = "dictionary_prefix.rs"]
mod dictionary_prefix;
#[path = "reader/discovery.rs"]
mod discovery;
#[path = "reader/selection.rs"]
mod selection;
#[path = "reader/sources.rs"]
mod sources;
