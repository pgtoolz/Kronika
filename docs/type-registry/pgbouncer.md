# Class 2: PgBouncer log events

[Русская версия](pgbouncer.ru.md)

`pgbouncer_events` contains one row per event. Layout `2_100_002` adds connection context to `2_100_001`. Both remain readable. See the [codec](../../crates/kronika-registry/src/codec/pgbouncer_events.rs) and [Events reference](../features.md#events).

## Source and fields

The collector discovers `logfile` through `SHOW CONFIG` using `--pgbouncer-dsn` or `KRONIKA_PGBOUNCER_DSNS`. `--pgbouncer-log` or `KRONIKA_PGBOUNCER_LOGS` adds local paths and patterns. It does not collect `SHOW POOLS`, `SHOW STATS` or `SHOW CLIENTS`.

| Field | Nullable | Definition |
| --- | --- | --- |
| `ts` | no | Unix microseconds. Source time resolves UTC, numeric offsets and IANA zones, otherwise the collector's local zone is used. Missing or invalid time uses the batch read time. |
| `source_file` | no | File read. |
| `level` | no | `0` FATAL, `1` ERROR, `2` WARNING, `3` LOG, `4` DEBUG, `5` NOISE. |
| `database` | yes | Database section in `pgbouncer.ini`. |
| `username` | yes | Login user or peer literal. |
| `host` | yes | Client/server address, without an unambiguous port suffix. IPv6 brackets and `unix(<pid>)` are retained. |
| `pid` | yes | PgBouncer process ID. |
| `side` | yes | `C` client, `S` server. |
| `port` | yes | Connection port. Zero is retained. |
| `age_s` | yes | Connection age in whole seconds, when printed. |
| `text` | no | Full message and tab-prefixed continuations, bounded to 5 KiB. |

PID, side, port and age are unavailable in the previous layout. Missing context stays null. `(nodb)`, `(nouser)` and peer names remain literal values.

## Message selection

All recognizable WARNING, ERROR and FATAL messages are retained, including `pooler error:`. Unknown LOG messages, signals, reloads and unexpected disconnects are retained. DEBUG and NOISE are skipped.

Routine LOG connection chatter and numeric periodic statistics are skipped. Normal closing reasons are `client close request`, `server idle timeout` and `server lifetime over`. Unknown reasons and malformed age suffixes remain visible.

```text
2026-09-21 12:34:56.789 [12345] WARNING C-0x55f1: db/user@10.0.0.1:41537 closing because: query timeout (age=42s)
```

This records PID `12345`, side `C`, port `41537`, age `42` and the full message `closing because: query timeout (age=42s)`. Events normalizes the closing wrapper and valid age only for grouping. The representative detail retains the stored message.

Read bounds and offsets: [log reading](postgresql.md#read-bounds). Sources: [parser](../../crates/kronika-source-log/src/pgbouncer.rs), [time](../../crates/kronika-source-log/src/timestamp.rs).
