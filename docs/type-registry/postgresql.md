# Class 2: PostgreSQL log events

[Русская версия](postgresql.ru.md)

Sections occupy `2_001_001`–`2_099_999`. The [codec](../../crates/kronika-registry/src/codec/pg_log.rs) defines fields and units; the [Events reference](../features.md#events) defines UI grouping.

## Sources and formats

Using the connections in `KRONIKA_PG_DSNS`, the collector discovers log files through `pg_current_logfile()`. Paths and patterns in `KRONIKA_PG_LOGS` add files to that set. The collector reads the files locally; they must be accessible on its host. A file reached by both methods is read once. A previously unread file starts at its beginning; a known file resumes at its saved offset.

| Format by filename | Parsing |
| --- | --- |
| `.csv` | csvlog: positional fields; appended columns ignored. Open quotes continue the record onto the next line. |
| `.json` | jsonlog: one JSON object per line. |
| Any other name | stderr: severity, message and continuation fields. `log_line_prefix` is discovered through a DSN; an explicit path without a known prefix has absent database and user. |

`system_identifier` comes from `pg_control_system()` for a DSN source; it is `null` for an explicit path. `source_file` stores the filename. The filename suffix selects the format without content detection.

In stderr, `DETAIL:`, `HINT:`, `CONTEXT:`, `STATEMENT:` and `QUERY:` lines attach to the record; a leading tab continues the previous field. `STATEMENT:` and `QUERY:` are stored in `statement`.

Text log times are parsed in the collector host's local timezone; the printed zone designation is skipped. An ambiguous clock resolves to its first occurrence. If the timestamp is absent or unparseable, the PostgreSQL parser uses collection time. Fractional seconds retain microsecond precision. Correct text-log timing requires `log_timezone` to match the collector host zone.

## Registered types

`event_stream` denotes events or grouped repetitions, rather than a complete state snapshot. `type_id` selects the section’s field layout.

| `type_id` | Section | Semantics | Sort key |
|-----------|---------|-----------|----------|
| `2_001_001` | `pg_log_errors` | `event_stream` | `(severity, category, pattern, ts)` |
| `2_002_001` | `pg_log_checkpoints` | `event_stream` | `(ts, phase)` |
| `2_003_001` | `pg_log_autovacuum` | `event_stream` | `(ts, kind, relation)` |
| `2_004_001` | `pg_log_slow_queries` | `event_stream` | `(pattern, ts)` |
| `2_005_002` | `pg_log_lock_waits` | `event_stream` | `(ts, kind, pid)` |
| `2_006_001` | `pg_log_lifecycle` | `event_stream` | `(ts, kind)` |
| `2_007_001` | `pg_log_temp_files` | `event_stream` | `(ts, size_bytes)` |

## Collection grouping

| Section | Key within one read batch | Stored sample |
| --- | --- | --- |
| `pg_log_errors` | `(severity, category, pattern)` | First occurrence with original values. Pattern normalizes quoted names, bracketed/parenthesized lists, numbers after `transaction`/`relation`/`process`/`PID`/`signal`, and WAL addresses. |
| `pg_log_slow_queries` | SQL with normalized literals | Slowest occurrence and its duration. |

A group crossing a read-batch boundary produces a separate row in each batch. Other sections contain individual events.

## Recognized records

`WARNING` and higher severities produce error groups. `LOG` recognizes the following English forms; other `LOG` messages are omitted. Localized messages retain error groups but do not match the English `LOG` forms. Extended Protocol duration messages support `statement:` and `execute`; `parse` and `bind` are omitted.

| Section | Message prefix/shape | Source setting |
| --- | --- | --- |
| `pg_log_checkpoints` | `checkpoint starting:`, `checkpoint complete:`, `checkpoints are occurring too frequently` | `log_checkpoints` |
| `pg_log_autovacuum` | `automatic vacuum of table`, `automatic analyze of table`, including aggressive and anti-wraparound forms | `log_autovacuum_min_duration` |
| `pg_log_slow_queries` | `duration: <ms> ms  statement: <sql>`, `duration: <ms> ms  execute <name>: <sql>` | `log_min_duration_statement` |
| `pg_log_lock_waits` | `process <pid> still waiting for`, `process <pid> acquired`; PID lists from `holding the lock:` and `Wait queue:` → `holding_pids`, `wait_queue` | `log_lock_waits` |
| `pg_log_lifecycle` | `server process (PID …) was terminated`, `received … shutdown request`, `database system is ready to accept connections` | Server lifecycle messages |
| `pg_log_temp_files` | `temporary file: path …, size …` | `log_temp_files` |

## Error categories

`PANIC` always receives category `5`. A message containing `terminated by signal` and `: killed` receives `4`. Otherwise the first matching category in table order is selected; unmatched `FATAL` receives `6`, other unmatched messages receive `10`.

| Code | Category | Content |
| ---: | --- | --- |
| `0` | lock | Deadlock, lock timeout, lock wait. |
| `1` | constraint | Duplicate key, foreign key, not-null, check, exclusion. |
| `2` | serialization | `could not serialize access`. |
| `3` | timeout | Statement, transaction and idle-session timeouts; cancellation. |
| `4` | resource | Memory, connection slots, disk, OOM kill. |
| `5` | data corruption | Bad pages, unreadable blocks, every `PANIC`. |
| `6` | system | Files, I/O, crashes, remaining `FATAL`. |
| `7` | connection | Resets, unexpected EOF, broken pipes. |
| `8` | auth | Passwords, `pg_hba.conf`, permissions. |
| `9` | syntax | Syntax, missing objects, bad input. |
| `10` | other | Remaining errors. |

## Read bounds

The read buffer is 64 KiB; a batch consumes up to 4 MiB of raw file bytes. One collection reads up to 256 MiB from each file; remaining input waits for the next collection. A retained raw record is bounded to 64 KiB, message/statement/continuation text to 5 KiB, and a pattern to 256 bytes. Truncated CSV records are omitted because field boundaries after truncation are unresolved.

Sources: [parser](../../crates/kronika-source-log/src/postgres.rs), [time](../../crates/kronika-source-log/src/timestamp.rs), [tail](../../crates/kronika-source-log/src/tail.rs), [collector loop](../../bins/kronika-collector/src/log_sources.rs).
