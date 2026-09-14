# Changelog

[Русская версия](changelog.ru.md)

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
