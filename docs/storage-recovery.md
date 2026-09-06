# Storage failures and recovery

[Русская версия](storage-recovery.ru.md) · [Services](services.md)

Kronika stores collected data in three kinds of files:

| File | Contents | Who writes it |
| --- | --- | --- |
| `active.wal` | The current append journal: completed batches of collected data waiting to become a finished recording. This is separate from PostgreSQL's own WAL. | Collector |
| `YYYY/MM/DD/<segment-id>.zms` | A finished compressed recording, called a segment. | Collector; `kronika-dump slice` can create a separate ZMS file. |
| `.idx` beside a ZMS | Derived summaries and row locations used to answer queries faster. | Web |

## After an abrupt collector stop

An OOM kill terminates the process because the machine or container has run out
of memory. Like `SIGKILL`, it can interrupt a write without running shutdown
code. The journal may remain valid or contain an incomplete append, depending
on when the process stopped. Data still in collection buffers has not reached
the journal and cannot be recovered from it.

For each append, collector writes the batch and its header in separate
operations, then calls `sync_data`. Only after that succeeds is the append counted
as complete. One collection cycle can append several batches: earlier batches
can be saved even if the cycle did not finish. A returned write error triggers
rollback while the process is alive; a killed process cannot perform it.
Synchronization requests persistence from the filesystem. A process kill does
not itself clear the operating system's file cache; power loss can. Durability
also depends on the filesystem and storage device completing synchronization.

A finished ZMS is first written to a temporary file and synchronized. Its final
name is then added without replacing an existing file, and the containing
directory is synchronized. The temporary name is removed and the directory is
synchronized again. Only successful publication permits clearing the journal.

## What collector does on startup

Collector checks the journal's format signature and version, header checksum,
state and segment identity, recorded length against actual file length, and
every stored batch. Batch checks include frame boundaries, size limits, the
section catalog and CRC32C checksums. CRC32C detects changed bytes; it cannot
reconstruct them.

| Journal state | Startup result |
| --- | --- |
| Missing or zero-byte file | Initialize an empty journal. An initialized empty journal has a 36-byte header. |
| Completely valid populated journal | Create a ZMS under the segment identity saved in the header, then clear the journal. New collection starts a new segment. |
| Incomplete header or final batch, length mismatch, damaged frame or checksum | Refuse startup and leave the existing `active.wal` unchanged. No prefix is published and no trailing batch is discarded. |
| Correct checksums but unreadable Parquet data, unknown recorded type or incompatible schema | Segment creation fails; the journal remains in place. Checksums do not replace data decoding. |
| Journal exceeds the configured byte limit or format batch-count limit | Refuse to open it; the limit does not truncate the file. |
| Verified marker of an interrupted journal reset | Complete that reset, after checking the marker, allowed header transition and old batches. |

There is no automatic “keep everything before the last good record” repair.
The low-level scanner can identify a valid prefix, but collector requires the
whole populated journal to pass. Successfully saved batches retain their rows
and combined time bounds; the finished catalog's `window_count` counts journal
batches, which need not equal collection cycles. Collection does not resume
inside that recovered segment.

If a ZMS already exists at the recovered name, it must validate and match the
newly generated file byte for byte. A different existing file is not overwritten.
The journal reset has its own synchronized marker: after publication, collector
records the reset, writes an empty header, then truncates to 36 bytes. A verified
interrupted reset can be completed on restart; this does not repair arbitrary
header or body damage.

A successful recovered write logs `segment_write_finish` with `reason=recovered`
and `segment_path`. This event precedes journal reset. After reset succeeds,
stdout contains `wrote <path> reason=recovered`; `ready` follows successful
initialization. An open failure includes
`open active.wal; the existing file is preserved on failure`. A failed recovered write or reset logs
`segment_close_failure`, `reason=recovered`, and stage `write` or `journal-reset`.

## Reading the journal while collector writes

A read captures a journal identity and completed byte boundary. It reads only
batches before that boundary; later appends do not extend that read. An already
captured snapshot can remain readable while a later batch is being appended.
If the journal is reset, replaced or starts another segment, the old read can
fail because its source changed. Retrying against the new generation is
separate from repairing damaged bytes.

A fresh directory scan checks the captured journal length and every batch in
it. An incomplete or damaged append can exclude the active journal with a
warning, while finished recordings remain listed. This can also happen during
an append; a warning alone does not prove permanent damage. A verified reset
marker represents an empty active journal after the old batches validate.
**Web never truncates, resets or repairs `active.wal`.**

## When finished recordings are checked

A section is a set of rows of one recorded type, such as process measurements.
ZMS stores each section compressed, plus a catalog describing its byte range,
row count and checksum. Validation happens in stages:

| Operation | What is checked |
| --- | --- |
| Web catalog discovery | Opening/trailing `ZMS1` signatures, tail and catalog lengths, format version, catalog checksum, section ordering and bounded nonoverlapping byte ranges. Section bodies are not read or decompressed. |
| Validated range listing, including ordinary `kronika-dump` | The same structure, then every raw section checksum in selected finished segments. Files outside the selected time range do not receive this body check. |
| Reading rows of a requested section | That section's raw CRC32C, Parquet metadata and read limits, recorded row count and expected schema. Decompression and row decoding can fail as data is consumed. Required dictionaries are read separately. Other sections are not all decoded. |

For example, a truncated catalog can exclude the whole file immediately. A
changed byte in a section can leave the file visible in the catalog, then fail
when that section is read. Even with a matching checksum, an invalid Parquet
page or a field with the wrong type can fail decoding. Successfully opening a
catalog or reading one chart does not validate every row in the recording.
The [format reference](../crates/kronika-format/README.md) defines the exact
layout, limits and checksum coverage.

## Missing or broken indexes

For a finished local ZMS, web rebuilds an absent index, an opened index that
fails format/checksum validation, or an index missing required query blocks.
This includes obsolete headers: the current format is `KRNIDX1`. Rebuilding
reads the ZMS, writes and synchronizes a temporary index, then replaces the
`.idx` atomically after rechecking the source file. An index file that cannot
be opened because of access or filesystem errors can fail the request before
rebuilding begins. A rebuild can also fail if the ZMS cannot be read or the
index cannot be written.

A valid IDX can still answer summary queries when a ZMS body is damaged, because
that path does not reread the body. It cannot restore the raw rows or make them
readable. An HTML report uses its embedded ZMS and IDX in memory: it validates
the supplied index and does not rebuild a missing or broken index in the browser.

## What a web request returns

| Request or stage | Result on a storage read failure |
| --- | --- |
| Catalog scan | Invalid/unreadable files can be omitted with warning records while other recordings remain available. Root-level access or traversal failures can fail the request. The browser filters catalog warning records; an omitted file does not necessarily produce a visible error panel. |
| Requested data before response headers | A read failure returns HTTP `500` with `{"error":"unreadable"}`, including a failure while the server is still holding the first output bytes before sending them. |
| Requested segment or section is absent | HTTP `404` with `{"error":"no_such_segment"}` or `{"error":"no_such_section"}`. |
| Data stream after headers and initial bytes were sent | The response body aborts. Its already-sent HTTP status cannot change; no final NDJSON error record is appended. Received rows are not a complete successful result. |
| HTML export | Invalid-ZMS or active-journal scan warnings stop export with HTTP `500`, `export_failed`, including warnings about files outside the requested interval. |
| Instance label | A read error is logged and the endpoint returns HTTP `200` with `database: null`. |

If a captured file changed during reading and the error permits a fresh read,
web retries once before sending response headers. A second failure ends the
request; checksum and decoding failures are not retried this way. After
transmission starts, web aborts the response instead of mixing file generations.
The browser separately retries a failed network/body transfer once; it does not
retry an HTTP error response.

A failed request does not stop the web server. Requests that need different
readable data can still succeed. An initial hour-load failure shows an error.
A table-load failure has its own error state, distinct from a successful empty
result; matching previously loaded rows can remain visible. A failed refresh
preserves the working view. Some supplementary requests, such as details about
groups of processes with shared resource limits (cgroups), only log an error in
the browser console and leave the primary rows intact. Catalog warnings do not
themselves open an error panel.

## Inspecting a failure

For the units in the [service guide](services.md), read both service logs:

```sh
sudo journalctl -u kronika-collector -u kronika-web --since '10 minutes ago'
sudo kronika-dump /var/lib/kronika
sudo kronika-dump /var/lib/kronika --json
sudo kronika-dump /var/lib/kronika --section 1100001 --limit 10
```

Use your configured data directory in place of `/var/lib/kronika`. Collector
errors identify the operation; segment events include `segment_path`. Web logs
failed API reads with the underlying error message, but scan warnings contain
only a warning code. The catalog warning identifies a segment or the active
journal; not every message includes a full path. A finished segment's ID maps
to `YYYY/MM/DD/<segment-id>.zms` in the data directory.

`kronika-dump` checks structure and section checksums before printing admitted
segments. Warnings appear on stderr, or as records on stdout with `--json`.
**Warnings can accompany exit status 0:** inspect them as well as the output.
`--section` additionally decodes the chosen type and needed dictionaries; the
example prints at most 10 rows per segment. A returned decoding error exits
nonzero. Neither inspection command changes the input files. The diagnostic
phrase `set aside` means excluded from that scan, not moved on disk.

After an OOM kill, normal startup handles a valid journal automatically. If it
refuses damaged bytes, keep the affected file for inspection: restarting again
does not turn it into a repaired journal. There is no repair command that
reconstructs lost WAL/ZMS bytes. Index rebuilding applies only to derived IDX
files. The [dump reference](../bins/kronika-dump/README.md) describes inspection
and interval extraction; extraction is not a raw-file repair operation.
