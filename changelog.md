# Changelog

[Русская версия](changelog.ru.md)

## 1.1.0 — Unreleased

Changes since the last published release, 1.0.1.

### Planned

- Use `KRONIKA_PG_DSN` for one PostgreSQL server. To monitor several servers, run the same `kronika-collector` binary as a separate process for each server, with its own `KRONIKA_PG_DSN` and `KRONIKA_STORAGE_DIR`. Each process collects the accessible databases on its server.
- Stagger scheduled ZMS segment closing with a per-directory offset that survives restarts. The first segment may be shorter; later scheduled segments retain the configured period. Size limits, forced closing and recovery are unchanged. Simultaneous closes remain possible.

`KRONIKA_PG_DSNS` is deprecated: only the first DSN is used; the parameter will be removed. Use `KRONIKA_PG_DSN`.

### Other changes

- Add `KRONIKA_COLLECTOR_MODE=postgresql` for local or remote PostgreSQL without Linux collection or sudo. The existing `local` mode retains OS collection from the same VM or pod, with optional PostgreSQL. See [collector configuration](bins/kronika-collector/README.md).
- Collect all visible, accessible cgroup v2 groups and show their CPU, Memory, I/O and Tasks in tables with search, sorting and history. Keep each group separate and distinguish unlimited from unavailable limits. Older recordings remain readable.
- Read PostgreSQL 12 and older `pg_stat_statements` layouts without requesting absent fields. Use `total_time` for older statement layouts and hide Activity metrics unsupported by the recording.
- Keep Activity heatmaps within their recorded time window in partial-hour HTML reports. Cell clicks and row navigation select the corresponding recorded interval instead of jumping to the report start.

## [1.0.1](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.1) — 2026-09-06

- Clarify installation and startup in English and Russian, including local PostgreSQL setup and which machine supplies Linux measurements with a remote PostgreSQL connection.

## [1.0.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.0) — 2026-09-06

- Initial release: Linux and PostgreSQL recording, web inspection and self-contained offline HTML reports. Static Linux archives for x86-64 and ARM64 include the collector, web server, dump and report tools.
