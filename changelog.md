# Changelog

[Русская версия](changelog.ru.md)

## [1.2.3](https://github.com/pgtoolz/Kronika/releases/tag/v1.2.3)

- Fixed native PgBouncer journal messages being dropped. Exported wrapper timestamps and PIDs are retained. Messages without a severity use LOG.

## [1.2.2](https://github.com/pgtoolz/Kronika/releases/tag/v1.2.2)

- Fixed PostgreSQL log collection stopping on messages without a prefix or a usable timestamp. Recognized messages without a usable timestamp are saved with the read time.
- PgBouncer logs retain unlisted warnings, errors and lifecycle messages, with PID, connection side, port and age. Routine connection chatter is omitted. Text journalctl/syslog wrappers are supported.

## [1.2.1](https://github.com/pgtoolz/Kronika/releases/tag/v1.2.1)

- Fixed refresh when returning to a tab after the selected hour ends.
- Fixed manual and automatic refresh of Activity heatmaps.
- Cgroup collection runs only in containers with cgroup v2.
- When the kernel reports PSI as unsupported, the collector warns once and stops PSI reads until restart.
- The web server shows all sources by default.

## [1.2.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.2.0)

### Collection and configuration

- Collector, web, dump, report and the development demo now use clap for command-line parsing, short `-h` help and detailed `--help`. Collector, web and demo options override their environment settings. Collector storage limits accept sizes such as `64MiB` and `10GB`; plain byte counts still work. Decimal suffixes use powers of 1000, IEC suffixes powers of 1024.
- PostgreSQL source groups have independent intervals. Defaults are 30 seconds for server counters/settings, 10 seconds for activity/lock waits/VACUUM progress, 5 seconds during detected lock waits, and 300 seconds for statements/plans and tables/indexes. See [collector configuration](bins/kronika-collector/README.md#collection-intervals).
- Collector and web use two Tokio worker threads. The musl collector disables jemalloc transparent huge pages by default.

**Upgrade:** `KRONIKA_PG_INTERVAL_S` now controls only server counters/settings (`--pg-instance-interval-s`). To configure activity or statements/plans, use `KRONIKA_PG_ACTIVITY_INTERVAL_S` / `--pg-activity-interval-s` and `KRONIKA_PG_STATEMENTS_INTERVAL_S` / `--pg-statements-interval-s`. The statements/plans interval has a 300-second minimum and starts when their PostgreSQL collection pass finishes; `SIGUSR2` cannot bypass it.

### Browser and reports

- Cursor arrows follow recorded samples for the current screen, across segment boundaries, with at least one second between steps. Screen snapshots select the latest observation at or before the cursor, including overlapping segments. This works in web and offline reports; the API adds [`/api/snapshot/neighbor`](bins/kronika-web/README.md#endpoints).
- Statements and Plans heatmaps use 12 columns per hour, matching the five-minute columns in Tables and Indexes. Nearest samples outside the range can complete edge columns. Edge lookups and counter gaps are limited to 15 minutes; longer gaps contribute no rate. Valid zero differences remain zero, and ranking totals still use only in-range endpoints. See [heatmap timing](docs/metrics-time.md#heatmaps).
- Browser API caches distinguish serving builds. Help shows the served version and, when available, its build commit.
- Refresh recovers stalled requests without cancelling streams that keep receiving data; cancelled responses cannot replace the current view. Snapshot loading can recover, and the timeline distinguishes loading, errors and empty data. Fixed chart initialization before its container has a measurable size.

### Libraries and builds

- Moved reusable source acquisition into `kronika-source-os` and `kronika-source-pg`, report queries/generation into `kronika-report`, and recording extraction into `kronika-slice`. CLI and server responsibilities remain in the applications. See [library boundaries](crates/README.md).
- Report asset generation uses pinned Rust, wasm-bindgen and Clang versions on a canonical Linux host, with reproducibility checks for embedded HTML and WebAssembly.

**Source builds:** use `cargo build -p kronika-report-cli` for the report executable. The binary remains `kronika-report`; the Cargo package `kronika-report` now contains the shared library.

## [1.1.2](https://github.com/pgtoolz/Kronika/releases/tag/v1.1.2)

- Fixed PostgreSQL event timestamps when the server and collector use different time zones.
- The collector skips PostgreSQL log events older than 15 minutes. Set `KRONIKA_PG_LOG_MAX_LAG_S` to change the limit.

## [1.1.1](https://github.com/pgtoolz/Kronika/releases/tag/v1.1.1)

- Web authentication is now optional.
- Removed `KRONIKA_WEB_AUTH`.

## [1.1.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.1.0)

- Added `KRONIKA_PG_DSN` for the PostgreSQL connection string. To collect from several servers, run `kronika-collector` for each server with its own `KRONIKA_PG_DSN` and a separate `KRONIKA_STORAGE_DIR`.
- Collectors close ZMS files on a schedule with a random time offset. The offset survives restarts; closes can still coincide.

Move the first connection string from `KRONIKA_PG_DSNS` to `KRONIKA_PG_DSN` and remove the old variable. Support for `KRONIKA_PG_DSNS` will be removed.

### Other changes

- Added `KRONIKA_COLLECTOR_MODE=postgresql` to collect PostgreSQL data from a local or remote server without Linux metrics. The `local` mode collects Linux metrics from the same VM or pod and, optionally, PostgreSQL. See [collector configuration](bins/kronika-collector/README.md).
- The collector now records all visible, accessible cgroup v2 groups. CPU, Memory, I/O and Tasks tables show them with search, sorting and history. Groups stay separate; limits distinguish unlimited from unavailable. Older recordings remain readable.
- Fixed reading of PostgreSQL 12 and older `pg_stat_statements` layouts without requesting absent fields. Activity metrics unsupported by the recording are hidden.
- Fixed Activity heatmaps in partial-hour HTML reports to stay within their recorded time window. Cell clicks and row navigation select the corresponding recorded interval instead of jumping to the report start.

## [1.0.1](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.1) — 2026-09-06

- Clarify installation and startup in English and Russian, including local PostgreSQL setup and which machine supplies Linux measurements with a remote PostgreSQL connection.

## [1.0.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.0) — 2026-09-06

- Initial release: Linux and PostgreSQL recording, web inspection and self-contained offline HTML reports. Static Linux archives for x86-64 and ARM64 include the collector, web server, dump and report tools.
