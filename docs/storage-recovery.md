# Storage failures and recovery

[Русская версия](storage-recovery.ru.md) · [Services](services.md)

| File | Purpose | Writer |
| --- | --- | --- |
| `active.wal` | Current collected data, not yet saved as a finished segment. Separate from PostgreSQL WAL. | Collector |
| `YYYY/MM/DD/<segment-id>.zms` | Finished compressed recording. | Collector; `kronika-dump slice` can create a separate ZMS. |
| `.idx` beside a ZMS | Search index derived from the recording. | Web |

## Inspecting a failure

Start with the service logs and a read-only inspection of the recordings:

```sh
sudo journalctl -u kronika-collector -u kronika-web --since '10 minutes ago'
sudo kronika-dump /var/lib/kronika
sudo kronika-dump /var/lib/kronika --json
sudo kronika-dump /var/lib/kronika --section 1100001 --limit 10
```

Use your configured storage directory. These commands assume the units in the
[service guide](services.md). Collector errors name the failed operation;
segment events include `segment_path`. Web API errors include the underlying
read error. Catalog warnings identify the active journal or a segment, sometimes
without its full path. Segment IDs map to `YYYY/MM/DD/<segment-id>.zms`.

`kronika-dump` checks file structure and section checksums. `--section` also
decodes that type and its dictionaries; the example prints up to 10 rows per
segment. Decoding errors return a nonzero exit status. **Warnings can accompany
exit status 0:** read stderr, or the warning records on stdout with `--json`.
The phrase `set aside` means omitted from the scan, not moved on disk.

If startup refuses a damaged file, preserve it before further investigation.
Repeated restarts do not repair it. There is no command to reconstruct lost
WAL or ZMS bytes. [Interval extraction](../bins/kronika-dump/README.md) can save
readable data separately, but does not repair its source.

## After an abrupt collector stop

Start the collector normally. It immediately converts a valid nonempty
`active.wal` into a ZMS and resumes collection. `SIGINT` and `SIGTERM` leave
the journal for this recovery rather than closing a final ZMS at shutdown.

OOM kills and `SIGKILL` can interrupt a write. The journal may remain valid or
contain an incomplete batch. Data that had not reached the journal cannot be
recovered from it. Some batches may survive even if their collection cycle did
not finish. Power-loss durability also depends on the filesystem and device.

## What collector does on startup

| Journal state | Result |
| --- | --- |
| Missing or zero-byte file | Initialize an empty journal. |
| Valid nonempty journal | Save a ZMS with the recorded segment ID, clear the journal and begin a new segment. |
| Incomplete data, length mismatch or checksum failure | Refuse startup and preserve `active.wal`; do not discard the damaged tail or publish only the valid prefix. |
| Unreadable Parquet data, unknown type or incompatible schema | Fail segment creation and preserve the journal. |
| Configured size limit or format batch-count limit exceeded | Refuse to open the journal, without truncating it. |
| Verified interrupted journal reset | Finish the reset. This does not repair unrelated damage. |

An existing ZMS at the recovered path must be valid and match the recovered
file byte for byte; a different file is never overwritten.

Look for `wrote <path> reason=recovered` and `ready` to confirm recovery and
startup. `segment_write_finish` alone is not enough. On failure, inspect
`segment_close_failure` or the `open active.wal` error; the latter confirms
that the existing file was preserved.

## Reading the journal while collector writes

A request reads the completed batches available when it starts, not later
appends. A journal reset or replacement can interrupt that read. An active
journal may briefly disappear from a fresh catalog scan during an append;
finished recordings remain available. Retry after collection settles before
concluding that the file is damaged. **Web never truncates, resets or repairs
`active.wal`.**

## When finished recordings are checked

A file appearing in the catalog does not prove that all its data is readable:

| Operation | Checks |
| --- | --- |
| Web catalog discovery | File structure and catalog checksum, without reading section bodies. |
| Validated range listing, including ordinary `kronika-dump` | Structure and every section checksum in selected finished segments; bodies outside the selected interval are not checked. |
| Reading a section | Its checksum and whether its rows can be decoded. This does not validate every other section. |

A damaged catalog can hide a whole file. Damaged section data may fail only
when a particular table or chart reads it. Checksums detect changed bytes but
cannot reconstruct them. The [format reference](../crates/kronika-format/README.md)
describes byte layout and validation limits.

## Missing or broken indexes

Web automatically rebuilds missing, obsolete or invalid local `.idx` files,
including indexes without blocks required by a query. It needs a readable ZMS
and permission to write in the storage directory. An index that cannot be
opened because of permissions or filesystem errors can fail the request before
rebuilding starts; check those errors first.

An index can still answer some summary queries from a damaged ZMS, but cannot
restore its rows. Browser HTML reports validate their embedded index and do not
rebuild it. Recreate a damaged report from readable source recordings.

## What a web request returns

| Failure | Result |
| --- | --- |
| Invalid or unreadable file during catalog scan | The file can be omitted with a warning while other recordings remain available. Storage-root access or traversal errors can fail the whole request. |
| Data cannot be read before the response starts | HTTP `500`, `{"error":"unreadable"}`. |
| Segment or section does not exist | HTTP `404`, `{"error":"no_such_segment"}` or `{"error":"no_such_section"}`. |
| Read fails after the response starts | The transfer aborts. Received rows are incomplete even if the HTTP status was successful. |
| Invalid ZMS or active-journal warning during HTML export | HTTP `500`, `export_failed`, even when the warning concerns a file outside the requested interval. |

Failed requests do not stop web. The interface reports hour/table-load errors
and may retain previously loaded rows; a failed refresh preserves the working
view. Some supplementary failures appear only in the browser console. Catalog
warnings do not open an error panel, so inspect logs when recordings are missing.

## Back up before further investigation

Stop collector and web before copying storage, or use a filesystem snapshot
with equivalent consistency. Preserve the complete directory hierarchy.
`active.wal` and ZMS files are source data; IDX files can be rebuilt.
PostgreSQL log-tail state lives outside the storage directory: if needed for
recovery, copy it separately at the same consistency point.

Keep one collector per storage directory. Do not bypass ownership protection
to start a second writer. The [storage layout reference](../crates/kronika-layout/README.md)
describes ownership, file publication and supported filesystem guarantees.
