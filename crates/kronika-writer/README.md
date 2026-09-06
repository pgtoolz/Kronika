# kronika-writer

[Русская версия](README.ru.md)

`kronika-writer` turns one or more bounded collection windows into a durable
ZMS segment. It maintains in-memory section buffers, per-segment string
interning (replacing repeated strings with dictionary references), the version-1 `active.wal` journal, recovery, and finished-segment
publication. Other crates handle source queries, format bytes, and the
data-directory grammar.

## Collection window

`SectionBuffers::push<T: Section>` stores rows by registered type. A type buffer
stops at `SECTION_WRITE_BATCH_ROWS` and returns the rejected row so the caller
can flush and retry without loss. `flush` encodes data sections in type-id order, appends
dictionary sections, derives the catalog time range, and returns one ZMS part.
A successful flush empties the row buffers.

`dict::encode` converts the current interner window into sorted
`dict.strings` and `dict.blobs` sections. Snapshot rows refer to those values by
`str_id`.

## Interner

`Interner` maps each value to a dictionary identifier within one open segment.
Data rows store that identifier instead of repeating the value. The current
window keeps full stored bytes under `DictLimits`. After the caller successfully
writes a window, `flush_window` replaces those bytes with compact metadata for
collision detection, deduplication, and final placement. Flushed value bytes do
not remain in memory until the segment is written.

Interning is transactional on collision, placement conflict, or byte-cap
failure: prior state remains valid. The caller writes or flushes when it receives
`DictError::Full`.

## Journal

`Journal::open(&WriterOwner, config)` opens the root-level `active.wal`
through a capability from `kronika-layout`. A new journal is initialized as a
durable 36-byte version-1 empty header; it is never represented by a zero-length
file.

Journal version 1 uses the magic `KRNJNL1\0`. Its checksummed header records
whether the journal is empty or active, the active [`SegmentId`][layout], and
the exact number of following frame bytes. `append(segment_id, part)` validates
the ZMS part and writes its `ZMSP` frame. The first append makes the segment id
and first frame durable at the same synchronization boundary. Later appends
must use the same id.

`Journal::open` validates the complete header and frame body without loading the
whole file. It rejects a headerless, differently versioned, torn, or damaged
journal and leaves it unchanged. It initializes a zero-length file with the
empty header.

`JournalConfig::max_journal_len` caps the physical file, including the
temporary 32-byte reset marker. Every append, including the first one, reserves
space for that marker. A frame that would exceed the cap returns
`JournalError::Full`, allowing the collector to write first. Version 1 admits at
most 1 GiB per journal, 1,000,000 frames per journal, and 64 MiB per ZMS part.
Configuration may only lower those absolute limits.

`reset` is valid only after successful segment publication. It first appends
and synchronizes a marker containing the pre-reset length, `SegmentId`, and
header checksum. It then writes `JournalHeader::EMPTY` and calls `sync_data`
while the marker and frame body are still present. Only after that
synchronization does it truncate the file to 36 bytes and call `sync_data` a
second time. If the process exits after committing the marker, the next
`Journal::open` validates that marker and completes the reset. A failed rollback
or a failure after marker commit poisons the open journal: every further
operation fails, the daemon exits, and the next open completes the reset from
the committed marker.

## Writing

`write_segment(journal, owner, SegmentAddress)` validates every recorded part
and reads each body through its checked catalog range. For each registered data
`type_id`, it decodes all journal bodies, combines their rows, applies the
registry sort key plus every remaining column as a deterministic total order,
and emits one canonical Parquet body. Dictionary bodies are decoded and
normalized into at most one `dict.strings` and one `dict.blobs` body. Exact
repeated dictionary records are deduplicated; conflicting values, metadata, or
placement fail the write.

Final bodies use Parquet 1.0, PLAIN values, RLE levels, and Zstd level 6, with
dictionary encoding, statistics, and offset indexes disabled. Collection
windows are appended without a projected final-section admission check.
`write_segment` validates catalog field widths, decoded work and arithmetic,
allows multiple Parquet pages, and checks the actual body against the 1 GiB
version-1 envelope.

`write_segment` writes the coalesced bodies and end catalog to a temporary file
in the segment's UTC day. ZMS publication synchronizes the file, adds the
canonical `YYYY/MM/DD/N.zms` name with a hard link, synchronizes the day,
removes the temporary name, and synchronizes the day again. An existing
destination is never overwritten. Recovery accepts an existing ZMS only if it
passes structural validation and is byte-identical to the generated segment.

After acquiring the writer owner lock, collector startup removes only
recognized stale ZMS publication temporaries. It leaves IDX and index-probe
temporaries to the index owner.

`write_segment` does not reset the journal, choose the `SegmentId`, or implement
retention; the collector performs those operations.
`SegmentAddress` derives the only valid path from the id, and the writer
accepts only that strict calendar-tree address.

The API reports journal I/O, framing, and capacity errors separately from write
validation, destination, and synchronization errors. See
[`src/lib.rs`](src/lib.rs) for the API,
[`../kronika-format/`](../kronika-format/) for on-disk framing, and
[`../kronika-layout/`](../kronika-layout/) for paths and ownership.

[layout]: ../kronika-layout/src/time.rs
