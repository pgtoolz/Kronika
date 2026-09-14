# Kronika

[Русская версия](README.ru.md)

Kronika records Linux metrics, PostgreSQL statistics, query plans and events
from PostgreSQL/PgBouncer logs. The collector runs on the monitored Linux machine
or records only PostgreSQL data from a local or remote server. The web interface
shows resource use, processes, queries and locks during a selected hour,
and how they changed over time.

![Process CPU activity and the process snapshot for a recorded hour](docs/images/processes.png)

[Open the interactive preview](https://pgtoolz.github.io/Kronika/).

A recorded hour, 5 September 2026, 19:00–20:00 UTC:
[Processes](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=processes) · [Statements](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=pg.statements) · [Plans](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=pg.plans) · [Host](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.2.html?at=1788634833931637&view=host).

## Install and run

[Install Kronika 1.1.2](INSTALL.md) from a [Linux release archive](docs/releases.md#download),
or [build from source](docs/build.md).

Choose `local` to record Linux and, optionally, PostgreSQL in the same VM or pod.
Choose `postgresql` for a remote server or when you only need database metrics.

### Linux and optional PostgreSQL

Start collecting Linux metrics:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

<a id="linux-and-postgresql"></a>
For PostgreSQL on the collector machine, supply its connection string in
`KRONIKA_PG_DSN` when starting collector. Use a PostgreSQL account with the
[monitoring privileges](INSTALL.md#5-postgresql).

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

Leave `KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset for this local machine: CPU capacity
comes from its recorded CPU snapshots.

### PostgreSQL only — local or remote

```sh
sudo install -d -m 0700 -o "$(id -u)" /var/lib/kronika

KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

If the PostgreSQL CPU capacity is known, add `KRONIKA_POSTGRES_EFFECTIVE_CPUS=4`,
replacing `4` with its CPU count.
Without it, SQL metrics remain available. PostgreSQL Health is unknown.
See [collector configuration](bins/kronika-collector/README.md#remote-postgresql).

To collect from several PostgreSQL servers, run a `kronika-collector` process
for each server with its DSN and a separate storage directory. See the
[two-server example](bins/kronika-collector/README.md#several-postgresql-servers).

### Open the web interface

Start `kronika-web` in a second terminal with the collector’s data directory.

#### For `local` mode

Use `KRONIKA_WEB_SOURCES=1` for Linux only, as below. Change `1` to `3`
when also collecting PostgreSQL:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
  KRONIKA_WEB_SOURCES=1 \
  /usr/local/bin/kronika-web
```

#### For `postgresql` mode

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=0.0.0.0:8080 \
  KRONIKA_WEB_SOURCES=2 /usr/local/bin/kronika-web
```

Open `http://<server-ip>:8080`.

To require sign-in, add `KRONIKA_WEB_USER=kronika` and
`KRONIKA_WEB_PASSWORD='replace-with-a-random-password'` to the launch command.

[Systemd setup](docs/services.md) covers running both programs as services and
changing an existing service's configuration.

### Storage

For a PostgreSQL workload with roughly 500 tables and 3,000 indexes, estimate
**about 200 MB of compressed recordings per day**. Volume depends on collection
intervals and the number of recorded objects and distinct queries.

`KRONIKA_RETENTION=2147483648` sets the default **2 GiB** storage budget,
including journals and indexes. When the target is exceeded, the collector
automatically removes the oldest finished recordings and their indexes.
For **10 GiB**, set `KRONIKA_RETENTION=10737418240` (raw bytes).

`auto` and `auto:P` instead set a used-space percentage target for the whole
backing filesystem. See [storage configuration](bins/kronika-collector/README.md#storage)
for the rotation rules and automatic mode.

## Recorded data and views

| Domain | What you can inspect | Reference |
| --- | --- | --- |
| Processes | Command, state and process number (PID), CPU use, memory and disk reads/writes, process tree and hourly activity. | [Linux metrics](docs/metrics-linux.md) |
| Host | CPU, memory, time waiting for resources (PSI), network and disks, free space and device relationships, cgroup resource limits and use. | [Linux metrics](docs/metrics-linux.md) |
| PostgreSQL sessions | Overview, Activity, Locks, Vacuum: session states and waits, blocking chains, query/transaction durations and table cleanup progress. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| Queries and plans | Statements and Plans: calls, execution/planning time, page and temporary-file reads, write-ahead log (WAL) output, SQL and plan text. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| PostgreSQL objects | Databases, Tables and Indexes: settings, size, reads and changes, maintenance and transaction ages. Grouping by database, schema and tablespace. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| Events | Grouped PostgreSQL/PgBouncer log events, occurrences, durations and recorded context, metric marks. | [Views and controls](docs/features.md) |
| Time and charts | Choose an hour and a time within it. View changes, activity maps, totals and the distribution of measurements. | [Time and calculations](docs/metrics-time.md) |

[Views and controls](docs/features.md) explains how to select measurements, group,
search and sort rows, inspect details in Inspector, view charts and export. The
[operator guide](docs/operator-guide.md) contains four worked examples from
the preview recording.

![Recorded statement, SQL text and interval activity](docs/images/statements.png)

![Recorded execution plan and associated SQL](docs/images/plans.png)

## Collection and access

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.svg">
  <img alt="Linux and PostgreSQL feed the collector, and web reads its recording for the browser and MCP clients" src="docs/images/architecture.svg">
</picture>

The default collection intervals are 5 seconds for processes, 10 seconds for
core Linux metrics, 30 seconds for PostgreSQL metrics, and 300 seconds for
tables and indexes.
[Collector configuration](bins/kronika-collector/README.md) defines source
selection, intervals, access permissions and removal of old recordings.

The web server serves the browser, HTTP API and MCP at one address and port.
The **AI** panel provides connection settings. [MCP tools](docs/features.md#mcp) return
values at a chosen time, objects ranked by a measurement, field descriptions,
events and row details.

## Portable HTML export

**Export** saves a selected interval of your recording as one interactive HTML
file. It includes the interface and data, so tables, search and charts work without
a server or network connection.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/report-export-dark.svg">
  <img alt="Export an interval from web or a saved recording into one interactive offline HTML file" src="docs/images/report-export.svg">
</picture>

[kronika-dump](bins/kronika-dump/README.md) inspects storage and extracts an
interval into a standalone ZMS recording, and [kronika-report](bins/kronika-report/README.md)
converts that recording into HTML.

## Documentation

- Setup: [Install](INSTALL.md) · [Archives and CI](docs/releases.md) · [Services](docs/services.md) · [Storage failures](docs/storage-recovery.md) · [Source build](docs/build.md)
- Reference: [Controls](docs/features.md) · [Time](docs/metrics-time.md) · [Linux](docs/metrics-linux.md) · [PostgreSQL](docs/metrics-postgresql.md) · [MCP](docs/mcp-clients.md)
- Programs: [Collector](bins/kronika-collector/README.md) · [Web](bins/kronika-web/README.md) · [Dump](bins/kronika-dump/README.md) · [Report](bins/kronika-report/README.md)
- Recorded fields: [Linux](docs/type-registry/os.md) · [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md) · [PostgreSQL events](docs/type-registry/postgresql.md) · [PgBouncer events](docs/type-registry/pgbouncer.md)
- Development: [Segment format](crates/kronika-format/README.md) · [Development demo](bins/kronika-demo/README.md)

[MIT License](LICENSE).
