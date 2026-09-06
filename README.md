# Kronika

[Русская версия](README.ru.md)

Kronika records Linux metrics, PostgreSQL statistics, query plans and events
from PostgreSQL/PgBouncer logs. The collector runs on the monitored machine and
saves data to its disk. The web interface shows what happened during a selected
hour: resource use, individual processes and queries, locks and changes over time.

![Process CPU activity and the process snapshot for a recorded hour](docs/images/processes.png)

[Open the interactive preview](https://pgtoolz.github.io/Kronika/).

A recorded hour, 5 September 2026, 19:00–20:00 UTC:
[Processes](https://pgtoolz.github.io/Kronika/reports/kronika-demo-hour-5bbe2dd.html?at=1788634833931637&view=processes) · [Statements](https://pgtoolz.github.io/Kronika/reports/kronika-demo-hour-5bbe2dd.html?at=1788634833931637&view=pg.statements) · [Plans](https://pgtoolz.github.io/Kronika/reports/kronika-demo-hour-5bbe2dd.html?at=1788634833931637&view=pg.plans) · [Host](https://pgtoolz.github.io/Kronika/reports/kronika-demo-hour-5bbe2dd.html?at=1788634833931637&view=host).

## Install and run

Install a [prebuilt Linux archive](INSTALL.md), or
[build from source](docs/build.md). The archive contains `kronika-collector`,
`kronika-web`, `kronika-dump`, and `kronika-report`.

After installation, start collection on the monitored host:

```sh
sudo install -d -m 0700 /var/lib/kronika
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

In a second terminal, start the web server over that data directory:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

Open <http://127.0.0.1:8080/> and sign in. The web server reads new data while
collection runs. `Ctrl+C` stops either process and retains the recording.
[Systemd units](docs/services.md) run both programs as services.

### Local and remote PostgreSQL

After [creating a monitoring role](INSTALL.md#5-postgresql), stop the collector
and set its connection string (DSN). When local PostgreSQL shares the collector’s CPU limits,
leave `KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset:
the available CPU count is calculated from recorded data. A cgroup is a Linux
group of processes with shared resource limits; its CPU-time quota and allowed
set of CPUs are taken into account.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

For remote PostgreSQL or PostgreSQL in a different cgroup, set the CPU capacity
available to that PostgreSQL server. Example for PostgreSQL with 4 CPUs:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  KRONIKA_POSTGRES_EFFECTIVE_CPUS=4 \
  /usr/local/bin/kronika-collector
```

The connection address alone does not show whether the programs share resource
limits. When the collector is configured for Linux and PostgreSQL, set
`KRONIKA_WEB_SOURCES=3` when starting the web server.

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

The names below match sections in the interface.

| Domain | What you can inspect | Reference |
| --- | --- | --- |
| Processes | Command, state and process number (PID), CPU use, memory and disk reads/writes; process tree and hourly activity. | [Linux metrics](docs/metrics-linux.md) |
| Host | CPU, memory, time waiting for resources (PSI), network and disks, free space and device relationships; container resource limits and use. | [Linux metrics](docs/metrics-linux.md) |
| PostgreSQL sessions | Overview, Activity, Locks, Vacuum; session states and waits, blocking chains, query/transaction durations and table cleanup progress. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| PostgreSQL SQL | Statements and Plans; calls, execution/planning time, page and temporary-file reads, write-ahead log (WAL) output, SQL and plan text. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| PostgreSQL objects | Databases, Tables, Indexes and settings; size, reads and changes, maintenance and transaction ages; grouping by database, schema and tablespace. | [PostgreSQL metrics](docs/metrics-postgresql.md) |
| Events | Grouped PostgreSQL/PgBouncer log events, occurrences, durations and recorded context; metric marks. | [Views and controls](docs/features.md) |
| Time and charts | Choose an hour and a time within it; view changes, activity maps, totals and the distribution of measurements. | [Time and calculations](docs/metrics-time.md) |

[Views and controls](docs/features.md) explains how to select measurements, group,
search and sort rows, inspect details in Inspector, view charts and export. The
[operator guide](docs/operator-guide.md) contains four worked examples from
the preview recording.

![Recorded statement, SQL text and interval activity](docs/images/statements.png)

![Recorded execution plan and associated SQL](docs/images/plans.png)

## Collection and access

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/architecture-dark.svg">
  <img alt="Linux and PostgreSQL feed the collector; web reads its recording for the browser and MCP clients" src="docs/images/architecture.svg">
</picture>

The default collection intervals are 5 seconds for processes, 10 seconds for
core Linux metrics, 30 seconds for PostgreSQL metrics, and 300 seconds for
tables and indexes.
[Collector configuration](bins/kronika-collector/README.md) defines source
selection, intervals, access permissions and removal of old recordings.

The web server serves the browser, HTTP API and MCP at one address and port.
MCP is a protocol through which an AI client can read stored data. The **AI**
panel provides connection settings. [MCP tools](docs/features.md#mcp) return
values at a chosen time, objects ranked by a measurement, field descriptions,
events and complete row details.

## Portable HTML export

**Export** saves a selected interval of your recording as one interactive HTML
file. It embeds the interface, data and a Rust/WebAssembly program to process
queries on the browser’s main thread. Opening the file requires no server or
network connection.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/report-export-dark.svg">
  <img alt="Export an interval from web or a saved recording into one interactive offline HTML file" src="docs/images/report-export.svg">
</picture>

[kronika-dump](bins/kronika-dump/README.md) inspects storage and extracts an
interval into a standalone ZMS recording; [kronika-report](bins/kronika-report/README.md)
converts that recording into HTML. Offline reports provide tables, search,
charts and activity maps.

## Documentation

- Setup: [Install](INSTALL.md) · [Archives and CI](docs/releases.md) · [Services](docs/services.md) · [Storage failures](docs/storage-recovery.md) · [Source build](docs/build.md)
- Reference: [Controls](docs/features.md) · [Time](docs/metrics-time.md) · [Linux](docs/metrics-linux.md) · [PostgreSQL](docs/metrics-postgresql.md) · [MCP](docs/mcp-clients.md)
- Programs: [Collector](bins/kronika-collector/README.md) · [Web](bins/kronika-web/README.md) · [Dump](bins/kronika-dump/README.md) · [Report](bins/kronika-report/README.md)
- Recorded fields: [Linux](docs/type-registry/os.md) · [PostgreSQL metrics](docs/type-registry/postgresql-metrics.md) · [PostgreSQL events](docs/type-registry/postgresql.md) · [PgBouncer events](docs/type-registry/pgbouncer.md)
- Development: [Segment format](crates/kronika-format/README.md) · [Development demo](bins/kronika-demo/README.md)

[MIT License](LICENSE).
