# GitHub Pages demo recording

[Русская версия](README.ru.md)

`github-pages-hour-20260921T085946Z.zms` is the current Pages input, recorded
on 21 September 2026 during a full hour of demo workloads. It contains real
Linux, PostgreSQL and PgBouncer observations: seven cgroups, processes,
concurrent transactions, lock waits, query plans and 429 PgBouncer events
with full messages and connection context.

`kronika-dump slice` created the standalone file from the collector directory
`CAPTURE`:

```sh
KRONIKA_STORAGE_DIR=CAPTURE kronika-dump slice \
  --from 2026-09-21T08:59:46Z \
  --to 2026-09-21T09:59:45Z \
  --out github-pages-hour-20260921T085946Z.zms
```

The inclusive whole-second endpoints select `[08:59:46, 09:59:46)` UTC,
exactly 3,600 seconds. The 813,685-byte ZMS contains 57 physical sections.
Nearby snapshots support interval calculations. The report uses the explicit
visible bounds in `github-pages-hour-20260921T085946Z.slice`.

`scripts/build-pages-report.sh` verifies the checksum, renders the fixed input
twice, compares the HTML bytes and exercises the report offline in Chromium.

The original `github-pages-hour.zms`, checksum and slice retain the
5 September 2026 recording used by the historical operator-guide examples.
