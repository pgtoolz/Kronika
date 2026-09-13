# Changelog

[Русская версия](changelog.ru.md)

## 1.1.0 — Unreleased

Changes since the last published release, 1.0.1.

- Use `KRONIKA_PG_DSN` for one PostgreSQL server. Start a separate `kronika-collector` process for each server, with its own DSN and `KRONIKA_STORAGE_DIR`. All processes use the same binary; each collects metrics from the accessible databases on its server.
- Collectors close ZMS files on a schedule with a random time offset. The offset survives restarts; closes can still coincide.

`KRONIKA_PG_DSNS` is deprecated and will be removed; only its first DSN is used. Replace it with `KRONIKA_PG_DSN` and remove the old variable. Setting both stops startup.

### Other changes

- Add `KRONIKA_COLLECTOR_MODE=postgresql` for local or remote PostgreSQL without Linux collection or sudo. The `local` mode collects Linux metrics from the same VM or pod and, optionally, PostgreSQL. See [collector configuration](bins/kronika-collector/README.md).
- Collect all visible, accessible cgroup v2 groups and show them in CPU, Memory, I/O and Tasks tables with search, sorting and history. Groups stay separate; limits distinguish unlimited from unavailable. Older recordings remain readable.
- Read PostgreSQL 12 and older `pg_stat_statements` layouts without requesting absent fields. Hide Activity metrics unsupported by the recording.
- Keep Activity heatmaps within their recorded time window in partial-hour HTML reports. Cell clicks and row navigation select the corresponding recorded interval instead of jumping to the report start.

## [1.0.1](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.1) — 2026-09-06

- Clarify installation and startup in English and Russian, including local PostgreSQL setup and which machine supplies Linux measurements with a remote PostgreSQL connection.

## [1.0.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.0) — 2026-09-06

- Initial release: Linux and PostgreSQL recording, web inspection and self-contained offline HTML reports. Static Linux archives for x86-64 and ARM64 include the collector, web server, dump and report tools.
