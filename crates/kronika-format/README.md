# kronika-format

[Русская версия](README.ru.md)

`kronika-format` defines the binary contract for writing and reading Kronika
segments. It is not a standalone converter or a generic analytics format.

Data moves through these components:

```text
Linux / cgroup / PostgreSQL / PostgreSQL log
        |
        v
kronika-collector collects one window
        |
        v
kronika-registry encodes typed rows as section bodies
        |
        v
kronika-writer builds a ZMS part
        |
        v
$KRONIKA_STORAGE_DIR/active.wal
        |
        | write_segment()
        v
$KRONIKA_STORAGE_DIR/YYYY/MM/DD/N.zms
        |
        v
kronika-store / kronika-reader
        |
        v
kronika-web
```

The journal is always `$KRONIKA_STORAGE_DIR/active.wal`. Finished ZMS files use
the strict UTC calendar tree:

```text
$KRONIKA_STORAGE_DIR/
|-- active.wal
`-- YYYY/
    `-- MM/
        `-- DD/
            `-- N.zms
```

`N` is the decimal [`SegmentId`](../kronika-layout/src/time.rs): Unix
microseconds of the first collection window successfully appended to the
segment. `YYYY/MM/DD` is the UTC day derived from that id.

There are no separate files for string values. While a segment is open,
dictionary bodies live in the ZMS parts inside `active.wal`. On completion,
the writer decodes and normalizes them into at most one `dict.strings` and one
`dict.blobs` body in the finished `.zms`.

The format crate defines the `ZMS1` and `ZMSP` byte layouts, the end catalog,
CRC32C checksums, `StrId`, and the bounded dictionary model. Other crates
handle:

- Linux and PostgreSQL semantics and section schemas: those belong to
  [`kronika-registry`](../kronika-registry/src/lib.rs);
- Parquet encoding, buffering, journal I/O, and writing: those belong to
  [`kronika-writer`](../kronika-writer/README.md);
- collection intervals, rotation, and source limits: those belong to
  [`kronika-collector`](../../bins/kronika-collector/README.md);
- data-directory paths, strict discovery, and ownership:
  [`kronika-layout`](../kronika-layout/);
- typed queries, HTTP, retention, encryption, or remote storage.

## From collection windows to a segment

The collector keeps one string dictionary (interner) for the open segment and creates fresh
row buffers for each non-empty collection cycle. The registry sorts each
snapshot section by its contract key and encodes it. The writer adds the
current window's dictionary records, builds a ZMS part, wraps it in a `ZMSP`
frame, appends it to `active.wal`, and calls `sync_data`. After a successful
append, the interner keeps compact identity metadata for values already in the
journal instead of writing those values again in later windows.

The collector appends windows until size, age, forced-rotation, or journal
capacity closes the segment. `write_segment` then:

1. validates each recorded part catalog and every body range it reads;
2. groups registered data sections by `type_id`, decodes their Parquet bodies,
   and combines their rows;
3. sorts each combined data section by its registry key and then every
   remaining column, producing a deterministic total order;
4. normalizes dictionary records by `str_id`, deduplicating exact repeats and
   rejecting conflicting values, metadata, or placement;
5. re-encodes each populated type as one canonical Parquet body and writes a
   packed ZMS with one end catalog;
6. synchronizes an invocation-owned sibling temporary file and publishes it
   with a no-replace hard link.

The collector resets `active.wal` only after publication succeeds. Writing
removes per-window Parquet framing and repeated catalog entries. It decodes and
re-encodes journal bodies instead of copying them into the finished ZMS.

### Windows, parts, sections, and bodies

The format uses four levels:

- a **collection window** is one non-empty collector cycle;
- a **ZMS part** is one framed window record with its own `ZMS1`, bodies,
  catalog, and tail index;
- a **section** is one catalog entry: `type_id`, offset, length, row count, and
  CRC32C;
- a **section body** is the byte range addressed by a catalog entry. The
  writer places one self-contained Parquet file in that range.

While the segment is open, every part is wrapped in its own journal frame:

```text
active.wal
|
+-- ZMSP frame #1
|   `-- ZMS part for window #1
|       |-- "ZMS1"
|       |-- data section bodies
|       |-- new dict.strings / dict.blobs records
|       |-- part catalog
|       `-- tail index
|
`-- ZMSP frame #2
    `-- ZMS part for window #2
        |-- "ZMS1"
        |-- data section bodies
        |-- new dict.strings / dict.blobs records
        |-- part catalog
        `-- tail index
```

Within each part, the writer places non-empty data bodies in ascending
`type_id` order, followed by `dict.strings` and then `dict.blobs`. An absent
dictionary section occupies no space. The same canonical inventory applies to
the finished ZMS: every populated type occurs once, data types are ascending,
and the two optional dictionaries form the tail. Readers locate every body
through the catalog.

Completing the segment removes the individual part framing:

```text
active.wal                         YYYY/MM/DD/N.zms

ZMSP [part for window #1]             "ZMS1"
ZMSP [part for window #2] -- write_segment() -> one canonical body per populated data type
...                                  optional normalized dict.strings
                                     optional normalized dict.blobs
                                     one shared end catalog
                                     one tail index
```

The finished ZMS has no explicit window-boundary markers. Rows from all windows
of one type share a canonical body, and their originating part is not recorded.
The catalog describes physical sections, not window numbers.

## Finished segment v1 layout

All integers are little-endian. Canonical output packs fields without alignment
padding. For `N` catalog entries and `B`
total bytes in all section bodies:

```text
offset  size          contents
0       4             "ZMS1"
4       B             section body 0, section body 1, ... in catalog order
4+B     32*N          catalog entries
...     32            catalog metadata
...     8             tail index
```

The exact file size is:

```text
zms_bytes = B + 32*N + 44
catalog_len = 32*N + 32
```

The 44 fixed bytes are the leading magic, catalog metadata, and tail index.
There is no outer compression layer. These equations describe canonical writer
output. `validate_catalog_layout`, part validation, and physical readers require
body ranges to be contiguous from the leading magic to the catalog. They reject
noncontiguous or overlapping ranges, trailing body bytes, repeated types,
nonzero flags, empty populated sections, and noncanonical section order.

### Catalog entry: 32 bytes

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | `type_id` | `u32` | Section schema registered by `kronika-registry`. |
| 4 | `flags` | `u32` | Reserved; writers store zero. |
| 8 | `offset` | `u64` | Absolute body offset from the first byte of the file. |
| 16 | `len` | `u64` | Body length in bytes. |
| 24 | `rows` | `u32` | Number of logical rows recorded for the body. |
| 28 | `crc32c` | `u32` | CRC32C of this section body. |

Each populated `type_id` appears exactly once. Data sections are ordered by
ascending `type_id`, followed by at most one `dict.strings` and at most one
`dict.blobs`. Different chart entities and rows from different collection
windows are coalesced inside the one body for their type.

### Catalog metadata: 32 bytes

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | `min_ts` | `i64` | Earliest section timestamp, Unix microseconds. |
| 8 | `max_ts` | `i64` | Latest section timestamp, Unix microseconds. |
| 16 | `entry_count` | `u32` | Number of 32-byte entries before this block. |
| 20 | `format_version` | `u32` | Container layout version; writers store `1`. |
| 24 | `crc32c` | `u32` | CRC32C of entries and metadata with this field zeroed. |
| 28 | `window_count` | `u32` | Collection windows coalesced into this container. `build_part` stores `1`; `write_segment` stores the exact number of journal parts; zero means unknown. |

### Tail index: 8 bytes

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | `catalog_len` | `u32` | Entries plus the 32-byte metadata block; excludes the tail itself. |
| 4 | `magic` | 4 bytes | `"ZMS1"`. |

A reader starts at the end:

1. read the final eight bytes;
2. use `catalog_len` to find the catalog start;
3. decode and CRC-check the catalog;
4. read a selected body by its absolute `offset` and `len`;
5. verify the body CRC before handing the bytes to the registry codec.

### Example: a snapshot and two dictionaries in one ZMS

The following simplified ZMS for a Linux host contains one
`os_process` body, one dictionary body for short string values, and one
dictionary body for large values. Let their sizes be `S`, `T`, and `L`, with
`B = S + T + L`.

```text
offset

0       +----------------------------------------------------------+
        | "ZMS1"                                                   | 4 bytes
4       +----------------------------------------------------------+
        | body #0: os_process, type_id=1_100_001             | S bytes
        | Parquet: ts | pid | comm=H | cmdline=Q | ...             |
4+S     +----------------------------------------------------------+
        | body #1: dict.strings, type_id=3_001_001                 | T bytes
        | Parquet: str_id | bytes                                  |
4+S+T   +----------------------------------------------------------+
        | body #2: dict.blobs, type_id=3_002_001                   | L bytes
        | Parquet: str_id | stored_bytes | full_len | ...          |
4+B     +----------------------------------------------------------+
        | catalog entry #0: type=1_100_001, offset=4, len=S        | 32 bytes
36+B    | catalog entry #1: type=3_001_001, offset=4+S, len=T      | 32 bytes
68+B    | catalog entry #2: type=3_002_001, offset=4+S+T, len=L    | 32 bytes
100+B   +----------------------------------------------------------+
        | catalog metadata                                         | 32 bytes
132+B   +----------------------------------------------------------+
        | catalog_len=128 | "ZMS1"                                 | 8 bytes
140+B   +----------------------------------------------------------+
```

For three bodies, `catalog_len = 3 * 32 + 32 = 128`, so the complete file is
`B + 140` bytes. If the window has no large values, the `dict.blobs` body and
its catalog entry are absent. Every non-empty window can contribute rows, but
the finished catalog still contains each populated `type_id` only once.

Kronika encodes each data or dictionary body as a self-contained Parquet file
with its own `PAR1 ... PAR1` framing. The ZMS container treats those bytes as
opaque and does not parse column metadata. The Parquet rows shown above are
decoded contents, not literal plain text in the file: internal encoding and
Zstd mean that text bytes need not occur verbatim in the ZMS.

The byte-exact [`minimal.zms`](tests/fixtures/minimal.zms) fixture is 80 bytes:
`ZMS1`, one four-byte body `01 02 03 04`, one catalog entry, metadata, and the
tail. [`tests/fixture.rs`](tests/fixture.rs) records every offset and verifies
that the encoder reproduces the fixture byte for byte.

## Section body encoding

`kronika-format` permits opaque bodies, but Kronika uses self-contained
Parquet for snapshot and dictionary sections. Journal parts and finished
segments use different writer profiles:

- a collection-window body uses Zstd level 3 and sorts snapshot rows by the
  registry key and dictionary rows by `str_id`;
- a final body uses Parquet 1.0, PLAIN values, RLE levels, and Zstd level 6;
  Parquet may split it into multiple pages or row groups, while dictionary
  encoding, statistics, and offset indexes are disabled;
- canonical data rows use the registry key followed by every remaining column
  as a deterministic total order; normalized dictionary rows use `str_id`;
- the catalog's `u32` row field bounds a section's rows; encoded and aggregate
  decoded section work are each bounded by the 1 GiB version-1 envelope;
- redundant Arrow schema metadata is omitted and `created_by` is empty.

Collection windows are validated independently and appended without projecting
the size of a future coalesced section. Segment rollover follows the configured
journal byte threshold after a successful append, or age. The finalizer checks
catalog field widths, aggregate decoded work, arithmetic, and the actual encoded
body against the version-1 envelope. Its conservative bound accounts for all
data pages and their framing:

```text
body_bound = 64 KiB + sum(zstd_bound(page_inputs_i) + page_count_i * 4 KiB)
body_bound <= 1 GiB
```

For the pinned Zstandard contract:

```text
zstd_bound(n) = n + floor(n / 256)
              + (n < 128 KiB ? floor((128 KiB - n) / 2048) : 0)
```

The 4 KiB term bounds each page header and column-chunk metadata; 64 KiB bounds
Parquet file framing. `write_segment` recomputes the multi-page bound as it
decodes each part and checks the actual encoded body against 1 GiB.

Omitting the embedded Arrow schema removes a duplicate logical schema from
every body; the exact saving depends on the section contract. The native
Parquet schema remains because the decoder needs the physical column layout.

There is no whole-file Zstd pass. Each Parquet body's header and footer, the ZMS
catalog, and the ZMS/ZMSP framing are outside a shared Zstd stream. A reader can
locate and verify one body without decompressing unrelated sections.

## String-value storage

A snapshot section does not store the original text in every record. A
`StrId` column stores only a `u64` equal to `xxh3_64(original_bytes)`. Zero
means "no value". The number is not a file offset: the original bytes must be
found under the same id in separate dictionary Parquet bodies.

`kronika-writer` emits two dictionary section types:

| `type_id` | Section | Columns after decoding |
| ---: | --- | --- |
| `3_001_001` | `dict.strings` | `str_id`, complete `bytes` |
| `3_002_001` | `dict.blobs` | `str_id`, `stored_bytes`, `full_len`, `truncated`, optional `full_sha256` |

Both sections are **inside the ZMS**, alongside snapshot sections.
`dict.blobs` is not a separate file, PostgreSQL TOAST, or external object
storage.

### The short string `postgres`

For the bytes `postgres`,
`H = xxh3_64(b"postgres") = 0x0939566173e67ada`. If two processes in one window
share that name, the data occupies one physical file as follows. The diagram
shows decoded Parquet contents and omits unrelated bodies:

```text
$KRONIKA_STORAGE_DIR/YYYY/MM/DD/N.zms  (one file)

[ "ZMS1" ]
[ os_process Parquet body, type_id=1_100_001
  after decoding:
  pid | comm
  101 | H
  102 | H ]
[ dict.strings Parquet body, type_id=3_001_001
  after decoding:
  str_id | bytes
  H      | b"postgres" ]
[ end catalog
  offset,len -> entire os_process body
  offset,len -> entire dict.strings body ]
[ tail index ]
```

The snapshot section contains two logical `StrId` values represented
as `u64`, while the dictionary contains one record with the `postgres` bytes.

The `os_user` reference section uses this same representation: it stores one
interned user name for an observed UID and process rows keep their numeric real
and effective UIDs. Repeated processes therefore do not repeat the name. The
reference row and its `dict.strings` entry travel through the ordinary journal
and ZMS writer, so restart recovery and dictionary normalization are unchanged.

The catalog points to the dictionary body:

```text
ZMS catalog entry: type_id=3_001_001, offset=X, len=T
                            |
                            v
byte range [X, X+T): self-contained Parquet body
|-- "PAR1"
|-- row group #0
|   |-- str_id column chunk: encodes H
|   `-- bytes column chunk: encodes b"postgres"
|-- Parquet metadata and metadata length
`-- "PAR1"
```

The bytes belong to the `bytes` column chunk of the `dict.strings` body.
In a finished ZMS, Parquet places the PLAIN value in the column's data page and
Zstd level 6 compresses that page. The `os_process` body keeps only `H`.
The ZMS catalog stores the offset and length of the entire dictionary body, not
of one value; finer offsets belong to Parquet metadata. Resolving `H`
requires decoding the dictionary body. The corresponding body inside a
journal part uses the collection-window Zstd level 3 profile.

The "after decoding" blocks show logical values, not on-disk bytes. The exact
cost of the two references need not be 16 bytes, and encoding and compression
mean that `postgres` need not occur as a plain byte sequence in the file.

Reading follows the end catalog:

```text
(1) tail index -> end catalog

(2) catalog -> os_process body
            -> decode Parquet
            -> comm=H

(3) catalog + H -> every dict.strings and dict.blobs body
                -> decode record with str_id=H
                -> b"postgres"
                -> comm="postgres"
```

ZMS has no global `StrId -> offset` index. A full dictionary read visits every
`dict.strings` and `dict.blobs` body and builds an in-memory map. The targeted
index path also reads dictionary bodies but retains only ids requested in
advance.

### Large and truncated values

The collector uses `DictLimits::default()`. Placement works as follows:

| Original value | Section | Bytes retained on disk |
| --- | --- | --- |
| `postgres`, 8 bytes | `dict.strings` | All 8 bytes. |
| Plan text, 20 KiB | `dict.blobs` | All 20 KiB, `full_len=20,480`, `truncated=false`, and no SHA-256. |
| `/proc/PID/cmdline`, 80 KiB | `dict.blobs` | All 80 KiB, `full_len=81,920`, `truncated=false`, and no SHA-256. |
| A 2 MiB value | `dict.blobs` | The first 1 MiB, `full_len=2,097,152`, `truncated=true`, and SHA-256 of the original 2 MiB. |

A truncated value's `StrId` is computed over all original bytes, not the stored
prefix. The discarded suffix cannot be recovered from the ZMS. A source that
caps a value's length does so before interning, so the dictionary treats the
bytes it receives as the complete value.

The default limits are:

| Limit | Value | Effect |
| --- | ---: | --- |
| Blob threshold | 4 KiB | Values at or above it use `dict.blobs`. |
| Truncation limit | 1 MiB | Longer values retain exactly this prefix. |
| Stored-byte cap | 64 MiB | Rejects an ordinary new value when stored dictionary bytes would exceed the cap; required `dict.hot_strings` values are inserted even when the cap is exceeded. |

These values are fixed in code, not environment variables.
The byte cap counts stored value bytes after truncation, not Parquet metadata.
For an ordinary new value, exceeding it returns `DictError::Full`. Required
`dict.hot_strings` values count toward stored bytes, but exceeding the cap does
not prevent their insertion. The encoded section can therefore exceed this cap.

`SegmentDicts` also models a `dict.hot_strings` subset, but `kronika-writer`
does not emit a third dictionary section. Those values are written into the
ordinary `dict.strings` body; there is no `dict.hot_strings` body to find in the
file.

### Dictionary deduplication

One `Interner` spans the open segment. Its current `SegmentDicts` detects hash
collisions and keeps one copy of equal bytes. After a window reaches the
journal, `flush_window` replaces its bytes with compact identity metadata.
Later windows do not write the same value again unless they add a stronger
placement requirement.

`write_segment` normalizes all dictionary records in the journal. Different
bytes or blob metadata for one `str_id`, or a conflict between `dict.strings`
and `dict.blobs` placement, fails the write. The result is sorted by `str_id`
and contains at most one physical record per id in either `dict.strings` or
`dict.blobs`. Physical readers reject repeated dictionary section types, and
dictionary decoding requires strictly increasing ids inside each body.

## `active.wal` and crash recovery

`active.wal` is the root-level durable journal of an unfinished segment.
Journal version 1 starts with a checksummed 36-byte header:

```text
"KRNJNL1\0" | version: u32 | state: u8 | id_present: u8 | reserved: u16
             | segment_id: i64 | body_len: u64 | header_crc32c: u32
```

The header records an empty state or the exact `SegmentId` of the active
segment and the number of frame bytes that follow. Every frame then has a
16-byte header followed by one complete ZMS part:

```text
"ZMSP" | part_len: u64 | header_crc32c: u32 | ZMS part
```

The journal-header checksum covers its first 32 bytes; each frame-header
checksum covers its first 12 bytes. The ZMS part has its own `ZMS1`, bodies,
catalog, and tail, so a frame can be validated before it is accepted. A
canonical empty journal is the 36-byte empty header, never a zero-length file.

For a clean journal, let `P` be its frame count, `N_in` its total part-catalog
entry count, and `B_in` the bytes in all input bodies. Let `K` be the number of
populated types after coalescing and `B_out` the bytes in their canonical
re-encoded bodies:

```text
active_parts_bytes     = 36 + B_in + 32*N_in + 60*P
finished_zms_bytes     = B_out + 32*K + 44
journal_minus_finished = (B_in - B_out) + 32*(N_in - K) + 60*P - 8
```

Every final body is at most 1 GiB, so the container also has the bound:

```text
B_out <= 1 GiB * K
finished_zms_bytes <= (1 GiB + 32) * K + 44
```

`K` has no repeated `type_id`. The size equations include Parquet re-encoding,
removed per-window bodies and catalog entries, normalized dictionaries, and
removed frame overhead.

Version 1 has three absolute admission limits: one ZMS part is at most 64 MiB,
one journal contains at most 1,000,000 frames, and the physical journal file is
at most 1 GiB including the temporary reset marker. Runtime configuration may
only lower these limits. The streaming scanner keeps one bounded ZMS part body
plus the returned frame references in memory. It stops at the first damaged
frame and does not search damaged bytes for another frame magic.

`Journal::open` validates the complete version-1 header, the recorded body
length, and every frame. A headerless file, another version, a torn header, a
length mismatch, or any torn or damaged frame returns an error and leaves the
file unchanged. A zero-length file is initialized to the canonical empty
header. Damage classifications from the low-level scanner do not repair the
journal.

On collector startup, a valid active journal is completed under its stored
`SegmentId` and published at `YYYY/MM/DD/N.zms`. Only successful publication
allows reset. Reset first persists a commit marker. It then writes
`JournalHeader::EMPTY` and calls `sync_data` while the marker and frame body
remain, truncates the file to 36 bytes, and calls `sync_data` again. If the
process exits after the marker is durable, the next `Journal::open` validates
it and completes the reset.

The reset marker is 32 bytes and records the previous journal length,
`SegmentId`, and header checksum. The configured journal cap is a literal
physical-file limit: the writer reserves those 32 bytes before every append,
including the first frame.

## File-size controls

The ZMS layout has no compression settings. Collector source intervals control
snapshot frequency. Segment size and age control how much data each file
contains and how many windows `write_segment` coalesces. The journal-size limit
triggers an early write and is not a target ZMS size. The collector does not cap
rows by source.

See the [collector configuration](../../bins/kronika-collector/README.md) for
the variables and defaults.

### Approaches not implemented

| Approach | Source of the saving | Constraint |
| --- | --- | --- |
| Higher final Parquet Zstd level | Pages inside one body may become smaller. | Costs more CPU; final bodies already use level 6, while collection-window bodies use level 3. |
| Outer compression for the whole ZMS | One stream can see repeated dictionaries, footers, and similar windows. | Direct body access by `offset` is lost; that would replace ZMS access semantics. |

## Integrity and limits

CRC32C covers every section body, the catalog, and each `ZMSP` header. It
detects accidental corruption; it is not authentication, a signature, or
encryption.

`kronika-format` validates framing, catalog length and checksum, section
bounds, and section checksums for complete parts. The finished-segment reader
accepts container version 1 and caps catalog, section, row, and row-group sizes.
An incompatible section schema receives a new `type_id`; changing the ZMS
framing requires a new container version.

References:

- [`src/lib.rs`](src/lib.rs), [`src/catalog.rs`](src/catalog.rs), and
  [`src/parts.rs`](src/parts.rs) for the byte contract;
- [`src/dictionary.rs`](src/dictionary.rs) for dictionary invariants;
- [`kronika-registry`](../kronika-registry/src/lib.rs) for section schemas and
  Parquet limits;
- [`kronika-writer`](../kronika-writer/README.md) for append and write behavior.
