# Class 1: PostgreSQL snapshots

[Русская версия](postgresql-metrics.ru.md)

Sections occupy `1_001_001`–`1_020_001`. Columns, units and keys are declared in the [registry codec](../../crates/kronika-registry/src/codec); derived values are defined in the [PostgreSQL reference](../metrics-postgresql.md).

## Collection scope and protocol

Metrics come from the first instance in `KRONIKA_PG_DSNS`; metric rows do not carry `system_identifier`. Database-local views are read from each connectable database. An absent, unsupported or unreadable source produces no section for that read.

The collector retains one connection per database between cycles. Database and extension discovery runs approximately every five minutes and updates the connection set.

| Read | Contract |
| --- | --- |
| Administrative queries | Simple Query Protocol. |
| Typed metrics | Sequential unnamed Extended Protocol queries; no named prepared statements or pipelining. |
| Result batch | At most 256 rows; approximately 512 KiB decoded-data target. The final row may exceed the byte target. The batch reaches WAL before the next row is fetched. |
| Query/plan text | SQL bounds each field to its first 65,536 characters; all eligible rows are read without top-N or a shared text budget. |
| Timeout | One bounded CancelRequest, then connection closure. |

## Native server views

`type_id` selects the section’s field layout. `snapshot_full` records a full snapshot on each collection; `conditional_full` records a full snapshot on cycles when the source’s collection condition holds; `on_change` records a changed state or a scheduled refresh. Additional settings emission rules appear below.

| `type_id` | Section | PostgreSQL | Collection scope | Semantics |
|---|---|---|---|---|
| `1_001_001` | `pg_stat_activity` | 10–12 | instance | `snapshot_full` |
| `1_001_002` | `pg_stat_activity` | 13 | instance | `snapshot_full` |
| `1_001_004` | `pg_stat_activity` | 14–18 | instance | `snapshot_full` |
| `1_005_001` | `pg_stat_database` | 10–11 | instance | `snapshot_full` |
| `1_005_002` | `pg_stat_database` | 12–13 | instance | `snapshot_full` |
| `1_005_003` | `pg_stat_database` | 14–17 | instance | `snapshot_full` |
| `1_005_004` | `pg_stat_database` | 18 | instance | `snapshot_full` |
| `1_006_001` | `pg_stat_bgwriter` | 10–16 | instance | `snapshot_full` |
| `1_006_002` | `pg_stat_bgwriter` | 17–18 | instance | `snapshot_full` |
| `1_007_001` | `pg_stat_wal` | 14–17 | instance | `snapshot_full` |
| `1_007_002` | `pg_stat_wal` | 18 | instance | `snapshot_full` |
| `1_008_001` | `pg_stat_archiver` | 10–18 | instance | `snapshot_full` |
| `1_009_001` | `pg_stat_io` | 16–17 | instance | `snapshot_full` |
| `1_009_002` | `pg_stat_io` | 18 | instance | `snapshot_full` |
| `1_010_001` | `pg_prepared_xacts` | 10–18 | instance | `snapshot_full` |
| `1_011_001` | `pg_locks` | 10–13 | instance | `conditional_full` |
| `1_011_002` | `pg_locks` | 14–18 | instance | `conditional_full` |
| `1_012_004` | `pg_stat_progress_vacuum` | 10–16 | instance | `conditional_full` |
| `1_012_005` | `pg_stat_progress_vacuum` | 17 | instance | `conditional_full` |
| `1_012_006` | `pg_stat_progress_vacuum` | 18 | instance | `conditional_full` |
| `1_013_005` | `pg_stat_user_tables` | 10–12 | each database | `snapshot_full` |
| `1_013_006` | `pg_stat_user_tables` | 13–15 | each database | `snapshot_full` |
| `1_013_007` | `pg_stat_user_tables` | 16–17 | each database | `snapshot_full` |
| `1_013_008` | `pg_stat_user_tables` | 18 | each database | `snapshot_full` |
| `1_014_003` | `pg_stat_user_indexes` | 10–15 | each database | `snapshot_full` |
| `1_014_004` | `pg_stat_user_indexes` | 16–18 | each database | `snapshot_full` |
| `1_017_001` | `pg_stat_checkpointer` | 17 | instance | `snapshot_full` |
| `1_017_002` | `pg_stat_checkpointer` | 18 | instance | `snapshot_full` |
| `1_019_001` | `pg_settings` | 10–18 | metric session | `on_change` |
| `1_020_001` | `pg_wal_storage` | 10–18 | instance | `snapshot_full` |

## Rows and field scope

| Section | Contract |
| --- | --- |
| `pg_stat_activity` | PID identifies a backend. PostgreSQL 14–18 reads nullable `datid` and `query_id` directly from the view. `datid` participates in navigation to Statements; shared/background backends may have `null`. |
| `pg_stat_progress_vacuum` | `relid` is a `pg_class` OID; `schemaname`/`relname` resolve from the catalog in the same query only for the connected database. Relations in other databases have absent names. |
| `pg_settings` | Effective metric-session settings; identity `(datid, usesysid, name)`, database and role names `datname`, `usename`. Read each PostgreSQL cycle; emitted on first success, change and new segment. A segment opened by another source reuses the latest successful snapshot. `primary_conninfo` and `ssl_passphrase_command` are excluded; other settings remain. |
| `pg_wal_storage` | Sum of regular-file sizes returned by `pg_ls_waldir()`; subdirectories excluded. The section is absent without permission to call the function. |
| Tables and indexes | Zero `reltablespace` resolves through the database's `dattablespace`. A storage-less partitioned parent has no placement. An index records its own tablespace. Table size includes heap main fork and TOAST; user indexes are excluded. |

## Extension views

| `type_id` | Section | Extension | Collection scope | Semantics |
|---|---|---|---|---|
| `1_002_001` | `pg_stat_statements` | `pg_stat_statements` 1.5–1.7 | discovered installation | `conditional_full` |
| `1_002_002` | `pg_stat_statements` | `pg_stat_statements` 1.8 | discovered installation | `conditional_full` |
| `1_002_003` | `pg_stat_statements` | `pg_stat_statements` 1.9 | discovered installation | `conditional_full` |
| `1_002_004` | `pg_stat_statements` | `pg_stat_statements` 1.10 | discovered installation | `conditional_full` |
| `1_002_005` | `pg_stat_statements` | `pg_stat_statements` 1.11 | discovered installation | `conditional_full` |
| `1_002_006` | `pg_stat_statements` | `pg_stat_statements` 1.12+ | discovered installation | `conditional_full` |
| `1_003_001` | `pg_store_plans_ossc` | OSSC-compatible zero-argument reader | discovered installation | `conditional_full` |
| `1_004_001` | `pg_store_plans_vadv` | vadv-compatible boolean reader, four-key getter, and native text converter | discovered installation | `conditional_full` |
| `1_015_001` | `pg_stat_statements_info` | `pg_stat_statements` 1.9+ | discovered installation | `snapshot_full` |
| `1_016_001` | `pg_store_plans_info` | exact readable `dealloc, stats_reset` view | discovered installation | `snapshot_full` |
| `1_018_001` | `pg_store_plans_datasentinel` | Datasentinel-compatible zero-argument reader with `relids` and `cmd_type` | discovered installation | `conditional_full` |

Discovery selects one usable installation per extension. `pg_stat_statements` selects the newest supported layout by extension version. `pg_store_plans` checks function signatures and result columns; the current database takes precedence, followed by database name. Readable info views are selected independently of the main reader.

`pg_stat_statements` requires version 1.5 or later and `pg_read_all_stats`; rows with a privilege-masked `queryid` are omitted. A separately readable `pg_stat_statements_info` does not require that role. `pg_store_plans_info` is selected by the exact `dealloc, stats_reset` shape; the vadv interface does not provide it.

Datasentinel adds `relids` and `cmd_type` to OSSC counters. vadv carries internal `queryid` and `queryid_stat_statements`; plan retrieval requires four keys and an executable non-set-returning `text -> text` converter from the same extension. The converter in the discovered schema wraps the getter, then SQL bounds the readable plan text to 65,536 characters.

Source: [PostgreSQL collector](../../crates/kronika-source-pg/src).
