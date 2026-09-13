# Changelog

[Русская версия](changelog.ru.md)

## 1.1.0 — Unreleased

Changes since the last published release, 1.0.1.

- Add `KRONIKA_PG_DSN` for the PostgreSQL connection string. To collect from several servers, run `kronika-collector` for each server with its own `KRONIKA_PG_DSN` and a separate `KRONIKA_STORAGE_DIR`.
- Collectors close ZMS files on a schedule with a random time offset. The offset survives restarts; closes can still coincide.

`KRONIKA_PG_DSNS` is deprecated and will be removed; only its first DSN is used. Replace it with `KRONIKA_PG_DSN` and remove the old variable. Setting both stops startup.

### Other changes

- Add `KRONIKA_COLLECTOR_MODE=postgresql` to collect PostgreSQL data from a local or remote server without Linux metrics. The `local` mode collects Linux metrics from the same VM or pod and, optionally, PostgreSQL. See [collector configuration](bins/kronika-collector/README.md).
- Collect all visible, accessible cgroup v2 groups and show them in CPU, Memory, I/O and Tasks tables with search, sorting and history. Groups stay separate; limits distinguish unlimited from unavailable. Older recordings remain readable.
- Read PostgreSQL 12 and older `pg_stat_statements` layouts without requesting absent fields. Hide Activity metrics unsupported by the recording.
- Keep Activity heatmaps within their recorded time window in partial-hour HTML reports. Cell clicks and row navigation select the corresponding recorded interval instead of jumping to the report start.

## [1.0.1](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.1) — 2026-09-06

- Clarify installation and startup in English and Russian, including local PostgreSQL setup and which machine supplies Linux measurements with a remote PostgreSQL connection.

## [1.0.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.0) — 2026-09-06

- Initial release: Linux and PostgreSQL recording, web inspection and self-contained offline HTML reports. Static Linux archives for x86-64 and ARM64 include the collector, web server, dump and report tools.
