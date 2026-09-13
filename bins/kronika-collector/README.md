# kronika-collector

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

The collector records Linux and PostgreSQL metrics and local PostgreSQL/PgBouncer
log events. It appends data to `active.wal` and periodically saves it as a
compressed `.zms` segment.

Set environment variables before starting the program. `KRONIKA_STORAGE_DIR`
is required; invalid values stop startup. Restart the process to apply changes.
For systemd, see [service configuration](../../docs/services.md).

[Storage failures and recovery](../../docs/storage-recovery.md) explains what
happens after an abrupt stop or when a recording cannot be read.

## Configuration

<a id="storage"></a>
### Storage

| Variable | Default | Accepted value and meaning |
| --- | --- | --- |
| `KRONIKA_STORAGE_DIR` | Required | Directory in which collected data is saved; use a directory, not a symbolic link. |
| `KRONIKA_SEGMENT_MAX_BYTES` | `67108864` (64 MiB) | Journal size in bytes at which a compressed segment becomes due. Positive whole number. |
| `KRONIKA_SEGMENT_MAX_AGE_S` | `900` | Period in seconds for scheduled segment closing. Nonnegative whole number; `0` makes a segment eligible immediately. |
| `KRONIKA_JOURNAL_MAX_BYTES` | `1073741824` (1 GiB) | Maximum journal size in bytes: `36..1073741824`. Reaching it saves the segment early. |
| `KRONIKA_RETENTION` | `2147483648` (2 GiB) | Storage target in bytes, or `auto` (= `auto:80`), or `auto:P`, where `P` is a whole percentage from 1 to 99. |

Collectors close ZMS files on a schedule with a random time offset. The offset
survives restarts; closes can still coincide.

The first segment after startup or an early close may be shorter. Ongoing
collection can delay closing; size limits and `SIGUSR2` can close it sooner.
`KRONIKA_INTERVAL_S=0` disables collection and segment closing by timer.

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

### Collection mode

Each `kronika-collector` process saves data from one PostgreSQL server in its
own storage directory. One DSN covers that server's accessible databases;
use separate processes for separate servers, including primary and standby.

`KRONIKA_COLLECTOR_MODE=local` is the default: Linux metrics and optional
PostgreSQL in the same VM or pod. Without a DSN, PostgreSQL metrics are not
collected; explicitly configured local logs can still be read.
Process links require PostgreSQL to run on the same machine and in
the same PID namespace as the collector. In containers, the selected cgroup can
include other containers; its metrics do not establish PostgreSQL process
identity or CPU capacity.

`KRONIKA_COLLECTOR_MODE=postgresql` records only PostgreSQL data from a local or
remote server. It does not collect Linux metrics, processes or cgroups.
`KRONIKA_PG_DSN` is required. The process needs access to PostgreSQL and write
access to storage. You can add local log files with
`KRONIKA_PG_LOGS`. PgBouncer log settings are not accepted in this mode.
Linux collection intervals do not apply.

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
| `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 | Discover all accessible cgroup v2 groups and read their resource counters and limits. |
| `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | Process-to-cgroup v2 mappings. |
| `KRONIKA_LOG_INTERVAL_S` | 10 | Configured PostgreSQL/PgBouncer logs. |
| `KRONIKA_PG_INTERVAL_S` | 30 | PostgreSQL metrics and settings. |
| `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 | Tables and indexes. |

### Connections and logs

A DSN is a database connection string, written as `key=value` pairs or a URL.
`KRONIKA_PG_DSN` accepts one connection string. Separate PgBouncer DSNs and log
paths with semicolons (`;`).

| Variable | Default | Meaning |
| --- | --- | --- |
| `KRONIKA_PG_DSN` | Unset | One PostgreSQL connection string for server metrics and accessible databases. In `local` mode, the same DSN discovers the current local log. Required in `postgresql` mode. |
| `KRONIKA_PG_SSL_ROOT_CERT` | Unset | PEM CA bundle replacing the included public CA roots. When unset, the included public roots are used. TLS validates the server hostname in both cases. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Unset | Available CPUs of the monitored PostgreSQL server: integer `1..4294967295`; requires `KRONIKA_PG_DSN`. Determined automatically in `local` mode on a machine shared with PostgreSQL. Without this count, SQL metrics continue and PostgreSQL Health is unknown. |
| `KRONIKA_PG_LOGS` | Unset | Optional readable local paths; final filename supports `*` and `?`. In `local` mode, paths add to `pg_current_logfile()` discovery. In `postgresql` mode, only explicit paths are opened. |
| `KRONIKA_PGBOUNCER_DSNS` | Unset | Connections to the administrative console (`dbname=pgbouncer`) to read `SHOW CONFIG`/`logfile`; the account must belong to `stats_users`. |
| `KRONIKA_PGBOUNCER_LOGS` | Unset | Local PgBouncer log paths; filenames can use `*` and `?` wildcards. |

Empty lists add no entries; empty entries between semicolons are errors.
The PostgreSQL DSN covers its initial database and other accessible databases
on that server, excluding template databases.

`KRONIKA_PG_DSNS` is deprecated and will be removed; only its first DSN is used.
Replace it with `KRONIKA_PG_DSN` and remove the old variable. Setting both stops startup.

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
`KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset.
Activity uses the CPU count from the last complete machine snapshot at or
before the sample. The DSN address alone does not prove that PostgreSQL
runs on this machine.

For remote PostgreSQL or container deployments, an optional positive whole
number supplies the PostgreSQL CPU capacity. A broader cgroup aggregate does
not give the capacity of its PostgreSQL child. Without capacity, SQL collection
continues; PostgreSQL Health and capacity-dependent marks are unknown. Web
reads the recorded value and has no separate CPU setting.

<a id="remote-postgresql"></a>
### PostgreSQL only — local or remote

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
  /usr/local/bin/kronika-collector
```

Add `KRONIKA_POSTGRES_EFFECTIVE_CPUS=4` when that server has 4 available CPUs.
Use `KRONIKA_PG_SSL_ROOT_CERT=/path/to/ca.pem` for a private CA. The DSN accepts
`sslmode=disable` (plaintext), `prefer` (TLS when available; default), or
`require` (TLS required). Every TLS connection checks the CA and hostname. `verify-full` is not accepted DSN syntax.

Start web over the same recording with `KRONIKA_WEB_SOURCES=2`. This declares
PostgreSQL in the catalog; the collector mode controls acquisition. Overall equals
PostgreSQL Health, or is unknown when PostgreSQL Health cannot be calculated.

In local recordings of a shared machine, Activity, Vacuum and Processes link
PostgreSQL sessions to their Linux processes. These links are unavailable in
PostgreSQL-only, container and older recordings. You can still inspect the
PostgreSQL and Linux data separately.
See [Health formulas](../../docs/metrics-time.md#health) and
[container collection](../../docs/metrics-linux.md#container-cgroups).

<a id="several-postgresql-servers"></a>
### Several PostgreSQL servers

To collect from several PostgreSQL servers, run a `kronika-collector` process
for each server with its DSN and a separate storage directory.
For two servers, start the first process in one terminal:

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-pg-a" \
  KRONIKA_PG_DSN='host=pg-a.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
  /usr/local/bin/kronika-collector
```

In the second terminal:

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-pg-b" \
  KRONIKA_PG_DSN='host=pg-b.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
  /usr/local/bin/kronika-collector
```

To view both recordings, start `kronika-web` separately for `kronika-pg-a`
and `kronika-pg-b`, using different listen addresses.

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
| Initial database used for PostgreSQL log discovery | `EXECUTE` on `pg_catalog.pg_current_logfile()` and `pg_catalog.pg_control_system()`. |

Schema, view and function privileges are database-local. Extension readers are
called directly. Default PostgreSQL/extension grants supply some of these
permissions; explicitly revoked privileges must be granted to the monitoring
role. The explicit `pg_current_logfile()` grant is needed on PostgreSQL 10–16.

### Discovery and session lifetime

| Item | Behavior |
| --- | --- |
| Database sessions | One reused connection per connectable database, replaced after at most one hour while healthy. The database and extension list is normally refreshed every five minutes, on a PostgreSQL or tables/indexes collection pass. Forced collection and failed discovery can cause earlier retries. |
| Extension inventory | Each database is checked during discovery. One usable installation of each extension is selected. |
| `pg_stat_statements` | Supports extension `1.5+` in the `1.x` series; PostgreSQL 14+ requires `1.9+`. The newest compatible set of fields is preferred. |
| `pg_store_plans` | OSSC and Datasentinel return different fields through a function with no arguments; the vadv boolean interface requires its four-key plan lookup function and plan-to-text converter. |
| Settings | Read with PostgreSQL metrics. A full snapshot is recorded after the first successful read, on change and in every segment. |
| Settings exclusions | `primary_conninfo` and `ssl_passphrase_command` are omitted; other command and custom settings are recorded. |

### Query execution

Connect directly to PostgreSQL or through PgBouncer in session pooling mode.
Transaction/statement pooling does not preserve the session settings needed
by the collector.

| Item | Value or behavior |
| --- | --- |
| Concurrency | One query at a time per connection. |
| Session limits | `statement_timeout=30s`, `lock_timeout=100ms` for all monitoring queries, including log discovery. |
| Client fetch deadline | After 35 seconds, attempt to cancel the query and close the connection. |
| Collector identity | One unique `application_name` per collector process; Activity/Locks exclude that exact name. |
| Text bounds | Statement and plan text limited to 65,536 characters in SQL. |
| Interrupted read | Data already recorded is retained. The rest of that read is skipped; other sources continue. |
| SQLSTATE `57014` | Counted as a query timeout. |
| Query logs | Debug `pg_query_finish`; warning `pg_query_slow` when fetch exceeds 500 ms; summary about every five minutes and at shutdown. |

`lock_timeout` limits each lock acquisition wait to 100 ms.
It does not limit how long an acquired lock is held.
The overall statement deadline remains 30 s (`statement_timeout`). A lock-wait
error (`55P03`) is logged, independent sources continue, and the read is tried
again on a later scheduled pass. These limits apply only to Kronika monitoring
sessions.
Source: [PostgreSQL documentation](https://www.postgresql.org/docs/current/runtime-config-client.html#GUC-LOCK-TIMEOUT).

Use `pg_query_summary` to track query volume, errors, timeouts and slow queries.
In `local` mode it also reports `peak_rss_kib`, the process’s peak physical memory
in KiB; this is unavailable in PostgreSQL-only mode. Connections are labelled
`user@host:port`. Source:
[query.rs](../../crates/kronika-source-pg/src/query.rs).

## Log collection

In `postgresql` mode, only log files explicitly listed in `KRONIKA_PG_LOGS`
are read. Paths returned by SQL are not used; remote files are not downloaded.

In `local` mode, the same `KRONIKA_PG_DSN` used for metrics discovers logs by
reading `pg_current_logfile()`, `data_directory` and `log_line_prefix`. This runs even when `KRONIKA_PG_LOGS`
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
| Discovery cadence | First collection cycle, then on the first collection cycle at least five minutes after the preceding scan; retries after errors. |
| Read limit | At most 256 MiB per file per collection. |
| PostgreSQL formats | Filename selects `.csv` → csvlog, `.json` → jsonlog, otherwise stderr. |
| Path-only identity | `system_identifier` is null; every row records its source file. |
| Path-only stderr | Database/user are unavailable; severity, SQLSTATE when present, message and continuations are parsed. Parsed timestamp is used when present, otherwise collection time. |
| Source error | Logged; other collection continues. |

## Linux collection

Linux collection runs only in `local` mode. On machines, VMs and containers,
one pass discovers all visible, accessible cgroup v2 directories at startup
and on `KRONIKA_OS_CGROUP_INTERVAL_S` (default 30 s). Empty and intermediate
groups are included without requiring visible processes. Each group keeps its
own path, identity and available CPU, memory, PIDs and per-device I/O values.
Missing fields stay unknown; parent and child counters are not added together.

Container resource context and PSI use the highest accessible
ancestor of the collector. That group can include other containers; its path
does not establish a pod or PostgreSQL identity. Machine PSI still uses the
host source. On v1-only systems cgroup metrics and process mappings are
unavailable; other enabled sources continue. See the
[Linux reference](../../docs/metrics-linux.md#container-cgroups).

Filesystem capacity is queried for `ext2`, `ext3`, `ext4`, `xfs`, `btrfs`,
`f2fs`, `zfs`, `tmpfs` and `overlay`. Other types retain null capacity fields.
Capacity reads for supported mounts share a one-second deadline. Mount rows
record their roots, space in bytes and available inodes. Device relationships connect partitions to
devices and layered devices to their underlying devices. In containers, these
relationships are limited to chains of mounted devices or devices accounted
for by cgroup I/O statistics.

## Run and signals

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika /usr/local/bin/kronika-collector
```

`SIGINT` and `SIGTERM` stop collection and retain `active.wal` without a final
ZMS close. Restarting recovers a valid nonempty journal immediately.
`SIGUSR2` collects immediately and saves a segment if that collection adds data.
`-h`, `--help` and `--version` exit before
configuration or storage access. Readiness and segment paths go to stdout;
structured logs go to stderr. In local mode, each `segment_write_finish` records `rss_kib`,
the peak physical memory occupied by the process in KiB.

In local mode, `cgroup_discovery_finish` reports group/device counts, `elapsed_us`,
process CPU `cpu_ticks` during acquisition and writing, and lifetime peak `rss_kib`.
CPU ticks use the recorded `clock_ticks_per_sec`; unavailable readings stay absent.

## Implementation

- [Configuration](src/config.rs) · [Schedule](src/scheduler.rs) · [Main loop](src/main.rs) · [Rotation implementation](src/rotation.rs)
- [database pool](../../crates/kronika-source-pg/src/pool.rs) · [extension discovery](../../crates/kronika-source-pg/src/extension.rs) · [settings](../../crates/kronika-source-pg/src/settings.rs) · [recorded layouts](../../docs/type-registry/postgresql-metrics.md)
- [source discovery](src/log_sources.rs) · [SQL facts and path resolution](src/log_sources/settings.rs) · [log collector](../../crates/kronika-source-log/src) · [PostgreSQL parser](../../crates/kronika-source-log/src/postgres.rs)
