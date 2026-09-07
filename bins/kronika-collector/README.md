# kronika-collector

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

The collector saves Linux and PostgreSQL metrics and events from local
PostgreSQL/PgBouncer logs. New data first goes into a journal, `active.wal`.
When the recording reaches its configured size or age, the collector saves
that portion as a compressed `.zms` file, called a segment, and continues.

Set options through environment variables before starting the program.
`KRONIKA_STORAGE_DIR`, the data directory, is required. Options are read once
at startup; an invalid value stops startup. Changes take effect on the next
start; [service configuration](../../docs/services.md) covers an existing service. Sources:
[configuration](src/config.rs), [collection schedule](src/scheduler.rs),
[main loop](src/main.rs).

[Storage failures and recovery](../../docs/storage-recovery.md) explains what
happens after an abrupt stop or when a recording cannot be read.

## Configuration

<a id="storage"></a>
### Storage

| Variable | Default | Accepted value and meaning |
| --- | --- | --- |
| `KRONIKA_STORAGE_DIR` | Required | Directory in which collected data is saved; use a directory, not a symbolic link. |
| `KRONIKA_SEGMENT_MAX_BYTES` | `67108864` (64 MiB) | Journal size in bytes at which a compressed segment becomes due. Positive whole number. |
| `KRONIKA_SEGMENT_MAX_AGE_S` | `900` | Seconds after a segment starts before it becomes due. Nonnegative whole number; `0` makes it eligible immediately. |
| `KRONIKA_JOURNAL_MAX_BYTES` | `1073741824` (1 GiB) | Maximum journal size in bytes: `36..1073741824`. Reaching it saves the segment early. |
| `KRONIKA_RETENTION` | `2147483648` (2 GiB) | Storage target in bytes, or `auto` (= `auto:80`), or `auto:P`, where `P` is a whole percentage from 1 to 99. |

A fixed budget counts the active journal, compressed recordings, their `.idx`
index files and collector temporary files. It must be at least twice
`KRONIKA_SEGMENT_MAX_BYTES`. For example, `KRONIKA_RETENTION=10737418240` sets
10 GiB. With `auto:P`, old recordings are removed when more than `P` percent
of the entire backing filesystem is used. `auto` means 80 percent; this
includes space used by other programs.

The collector checks after saving segments and on a one-minute timer; a
collection in progress can delay the check. It removes leftover temporary
files first, then indexes without a recording, then the oldest finished
recordings with their indexes. The active journal, newest finished segment
and unrelated files are retained. If these still exceed the target,
collection continues and logs `rotation_degraded`. Fixed mode also recounts
files hourly to include new indexes created by web.
[Rotation implementation](src/rotation.rs) defines the byte accounting and deletion.

### Collection mode

`KRONIKA_COLLECTOR_MODE=local` is the default: Linux metrics and optional local
PostgreSQL. PostgreSQL must share the recorded machine and PID namespace for
process links. In containers, the collected cgroup can include other containers;
it does not establish shared PostgreSQL processes or CPU capacity.

`KRONIKA_COLLECTOR_MODE=postgresql` records only PostgreSQL from a local or
remote server. It does not read
procfs/sysfs, host identity, Linux processes or cgroups. `KRONIKA_PG_DSNS` is
required. Root access is unnecessary; the process needs network and storage
access. Explicit `KRONIKA_PG_LOGS` paths are optional. PgBouncer log settings are
not accepted in this mode. OS intervals do not apply.

### Collection intervals

Intervals are nonnegative whole seconds. Each source has its own schedule.
A source interval of `0` reads on every timer wakeup. A cgroup is a Linux group
of processes with shared resource limits; PSI measures time spent waiting for
CPU, memory or I/O resources.

| Variable | Default, s | Data |
| --- | ---: | --- |
| `KRONIKA_INTERVAL_S` | 5 | Maximum timer sleep; `0` disables timed collection. A shorter positive source interval can wake the timer earlier. |
| `KRONIKA_OS_CORE_INTERVAL_S` | 10 | CPU, memory, disks, network, PSI. |
| `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 | Mounts, filesystem capacity and device topology. |
| `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 | Process counters. |
| `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 | Process status details. |
| `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 | Resource limits and use of the highest accessible cgroup v2 ancestor. |
| `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | Process-to-cgroup v2 mappings. |
| `KRONIKA_LOG_INTERVAL_S` | 10 | Configured PostgreSQL/PgBouncer logs. |
| `KRONIKA_PG_INTERVAL_S` | 30 | PostgreSQL metrics and settings. |
| `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 | Tables and indexes. |

### Connections and logs

A DSN is a database connection string, written as `key=value` pairs or a URL.
Separate connection strings and paths with semicolons (`;`).

| Variable | Default | Meaning |
| --- | --- | --- |
| `KRONIKA_PG_DSNS` | Unset | PostgreSQL connection strings. The first enables server metrics. In `local` mode, each also discovers its current local log. Required in `postgresql` mode. |
| `KRONIKA_PG_SSL_ROOT_CERT` | Unset | PEM CA bundle replacing the included public CA roots. When unset, the included public roots are used. TLS validates the server hostname in both cases. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Unset | Available CPUs of the first PostgreSQL server: integer `1..4294967295`; requires `KRONIKA_PG_DSNS`. Automatic only for a recorded shared local machine. Without capacity, SQL metrics continue and PostgreSQL Health is unknown. |
| `KRONIKA_PG_LOGS` | Unset | Optional readable local paths; final filename supports `*` and `?`. In `local` mode, paths add to `pg_current_logfile()` discovery. In `postgresql` mode, only explicit paths are opened. |
| `KRONIKA_PGBOUNCER_DSNS` | Unset | Connections to the administrative console (`dbname=pgbouncer`) to read `SHOW CONFIG`/`logfile`; the account must belong to `stats_users`. |
| `KRONIKA_PGBOUNCER_LOGS` | Unset | Local PgBouncer log paths; filenames can use `*` and `?` wildcards. |

Blank lists add no explicit entries. Blank entries between semicolons are errors.
The first PostgreSQL DSN supplies metrics from its initial database and other
accessible databases on that server, excluding template databases. Further
DSNs supply log discovery only in `local` mode. PostgreSQL metric rows have no separate field identifying the server.

### Other settings

| Variable | Default | Meaning |
| --- | --- | --- |
| `KRONIKA_LOG_LEVEL` | `info` | Logging detail: case-insensitive `error`, `warn`/`warning`, `info`, `debug`, `trace`; messages go to standard error (stderr). |
| `KRONIKA_PROC_ROOT` | `/proc` | Directory containing process information from procfs; setting it limits container detection to that directory’s cgroup file. |
| `KRONIKA_SYS_ROOT` | `/sys` | Directory containing kernel device information from sysfs. |
| `KRONIKA_STATVFS_FIXTURE` | Unset | Filesystem values for tests: `path=TOTAL:FREE:INODES:AVAILABLE_INODES;...` substitutes `statvfs` values. |

## PostgreSQL collection

### PostgreSQL CPU capacity

In `local` mode on a machine/VM shared with PostgreSQL, leave
`KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset. Each Activity sample uses the count of
distinct CPUs in the latest complete recorded CPU snapshot at or before it.
The DSN address does not establish placement.

For remote PostgreSQL or container deployments, an optional positive whole
number supplies the PostgreSQL CPU capacity. A broader cgroup aggregate does
not give the capacity of its PostgreSQL child. Without capacity, SQL collection
continues; PostgreSQL Health and capacity-dependent marks are unknown. Web
reads the recorded value and has no separate CPU setting.

<a id="remote-postgresql"></a>
### PostgreSQL only — local or remote

PostgreSQL-only collection does not need sudo.

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
  /usr/local/bin/kronika-collector
```

Add `KRONIKA_POSTGRES_EFFECTIVE_CPUS=4` when that server has 4 available CPUs.
Use `KRONIKA_PG_SSL_ROOT_CERT=/path/to/ca.pem` for a private CA. The DSN accepts
`sslmode=disable` (plaintext), `prefer` (TLS when available; default), or
`require` (TLS required). Every TLS connection checks the CA and hostname,
including reconnects and query cancellation. `verify-full` is not accepted DSN syntax.

Start web over the same recording with `KRONIKA_WEB_SOURCES=2`. This declares
PostgreSQL in the catalog; the collector mode controls acquisition. Overall equals
PostgreSQL Health, or is unknown when PostgreSQL Health cannot be calculated.

Process links in Activity, Vacuum and Processes require recorded shared-process
metadata for the selected segment. New local machine recordings supply it;
PostgreSQL-only, container and older recordings do not. Their PostgreSQL and OS
rows remain readable independently, without a link based only on matching PIDs.
See [Health formulas](../../docs/metrics-time.md#health) and
[container collection](../../docs/metrics-linux.md#container-cgroups).

<a id="postgresql-role"></a>
### PostgreSQL role

The PostgreSQL account needs these privileges. The [setup examples](../../INSTALL.md#5-postgresql)
show how to grant them to a monitoring role:

| Scope | Required privilege |
| --- | --- |
| Role | Inherited `pg_monitor` membership; collector does not issue `SET ROLE`. |
| Each collected database | `CONNECT`; normal catalog read/function access. |
| Selected extension schema | `USAGE`. |
| `pg_stat_statements` reader | `EXECUTE` on `pg_stat_statements(boolean)`. |
| `pg_store_plans` reader | `EXECUTE` on installed `pg_store_plans()` or `pg_store_plans(boolean)`; vadv interface also needs `pg_store_plans_get_plan(oid, oid, bigint, bigint)` and `pg_store_plans_textplan(text)`. |
| Installed `*_info` interface | `SELECT` on the info view and `EXECUTE` on its zero-argument function. |
| Each PostgreSQL log-discovery database | `EXECUTE` on `pg_catalog.pg_current_logfile()` and `pg_catalog.pg_control_system()`. |

Schema, view and function privileges are database-local. Extension readers are
called directly. Default PostgreSQL/extension grants supply some of these
permissions; explicitly revoked privileges must be granted to the monitoring
role. The explicit `pg_current_logfile()` grant is needed on PostgreSQL 10–16.

### Discovery and session lifetime

| Item | Contract |
| --- | --- |
| Database sessions | One reused connection per connectable database, replaced after at most one hour while healthy. The database and extension list is normally refreshed every five minutes, on a PostgreSQL or tables/indexes collection pass. Forced collection and failed discovery can cause earlier retries. |
| Extension inventory | One query per database on each discovery pass; the schema and available functions are remembered. One usable installation of each extension is selected. |
| `pg_stat_statements` | Supports extension `1.5+` in the `1.x` series; PostgreSQL 14+ requires `1.9+`. The newest compatible set of fields wins, then current database, then database name. |
| `pg_store_plans` | OSSC and Datasentinel return different fields through a function with no arguments; the vadv boolean interface requires its four-key plan lookup function and plan-to-text converter. Implementation selection uses current database, then database name. |
| Info views | `pg_stat_statements_info` and `pg_store_plans_info` are discovered independently of the main readers. |
| Settings | Read each PostgreSQL tick; full snapshot after first successful read, on change, and in every segment. Latest successful snapshot is reused when other sources open a segment. |
| Settings exclusions | `primary_conninfo` and `ssl_passphrase_command` are omitted; other command and custom settings are recorded. |

Sources: [database pool](../../crates/kronika-source-pg/src/pool.rs),
[extension discovery](../../crates/kronika-source-pg/src/extension.rs),
[settings](../../crates/kronika-source-pg/src/settings.rs),
[recorded layouts](../../docs/type-registry/postgresql-metrics.md).

### Query execution

| Item | Value or behavior |
| --- | --- |
| Transport | `sslmode=require` uses validated TLS; `prefer` (default) allows plaintext when TLS is unavailable; `disable` uses plaintext. Direct PostgreSQL or PgBouncer session pooling. Transaction/statement pooling do not retain the session state required by metric reads. |
| Protocol | Administrative queries use Simple Query Protocol. Metrics with known field types use a one-shot unnamed query through Extended Protocol. One query at a time per connection. |
| Session initialization | `SET statement_timeout = '30s'; SET lock_timeout = '100ms'` in one request before any monitoring query, including log discovery; repeated on every new connection. |
| Client fetch deadline | 35 seconds, then a CancelRequest attempt with a one-second deadline and connection close. |
| Collector identity | One unique `application_name` per collector process; Activity/Locks exclude that exact name. |
| Batch bounds | At most 256 rows, targeting 512 KiB of decoded data; the final row, bounded by the SQL query, can exceed the byte target. Each batch reaches the recording journal before the next is read. |
| Text bounds | Statement and plan text limited to 65,536 characters in SQL. |
| Stream error | Earlier appended batches remain; the remaining read is skipped and independent sources continue. |
| SQLSTATE `57014` | Counted as query timeout; session is reusable after `ReadyForQuery`. |
| Query logs | Debug `pg_query_finish`; warning `pg_query_slow` when fetch exceeds 500 ms; summary about every five minutes and at shutdown. |

`lock_timeout` limits each lock acquisition wait to 100 ms.
It does not limit how long an acquired lock is held.
The overall statement deadline remains 30 s (`statement_timeout`). A lock-wait
error (`55P03`) is logged, independent sources continue, and the read is tried
again on a later scheduled pass. These limits apply only to Kronika monitoring
sessions.
Source: [PostgreSQL documentation](https://www.postgresql.org/docs/current/runtime-config-client.html#GUC-LOCK-TIMEOUT).

`pg_query_summary` records query count/rate, rows, logical bytes, errors,
timeouts, slow queries, fetch/encoding/WAL times, encoded/appended bytes and
`peak_rss_kib`, the peak physical memory occupied by the process in KiB in local mode; unavailable in PostgreSQL-only mode. Connection labels are `user@host:port`. Source:
[query.rs](../../crates/kronika-source-pg/src/query.rs).

## Log collection

In `postgresql` mode, SQL paths are not opened locally. Only explicit
`KRONIKA_PG_LOGS` paths are read; remote files are not downloaded.

In `local` mode, for every `KRONIKA_PG_DSNS` entry, discovery reads `pg_current_logfile()`,
`data_directory` and `log_line_prefix`. This runs even when `KRONIKA_PG_LOGS`
is unset. The SQL function returns a current log path, not historical rotation
files; null supplies no automatic file. A relative path is resolved against
that PostgreSQL server's `data_directory`. The resulting file must be readable
on the collector host; the collector does not fetch files from a remote server.

`KRONIKA_PG_LOGS` adds local paths or filename patterns to the discovered sources. An identical
path is followed once, retaining discovered `system_identifier` and
`log_line_prefix` when available. Files described as “path-only” below were not found through
a database connection. Discovery requires the [function privileges](#postgresql-role)
listed above.

| Property | Behavior |
| --- | --- |
| Discovery cadence | First collection cycle, then on the first collection cycle at least five minutes after the preceding scan; retries after errors. `system_identifier` is cached after its first successful read. |
| Read bound | 64 KiB physical buffer; batches of at most 4 MiB raw bytes; at most 256 MiB per file per collection. |
| PostgreSQL formats | Filename selects `.csv` → csvlog, `.json` → jsonlog, otherwise stderr. |
| Path-only identity | `system_identifier` is null; every row records its source file. |
| Path-only stderr | Database/user are unavailable; severity, SQLSTATE when present, message and continuations are parsed. Parsed timestamp is used when present, otherwise collection time. |
| Source error | Logged; other collection continues. |

Sources: [source discovery](src/log_sources.rs), [SQL facts and path resolution](src/log_sources/settings.rs),
[log collector](../../crates/kronika-source-log/src),
[PostgreSQL parser](../../crates/kronika-source-log/src/postgres.rs).

## Linux collection

Linux collection runs only in `local` mode. Machine/VM recordings have no
workload cgroup rows. Cgroup collection requires cgroup v2. On v1-only systems,
cgroup metrics and process mappings are unavailable; other enabled Linux and
PostgreSQL sources continue. In a container, collector walks upward from its own
membership to the highest visible, readable ancestor within the accessible
mount. Counters describe that group, including its children. The selected
path can represent a pod, another aggregate or only collector's own group;
its path is shown without assuming a pod or PostgreSQL identity. Missing
metrics remain unavailable. See the [Linux reference](../../docs/metrics-linux.md#container-cgroups).

Filesystem capacity is queried for `ext2`, `ext3`, `ext4`, `xfs`, `btrfs`,
`f2fs`, `zfs`, `tmpfs` and `overlay`. Other types retain null capacity fields.
One helper process handles supported mounts under a shared one-second
deadline. Mount rows record their exact roots, space in bytes and available
file metadata entries (inodes). Device relationships connect partitions to
devices and layered devices to their underlying devices. In containers, these
relationships are limited to chains of mounted devices or devices accounted
for by cgroup I/O statistics.

## Run and signals

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika /usr/local/bin/kronika-collector
```

`SIGINT` and `SIGTERM` stop collection and retain the journal. `SIGUSR2` collects
immediately and requests segment publication when the cycle appends data and the
segment is nonempty. `-h`, `--help` and `--version` exit before
configuration or storage access. Readiness and segment paths go to stdout;
structured logs go to stderr. In local mode, each `segment_write_finish` records `rss_kib`,
the peak physical memory occupied by the process in KiB.
