# kronika-dump

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

`kronika-dump` shows the times, data sections and rows in a recording. Its
`slice` command extracts an interval into one `.zms` file that `kronika-report`
can turn into an HTML report. Sources: [inspection parser](src/main.rs), [slice command](src/slice.rs),
[parameter help](src/help.rs).

## Inspect

```sh
kronika-dump /var/lib/kronika
kronika-dump /var/lib/kronika --json
kronika-dump /var/lib/kronika --index
kronika-dump /var/lib/kronika --section 1100001 --limit 10
```

| Parameter | Default | Meaning |
| --- | --- | --- |
| `DIR` | Required | Real collector storage root, containing `YYYY/MM/DD/*.zms` and optionally `active.wal`. Symlinks and standalone ZMS paths are rejected. |
| Display without flags | Segment summaries | Segment bounds, physical section IDs, row counts, section bytes and physical overhead bytes. |
| `--section ID` | Unset | Decode one physical numeric `type_id`; resolve dictionary references. |
| `--index` | Unset | Derived series/index summaries; with `--json`, individual points and finding locators. Mutually exclusive with `--section`; creates no sidecar. |
| `--json` | Text output | NDJSON; scan warnings are JSON objects on stdout. |
| `--limit N` | `20` | Nonnegative row limit per segment; `0` means all rows. Requires `--section`. |
| `--from`, `--to` | Unbounded | Inclusive signed Unix microseconds; select intersecting segments. Section rows within selected segments are not trimmed. Either bound can be supplied. |

Inspection reads finished segments and the committed active journal. It requires
read access to storage; no configuration environment is read. Data goes to
stdout; text warnings/errors go to stderr. A closed output pipe exits
successfully; other failures exit nonzero.

## Slice

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika kronika-dump slice \
  --from 2026-09-05T19:00:00Z \
  --to 2026-09-05T19:59:59Z \
  --out incident.zms
```

| Parameter | Meaning |
| --- | --- |
| `KRONIKA_STORAGE_DIR` | Required real collector storage root with read access; finished segments and committed journal are inputs. |
| `--from RFC3339` | Required inclusive first whole second, with `Z` or a timezone offset. |
| `--to RFC3339` | Required inclusive last whole second, at or after `--from`. Equal bounds select one complete second. Fractional seconds and numeric Unix values are rejected. |
| `--out FILE.zms` | Required new `.zms` path. Parent directory must exist and be writable. Existing paths are rejected. |

Each option is supplied once, in any order. The logical interval is
`[from, to + 1 second)`. The output can retain samples within 30 seconds on each
side for interval calculations. An interval with no recorded rows fails.

Temporary files and scratch data are created beside the output on the same
filesystem. The completed ZMS is validated before publication. Stdout reports
bytes, rows, sections and requested/actual Unix microsecond bounds;
`requested_to_exclusive` is one second after `--to`. Errors go to stderr and
exit nonzero. [kronika-report](../kronika-report/README.md) converts the result
to HTML.

## Common options

`-h` and `--help` select general help; `slice -h` and `slice --help` select
slice help. `--version` prints the binary version. These calls exit before
storage access. `Ctrl+C` interrupts a running command.
