# GitHub Pages demo recording

[Русская версия](README.ru.md)

`github-pages-hour.zms` is the fixed production input for the Kronika Pages
report. It contains one full hour of synthetic Linux, PostgreSQL, and PgBouncer
data under the public hostname `kronika-demo`: short concurrent commerce
transactions, lock waits, query plans, and bounded CPU, memory, disk, and
network activity. The production slicer created the standalone ZMS:

```sh
KRONIKA_STORAGE_DIR=CAPTURE kronika-dump slice \
  --from 2026-09-05T19:00:00Z \
  --to 2026-09-05T19:59:59Z \
  --out github-pages-hour.zms
```

The inclusive whole-second endpoints represent the half-open interval
`[19:00:00, 20:00:00)`, exactly 3,600,000,000 microseconds. Production slicing
may retain nearby snapshots outside the requested interval. The report uses
the explicit bounds in `github-pages-hour.slice`; its timeline shows the
19:00–20:00 calendar hour.

The live recorder uses runtime timestamps and counters, so CI keeps this ZMS as
its fixed input. `scripts/build-pages-report.sh` checks its SHA-256, passes this
exact file and range to `kronika-report`, checks the embedded range, compares
two generated HTML files byte for byte, and exercises the result directly from
disk in Chromium.

Captured on 5 September 2026 with the demo workload running throughout the
requested hour. The 2,360,449-byte ZMS has 52 physical sections; its retained
snapshots span 18:59:58.821275–20:00:03.308629 UTC. These surrounding snapshots
support interval calculations; the visible report interval remains exactly 19:00–20:00 UTC.
