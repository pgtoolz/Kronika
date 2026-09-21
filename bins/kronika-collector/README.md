# kronika-collector

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

The collector records Linux and PostgreSQL metrics and local PostgreSQL/PgBouncer
log events. It appends data to `active.wal` and periodically saves it as a
compressed `.zms` segment.

Set `--storage-dir` or `KRONIKA_STORAGE_DIR` to the recording directory.
Restart the process after changing its configuration. For systemd, see
[service configuration](../../docs/services.md).

[Storage failures and recovery](../../docs/storage-recovery.md) explains what
happens after an abrupt stop or when a recording cannot be read.

## Configuration

Options override environment variables; environment variables override defaults.
For example, `--pg-activity-interval-s 5` overrides `KRONIKA_PG_ACTIVITY_INTERVAL_S`.
Existing environment-only service and container configurations continue to work.
Use `kronika-collector --help` for the full option list.

<a id="storage"></a>
### Storage

`--segment-max-bytes`, `--journal-max-bytes` and fixed `--retention` accept
a number of bytes or a size suffix. `K`/`KB`, `M`/`MB`, `G`/`GB` and `T`/`TB`
use powers of 1000; `KiB`, `MiB`, `GiB` and `TiB` use powers of 1024.
Thus `1G` = `1GB` = 1,000,000,000 bytes; `1GiB` = 1,073,741,824 bytes.
Suffixes are case-insensitive: `1.5kb`, `1.5Mb`, `1.5M` and `1.5GiB` are valid.
Without a suffix, `4096` and `4096.75` are byte counts.
Fractional byte results are rounded to the nearest byte. The same syntax works
in command-line options and environment variables.

| Option | Environment variable | Default | Accepted value and meaning |
| --- | --- | --- | --- |
| `--storage-dir` | `KRONIKA_STORAGE_DIR` | Required | Directory in which collected data is saved. Use a directory, not a symbolic link. |
| `--segment-max-bytes` | `KRONIKA_SEGMENT_MAX_BYTES` | `64MiB` | Journal size at which a compressed segment becomes due. Must be greater than zero. |
| `--segment-max-age-s` | `KRONIKA_SEGMENT_MAX_AGE_S` | `900` | Period in seconds for scheduled segment closing. Nonnegative whole number. `0` makes a segment eligible immediately. |
| `--journal-max-bytes` | `KRONIKA_JOURNAL_MAX_BYTES` | `1GiB` | Maximum journal size: 36 bytes to `1GiB`. Reaching it saves the segment early. |
| `--retention` | `KRONIKA_RETENTION` | `2GiB` | Fixed storage target, or `auto` (= `auto:80`), or `auto:P`, where `P` is a whole percentage from 1 to 99. |

Collectors close ZMS files on a schedule with a random time offset. The offset
survives restarts.

The first segment after startup or an early close may be shorter. Ongoing
collection can delay closing. Size limits and `SIGUSR2` can close it sooner.
`--interval-s 0` disables collection and segment closing by timer.

A fixed budget counts the active journal, compressed recordings, their `.idx`
index files and collector temporary files. It must be at least twice
`--segment-max-bytes`. For example, `--retention 10GiB` sets a 10 GiB target.
With `auto:P`, old recordings are removed when more than `P` percent
of the entire backing filesystem is used. `auto` means 80 percent. This
includes space used by other programs.

The collector checks after saving segments and on a one-minute timer. A
collection in progress can delay the check. It removes leftover temporary
files first, then indexes without a recording, then the oldest finished
recordings with their indexes. The active journal, newest finished segment
and unrelated files are retained. If these still exceed the target,
collection continues and logs `rotation_degraded`. Fixed mode also recounts
files hourly to include new indexes created by web.

### Collection mode

| Option | Environment variable | Default | Values |
| --- | --- | --- | --- |
| `--mode` | `KRONIKA_COLLECTOR_MODE` | `local` | `local` or `postgresql`. |

`--mode local` is the default: Linux metrics and optional
PostgreSQL in the same VM or pod. Without a DSN, PostgreSQL metrics are not
collected. Explicitly configured local logs can still be read.
Process links require PostgreSQL to run on the same machine and in
the same PID namespace as the collector. In containers, the selected cgroup can
include other containers. Its metrics do not establish PostgreSQL process
identity or CPU capacity.

`--mode postgresql` records only PostgreSQL data from a local or
remote server. It does not collect Linux metrics, processes or cgroups.
`--pg-dsn` or `KRONIKA_PG_DSN` is required. The process needs access to
PostgreSQL and write access to storage. Add local log files by repeating
`--pg-log`, or set `KRONIKA_PG_LOGS`. PgBouncer log settings are not accepted in this mode.
Linux collection intervals do not apply.

### Collection intervals

Intervals are nonnegative whole seconds. Each source has its own schedule. Except for
statements/plans, intervals may be zero to read on every timer wakeup.
Statements/plans require at least 300 seconds. A cgroup is a Linux group of
processes with shared resource limits. PSI measures time spent waiting for
CPU, memory or I/O resources.

| Option | Environment variable | Default, s | Data |
| --- | --- | ---: | --- |
| `--interval-s` | `KRONIKA_INTERVAL_S` | 5 | Maximum timer sleep. `0` disables timed collection. A shorter positive source interval can wake the timer earlier. |
| `--os-core-interval-s` | `KRONIKA_OS_CORE_INTERVAL_S` | 10 | CPU, memory, disks, network, PSI. |
| `--os-mount-topo-interval-s` | `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 | Mounts, filesystem capacity and device topology. |
| `--os-process-interval-s` | `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 | Process counters. |
| `--os-process-status-interval-s` | `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 | Process status details. |
| `--os-cgroup-interval-s` | `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 | Discover all accessible cgroup v2 groups in containers and read their resource counters and limits. |
| `--os-cgroup-mapping-interval-s` | `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 | Process-to-cgroup v2 mappings in containers. |
| `--log-interval-s` | `KRONIKA_LOG_INTERVAL_S` | 10 | Configured PostgreSQL/PgBouncer logs. |
| `--pg-instance-interval-s` | `KRONIKA_PG_INTERVAL_S` | 30 | Server counters and settings. |
| `--pg-activity-interval-s` | `KRONIKA_PG_ACTIVITY_INTERVAL_S` | 10 | Activity, lock waits and VACUUM progress; temporarily shortened when lock waits are detected. |
| `--pg-activity-blocked-interval-s` | `KRONIKA_PG_ACTIVITY_BLOCKED_INTERVAL_S` | 5 | Activity interval while lock waits exist, capped by the ordinary activity interval. `0` reads on every timer wakeup. |
| `--pg-statements-interval-s` | `KRONIKA_PG_STATEMENTS_INTERVAL_S` | 300 | `pg_stat_statements`, `pg_store_plans` and their info views. Minimum 300 seconds. |
| `--pg-relations-interval-s` | `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 | Table and index statistics in each discovered database. |

A successful, nonempty lock-wait snapshot changes the activity interval to
the smaller of `--pg-activity-interval-s` and `--pg-activity-blocked-interval-s`.
A successful empty snapshot restores the ordinary activity interval;
a failed read leaves it unchanged. With the defaults, activity, lock waits and
VACUUM progress are read every 10 seconds, or every 5 seconds while the collector
observes lock waits. A zero interval reads on every regular timer wakeup without
adding timer wakeups.

Statements and plans wait for the configured `--pg-statements-interval-s`
after the preceding PostgreSQL pass containing those sources finishes.
`SIGUSR2` does not bypass this limit. `--interval-s 0` still makes
collection signal-driven only, including activity during lock waits.
Collection is sequential: a slow SQL query or another running source can delay
the next activity snapshot beyond its configured interval.

### Connections and logs

A DSN is a database connection string, written as `key=value` pairs or a URL.
`--pg-dsn` accepts one connection string. In the default `local` mode, this
connection also discovers the current PostgreSQL log file through
`pg_current_logfile()` and `data_directory`. If the file is readable on the
collector host, `--pg-log` is not needed.

Repeat `--pg-log`, `--pgbouncer-dsn` and `--pgbouncer-log` for multiple entries.
This example reads extra local files alongside the automatically discovered log.
Quote DSNs and wildcard paths so the shell passes each value unchanged:

```sh
sudo /usr/local/bin/kronika-collector \
  --storage-dir /var/lib/kronika \
  --pg-dsn 'host=127.0.0.1 user=kronika_monitor dbname=postgres' \
  --pg-log '/var/log/postgresql/*.csv' \
  --pg-log '/srv/postgresql/*.json'
```

In `postgresql` mode, only explicit log paths are read. Those files must also
be readable on the collector host.

A list supplied on the command line replaces that entire environment list.
Command-line values are not split at semicolons. Environment lists keep the
`;` separator; an empty list adds no entries, and an empty entry is an error.

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--pg-dsn` | `KRONIKA_PG_DSN` | Unset | One PostgreSQL connection string for server metrics and accessible databases. In `local` mode, the same DSN discovers the current local log. Required in `postgresql` mode. |
| `--pg-ssl-root-cert` | `KRONIKA_PG_SSL_ROOT_CERT` | Unset | PEM CA bundle replacing the included public CA roots. When unset, the included public roots are used. TLS validates the server hostname in both cases. |
| `--postgres-effective-cpus` | `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Unset | Available CPUs of the monitored PostgreSQL server: integer `1..4294967295`. Requires a PostgreSQL DSN. Determined automatically in `local` mode on a machine shared with PostgreSQL. Without this count, SQL metrics continue and PostgreSQL Health is unknown. |
| `--pg-log` | `KRONIKA_PG_LOGS` | Unset | Optional readable local paths. Final filename supports `*` and `?`. In `local` mode, paths add to `pg_current_logfile()` discovery. In `postgresql` mode, only explicit paths are opened. |
| `--pg-log-max-lag-s` | `KRONIKA_PG_LOG_MAX_LAG_S` | `900` | Skip PostgreSQL log events older than this many seconds at read time. Positive whole number. |
| `--pgbouncer-dsn` | `KRONIKA_PGBOUNCER_DSNS` | Unset | Connections to the administrative console (`dbname=pgbouncer`) to read `SHOW CONFIG`/`logfile`. The account must belong to `stats_users`. |
| `--pgbouncer-log` | `KRONIKA_PGBOUNCER_LOGS` | Unset | Local PgBouncer log paths. Filenames can use `*` and `?` wildcards. |

The collector retains PgBouncer warnings, errors and unknown LOG messages with connection context. Missing or invalid timestamps use the read time. Routine connection chatter, periodic statistics and DEBUG/NOISE are skipped. See [recorded fields](../../docs/type-registry/pgbouncer.md).

Without `--pg-dsn`, the legacy `KRONIKA_PG_DSNS` still uses only its first
connection string and cannot be set together with `KRONIKA_PG_DSN`.
Migrate to `--pg-dsn` or `KRONIKA_PG_DSN`; support for `KRONIKA_PG_DSNS`
will be removed.

### Other settings

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--log-level` | `KRONIKA_LOG_LEVEL` | `info` | Logging detail: case-insensitive `error`, `warn`/`warning`, `info`, `debug`, `trace`. Messages go to standard error (stderr). |
| `--proc-root` | `KRONIKA_PROC_ROOT` | `/proc` | Directory containing process information from procfs. Setting it limits container detection to that directory’s cgroup file. |
| `--sys-root` | `KRONIKA_SYS_ROOT` | `/sys` | Directory containing kernel device information from sysfs. |
| — | `KRONIKA_STATVFS_FIXTURE` | Unset | Filesystem values for tests: `path=TOTAL:FREE:INODES:AVAILABLE_INODES;...` substitutes `statvfs` values. |

## PostgreSQL collection

### PostgreSQL CPU capacity

In `local` mode on a machine/VM shared with PostgreSQL, leave
`--postgres-effective-cpus` and `KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset.
Activity uses the CPU count from the last complete machine snapshot at or
before the sample. The DSN address alone does not prove that PostgreSQL
runs on this machine.

For remote PostgreSQL or container deployments, an optional positive whole
number supplies the PostgreSQL CPU capacity. A broader cgroup aggregate does
not give the capacity of its PostgreSQL child. Without capacity, SQL collection
continues. PostgreSQL Health and capacity-dependent marks are unknown. Web
reads the recorded value and has no separate CPU setting.

<a id="remote-postgresql"></a>
### PostgreSQL only — local or remote

```sh
sudo install -d -m 0700 -o "$(id -u)" /var/lib/kronika

/usr/local/bin/kronika-collector \
  --mode postgresql \
  --storage-dir /var/lib/kronika \
  --pg-dsn 'host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres'
```

Add `--postgres-effective-cpus 4` when that server has 4 available CPUs.

The DSN parameter `sslmode` controls TLS: `prefer` (default) uses TLS when
the server supports it. `require` accepts only TLS. `disable` turns TLS off.
TLS connections validate the server certificate and hostname. For a private CA,
set `--pg-ssl-root-cert /path/to/ca.pem`. The value `verify-full` is not supported.

Open the recording with [kronika-web](../kronika-web/README.md#run).
Overall equals PostgreSQL Health, or is unknown when PostgreSQL Health cannot be calculated.

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
Start collection from `pg-a.example.net` in one terminal:

```sh
sudo install -d -m 0700 -o "$(id -u)" /var/lib/kronika-pg-a

/usr/local/bin/kronika-collector \
  --mode postgresql \
  --storage-dir /var/lib/kronika-pg-a \
  --pg-dsn 'host=pg-a.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres'
```

Start collection from `pg-b.example.net` in another terminal:

```sh
sudo install -d -m 0700 -o "$(id -u)" /var/lib/kronika-pg-b

/usr/local/bin/kronika-collector \
  --mode postgresql \
  --storage-dir /var/lib/kronika-pg-b \
  --pg-dsn 'host=pg-b.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres'
```

To view both recordings, start `kronika-web` separately for `/var/lib/kronika-pg-a`
and `/var/lib/kronika-pg-b`, using different listen addresses.

<a id="postgresql-role"></a>
### PostgreSQL role

The PostgreSQL account needs these privileges. The [setup examples](../../INSTALL.md#5-postgresql)
show how to grant them to a monitoring role:

| Scope | Required privilege |
| --- | --- |
| Role | Inherited `pg_monitor` membership. Collector does not issue `SET ROLE`. |
| Each collected database | `CONNECT` and normal catalog read/function access. |
| Selected extension schema | `USAGE`. |
| `pg_stat_statements` reader | `EXECUTE` on `pg_stat_statements(boolean)`. |
| `pg_store_plans` reader | `EXECUTE` on installed `pg_store_plans()` or `pg_store_plans(boolean)`. Vadv interface also needs `pg_store_plans_get_plan(oid, oid, bigint, bigint)` and `pg_store_plans_textplan(text)`. |
| Installed `*_info` interface | `SELECT` on the info view and `EXECUTE` on its zero-argument function. |
| Initial database used for PostgreSQL log discovery | `EXECUTE` on `pg_catalog.pg_current_logfile()` and `pg_catalog.pg_control_system()`. |

Schema, view and function privileges are database-local. Extension readers are
called directly. Default PostgreSQL/extension grants supply some of these
permissions. Explicitly revoked privileges must be granted to the monitoring
role. The explicit `pg_current_logfile()` grant is needed on PostgreSQL 10–16.

### Discovery and session lifetime

| Item | Behavior |
| --- | --- |
| Database sessions | One reused connection per connectable database, replaced after at most one hour while healthy. The database and extension list is normally refreshed every five minutes, on a PostgreSQL or tables/indexes collection pass. Forced collection and failed discovery can cause earlier retries. |
| Extension inventory | Each database is checked during discovery. One usable installation of each extension is selected. |
| `pg_stat_statements` | Supports extension `1.5+` in the `1.x` series. PostgreSQL 14+ requires `1.9+`. The newest compatible set of fields is preferred. |
| `pg_store_plans` | OSSC and Datasentinel return different fields through a function with no arguments. The vadv boolean interface requires its four-key plan lookup function and plan-to-text converter. |
| Settings | Read with PostgreSQL metrics. A full snapshot is recorded after the first successful read, on change and in every segment. |

### Query execution

Connect directly to PostgreSQL or through PgBouncer in session pooling mode.
Transaction/statement pooling does not preserve the session settings needed
by the collector.

| Item | Value or behavior |
| --- | --- |
| Concurrency | One query at a time per connection. |
| Session limits | `statement_timeout=30s`, `lock_timeout=100ms` for all monitoring queries, including log discovery. |
| Client fetch deadline | After 35 seconds, attempt to cancel the query and close the connection. |
| Collector identity | One unique `application_name` per collector process. Activity/Locks exclude that exact name. |
| Text bounds | Statement and plan text limited to 65,536 characters in SQL. |
| Interrupted read | Data already recorded is retained. The rest of that read is skipped. Other sources continue. |
| SQLSTATE `57014` | Counted as a query timeout. |
| Query logs | Debug `pg_query_finish`. Warning `pg_query_slow` when fetch exceeds 500 ms. Summary about every five minutes and at shutdown. |

`lock_timeout` limits each lock acquisition wait to 100 ms.
It does not limit how long an acquired lock is held.
The overall statement deadline remains 30 s (`statement_timeout`). A lock-wait
error (`55P03`) is logged, independent sources continue, and the read is tried
again on a later scheduled pass. These limits apply only to Kronika monitoring
sessions.
Source: [PostgreSQL documentation](https://www.postgresql.org/docs/current/runtime-config-client.html#GUC-LOCK-TIMEOUT).

Use `pg_query_summary` to track query volume, errors, timeouts and slow queries.
In `local` mode it also reports `peak_rss_kib`, the process’s peak physical memory
in KiB. This is unavailable in PostgreSQL-only mode. Connections are labelled
`user@host:port`. Source:
[query.rs](../../crates/kronika-source-pg/src/query.rs).

## Log collection

In `postgresql` mode, only log files explicitly listed with `--pg-log` or
`KRONIKA_PG_LOGS` are read. The files must be readable on the collector host.

In `local` mode, the same PostgreSQL DSN used for metrics discovers logs by
reading `pg_current_logfile()` and `data_directory`. This runs even without
explicit log paths. Relative paths are resolved against that server's `data_directory`. A null result adds no path.

`--pg-log` or `KRONIKA_PG_LOGS` adds local paths or filename patterns.
In both modes, the configured server supplies `log_line_prefix`, `log_timezone`
and, when available, `system_identifier` for these files.
Event timestamps use `log_timezone`, independently of the collector's timezone.
Discovery requires the [function privileges](#postgresql-role) listed above.

| Property | Behavior |
| --- | --- |
| Discovery cadence | First collection cycle, then at least five minutes after the preceding scan. Failed scans are retried. |
| Read limit | At most 256 MiB per file per collection. |
| PostgreSQL formats | Filename selects `.csv` → csvlog, `.json` → jsonlog, otherwise stderr. |
| Time without a DSN | UTC/GMT/Z, numeric offsets and IANA names such as `Europe/Moscow` are accepted. Other abbreviations require the server's `log_timezone`. |
| Missing or invalid timestamp | Recognized messages use read time. Processing continues with the following records. |
| Source error | Logged. Collection from other sources continues. |

## Linux collection

Linux collection runs only in `local` mode. Cgroups are collected only in containers
with an exposed cgroup v2 mount. One pass discovers all visible, accessible directories at startup
and then every `--os-cgroup-interval-s` seconds (default 30 s). Empty and intermediate
groups are included without requiring visible processes. Each group keeps its
own path, identity and available CPU, memory, PIDs and per-device I/O values.
Missing fields stay unknown. Parent and child counters are not added together.

Container resource context and PSI use the highest accessible
ancestor of the collector. That group can include other containers. Its path
does not establish a pod or PostgreSQL identity. Machine PSI still uses the
host source. Machines and VMs retain their other OS and process metrics without
cgroup collection. Containers without v2 skip cgroup metrics, mappings and PSI.
See the
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
sudo /usr/local/bin/kronika-collector --storage-dir /var/lib/kronika
```

`SIGINT` and `SIGTERM` stop collection and retain `active.wal` without a final
ZMS close. Restarting recovers a valid nonempty journal immediately.
`SIGUSR2` requests immediate collection and saves a segment if that collection
adds data. Statements/plans keep their minimum interval even on a forced cycle.
`-h`, `--help` and `--version` exit without configuration validation or
storage access. Readiness and segment paths go to stdout.
Structured logs go to stderr. In local mode, each `segment_write_finish` records `rss_kib`,
the peak physical memory occupied by the process in KiB.

During container cgroup collection, `cgroup_discovery_finish` reports group/device counts, `elapsed_us`,
process CPU `cpu_ticks` during acquisition and writing, and lifetime peak `rss_kib`.
CPU ticks use the recorded `clock_ticks_per_sec`. Unavailable readings stay absent.

## Implementation

- [Configuration](src/config.rs) · [Schedule](src/scheduler.rs) · [Main loop](src/main.rs) · [Rotation implementation](src/rotation.rs)
- [database pool](../../crates/kronika-source-pg/src/pool.rs) · [extension discovery](../../crates/kronika-source-pg/src/extension.rs) · [settings](../../crates/kronika-source-pg/src/settings.rs) · [recorded layouts](../../docs/type-registry/postgresql-metrics.md)
- [source discovery](src/log_sources.rs) · [SQL facts and path resolution](../../crates/kronika-source-pg/src/log_discovery.rs) · [log collector](../../crates/kronika-source-log/src) · [PostgreSQL parser](../../crates/kronika-source-log/src/postgres.rs)
