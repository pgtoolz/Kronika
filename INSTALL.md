# Install on Linux

[Русская версия](INSTALL.ru.md) · [README](README.md)

Use `kronika-collector` to record your machine and `kronika-web` to view its
history in a browser. The archive also includes `kronika-dump` to inspect or
extract part of a recording and `kronika-report` to create an HTML report.

Steps 1–2 install the published archive. To compile the programs yourself, use
the [source-build guide](docs/build.md), then return to [collector startup](#3-start-collector).
PostgreSQL collection needs a connection with monitoring permissions.

## 1. Download and extract

[Kronika v1.0.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.0)
provides archives and `.tar.gz.sha256` checksum files. Check your machine's
architecture with `uname -m`:

| `uname -m` | Archive target |
| --- | --- |
| `x86_64` | `x86_64-unknown-linux-musl` |
| `aarch64` | `aarch64-unknown-linux-musl` |

Download, verify and extract the archive. For ARM64, change the first line to
`target=aarch64-unknown-linux-musl`:

```sh
target=x86_64-unknown-linux-musl
archive="kronika-1.0.0-$target.tar.gz"
release_url=https://github.com/pgtoolz/Kronika/releases/download/v1.0.0
curl -fLO "$release_url/$archive"
curl -fLO "$release_url/$archive.sha256"
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
```

## 2. Install

```sh
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
```

## 3. Start collector

On the machine to monitor, create a private recording directory owned by root:

```sh
sudo install -d -m 0700 /var/lib/kronika
```

Choose one startup below: Linux only, or Linux with PostgreSQL. Configuration
is read when the program starts.

<a id="3-record-linux"></a>
### Linux only

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

Use a real storage directory, not a symlink. Root can read protected process
I/O counters and local logs. Processes are sampled every 5 seconds and core
Linux metrics every 10 seconds. When the accumulated recording reaches
900 seconds of age, it is saved as a finished compressed file called a segment.
A size limit can finish the segment earlier. Web can read `active.wal` before
it becomes a finished segment. `Ctrl+C` stops collection and retains the
journal; the same command reopens the recording.

`KRONIKA_RETENTION` defaults to `2147483648` bytes (2 GiB). For a fixed 10 GiB
target, add `KRONIKA_RETENTION=10737418240`.
[Storage](bins/kronika-collector/README.md#storage) defines the counted files
and deletion order.

<a id="5-postgresql"></a>
### Linux and PostgreSQL

Use a role with monitoring permissions. To create one, run these commands in
`psql` as a PostgreSQL administrator:

```sql
CREATE ROLE kronika_monitor LOGIN;
\password kronika_monitor
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
```

The role needs inherited `pg_monitor` membership, `CONNECT` to each collected
database and the database-local extension permissions listed in
[PostgreSQL role](bins/kronika-collector/README.md#postgresql-role).

PostgreSQL on the collector machine:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

| Setting or connection | Contract |
| --- | --- |
| `KRONIKA_PG_DSNS` | The first connection string (DSN) enables metrics from that server's connectable databases. Additional semicolon-separated DSNs discover logs. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Optional integer `1..4294967295`: explicit CPU capacity of the monitored PostgreSQL server. Unset uses the collector machine or container's recorded CPU capacity. |
| Extension discovery | Supported `pg_stat_statements` and `pg_store_plans` interfaces are detected in connectable databases. Activity, Locks and relation statistics use PostgreSQL's built-in views. |
| Transport | Native client uses `NoTls`; direct PostgreSQL and PgBouncer session pooling are supported. Metric sessions retain `SET` state. |
| Log paths | Each `KRONIKA_PG_DSNS` entry discovers its current log through `pg_current_logfile()` even with `KRONIKA_PG_LOGS` unset. The server path must be readable on the collector host. `KRONIKA_PG_LOGS` adds local paths/globs; PgBouncer uses `KRONIKA_PGBOUNCER_DSNS` or `KRONIKA_PGBOUNCER_LOGS`. |

If PostgreSQL is on another machine, only its SQL data is read remotely;
Linux data comes from the collector machine. See [remote PostgreSQL](bins/kronika-collector/README.md#remote-postgresql)
for configuration and the meaning of process links and Health.

[Service configuration](docs/services.md) stores DSNs and web credentials in
root-readable environment files. [Collector reference](bins/kronika-collector/README.md)
defines intervals, supported extension layouts and log formats.

## 4. Start web

In a second terminal, set a password and start web with the same recording
directory. Use `KRONIKA_WEB_SOURCES=1` for Linux only, as shown below, or `3`
for Linux and PostgreSQL:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

Open <http://127.0.0.1:8080/> and sign in. Web requires write access to the
recording directory to create search indexes (`.idx`) and a lock file that
prevents concurrent index rebuilds. This example runs both programs as root
with private storage.

`KRONIKA_WEB_SOURCES` reports which sources are configured; it does not enable
collection or hide recorded data. User and password remain required with
`KRONIKA_WEB_AUTH=disabled`.

For access from another machine, run on that machine:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@monitored-host
```

Open <http://127.0.0.1:8080/> there. MCP uses the same listener and credentials
at `/mcp`; [client setup](docs/mcp-clients.md) is also available in the **AI**
panel. [Systemd](docs/services.md) defines persistent services.

## Reference

[Controls](docs/features.md) · [Worked examples](docs/operator-guide.md) ·
[Source build](docs/build.md) · [Dump](bins/kronika-dump/README.md) ·
[HTML reports](bins/kronika-report/README.md)
