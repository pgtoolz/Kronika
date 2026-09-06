# kronika-layout

[Русская версия](README.ru.md)

`kronika-layout` defines Kronika's local data-directory grammar. It maps a
stable segment identity to a UTC calendar directory, discovers entries within
fixed bounds, and opens files relative to verified directory descriptors.
Process-wide locks allow one collector to change `active.wal` and ZMS files and
one web process to change IDX files. `kronika-format` defines ZMS framing and
journal bytes; `kronika-writer` encodes sections and writes final segments.

## On-disk layout

```text
DATA_ROOT/
├── active.wal
├── .kronika-writer.owner.lock
├── .kronika-index.owner.lock
└── YYYY/
    └── MM/
        └── DD/
            ├── N.zms
            └── N.idx
```

`N` is the decimal [`SegmentId`](src/time.rs): the Unix timestamp in
microseconds of the first collection window successfully appended to that
segment. `YYYY/MM/DD` is the UTC day derived from that value. The path does not
use the segment catalog's `min_ts` or `max_ts`, finalization time, file
modification time, or the timestamp of a late event.

A segment that remains open across UTC midnight stays under the day on which
its `SegmentId` falls. Time-range queries still use the catalog's `min_ts` and
`max_ts`; the directory is a physical bucket, not a query index.

`N.zms` is the immutable source segment. `N.idx` contains replaceable derived
data and uses the same stem. An IDX can be rebuilt from its ZMS.

`active.wal` stays at the root. Journal format version 1 uses magic
`KRNJNL1\0`. Its header stores the active `SegmentId`.
`kronika-layout` controls access to the file, while `kronika-format` defines
its bytes and `kronika-writer` implements its lifecycle.

## Closed grammar

The data root is Kronika-owned. Its grammar contains the journal, two owner-lock
files, four-digit year directories, two-digit valid months and days, canonical
final files, and recognized Kronika publication temporaries. The scanner
follows only verified calendar directories. It records symbolic links, unknown
entries, misplaced segment ids, and root-level `.zms` or `.idx` files in
`LayoutSnapshot::foreign_entries` without traversing them. Valid entries remain
in the returned inventory. Exhausted limits and errors while reading the
verified tree fail the scan.

## Types and access

- `SegmentId` validates a Unix-microsecond identity representable by UTC years
  `0000..=9999`.
- `UtcDay` validates and formats one `YYYY/MM/DD` bucket.
- `SegmentAddress` binds one id to the only valid UTC day and returns the
  canonical `N.zms` and `N.idx` names.
- `DataRoot` holds an open root descriptor, classifies entries against the
  closed grammar, and opens final files without following symbolic links.
- `FileIdentity` identifies a ZMS by device, inode, length, `mtime`, and
  `ctime`; store and reader code check it again on the opened file descriptor.
- `WriterOwner` is the sole capability for `active.wal` and ZMS publication.
  One collector can hold it for a data root.
- `IndexOwner` is the sole capability for IDX publication and cleanup. One
  web process can hold it for a data root.

The lock files remain in `DATA_ROOT`; they are part of the layout even when no
process currently holds a lock.

## Traversal limits

`LayoutLimits` bounds every strict scan before it returns a snapshot:

| Field | Default | Hard maximum | Meaning |
| --- | ---: | ---: | --- |
| `max_visited_entries` | 1,000,000 | 4,000,000 | All filesystem entries visited in one scan. |
| `max_entries_per_day` | 10,000 | 1,000,000 | Entries inspected in one UTC day directory. |
| `max_segments` | 500,000 | 2,000,000 | Finished ZMS segments returned. |
| `max_metadata_bytes` | 134,217,728 | 134,217,728 | Shared cap for names, journal metadata, returned collections, and compact catalog summaries. |

Every value must be non-zero and no greater than its hard maximum. Exceeding a
runtime bound fails the scan instead of returning an incomplete inventory.
The default 128 MiB cap covers five 365-day years with one segment every
15 minutes for both cold discovery and an unchanged cached refresh. A wholesale
same-name replacement is rejected before both complete sets of summaries can
be retained.

## Publication and backup

ZMS publication creates and synchronizes a temporary file inside the target
day, adds the final `N.zms` name without overwriting an existing segment,
synchronizes the day, removes the temporary name, and synchronizes the day
again. IDX publication synchronizes a same-day temporary and atomically
replaces `N.idx` only after checking the input ZMS descriptor again.

Local publication supports Linux with ext4 or XFS. Descriptor-bound ZMS
publication resolves the already-open temporary through `/proc/self/fd`, so
containers and sandboxes must mount procfs. Equivalent durability and lock
behavior are not claimed for other operating systems or network filesystems.

A backup must preserve the complete directory hierarchy. Use a stopped
collector and web process or a filesystem snapshot with equivalent
consistency. IDX files may be rebuilt, but `active.wal` and ZMS files are
source data. PostgreSQL log-tail state is outside `DATA_ROOT`; if
it is needed for recovery, capture it separately at the same consistency
point.

The public API and error variants are in [`src/lib.rs`](src/lib.rs).
