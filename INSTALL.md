# Install on Linux

[Русская версия](INSTALL.ru.md) · [README](README.md)

Use `kronika-collector` to record Linux or PostgreSQL metrics and `kronika-web`
to view their history in a browser. The archive also includes `kronika-dump` to
inspect or extract part of a recording and `kronika-report` to create an HTML report.

## 1. Download and extract

Version **1.1.0 is unreleased**. Use a development archive for this source revision;
follow the [download and extraction instructions](docs/releases.md#development-builds).
Alternatively, [build and install from source](docs/build.md), then skip to
[collector startup](#3-start-collector).

## 2. Install

Run from the extracted archive directory:

```sh
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
```

## 3. Start collector

Choose a mode: Linux and optional PostgreSQL (`local`), or PostgreSQL only
on a local or remote server (`postgresql`). Configuration is read when the
program starts.

<a id="3-record-linux"></a>
### Linux and optional PostgreSQL

For `local` mode, create a private recording directory owned by root:

```sh
sudo install -d -m 0700 /var/lib/kronika
```

Start collecting Linux metrics:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

Processes are sampled every 5 seconds and core Linux metrics every 10 seconds.

The web interface shows recorded history and data from ongoing collection.
`Ctrl+C` stops collection and retains the journal; run the same command to resume.

`KRONIKA_RETENTION` defaults to `2147483648` bytes (2 GiB). For a fixed 10 GiB
target, add `KRONIKA_RETENTION=10737418240`.
[Storage](bins/kronika-collector/README.md#storage) defines the counted files
and deletion order.

<a id="5-postgresql"></a>
#### PostgreSQL connection

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

To collect from several PostgreSQL servers, run `kronika-collector` for each
server with its `KRONIKA_PG_DSN` and a separate `KRONIKA_STORAGE_DIR`. See the
[two-server example](bins/kronika-collector/README.md#several-postgresql-servers).

PostgreSQL and Linux metrics from the same VM or pod:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

On a machine shared with PostgreSQL, the CPU count is determined automatically;
leave `KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset. Installed `pg_stat_statements` and
`pg_store_plans` extensions supply query and plan statistics. Activity, Locks,
and table and index statistics use PostgreSQL's built-in views.

### PostgreSQL only — local or remote

Choose this mode for a remote server or when you do not need Linux metrics.

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

If you know the server’s available CPU count, set `KRONIKA_POSTGRES_EFFECTIVE_CPUS`
to that number (for example, `4`). If unknown, leave it unset: SQL metrics remain
available and PostgreSQL Health is unknown.
See [remote PostgreSQL](bins/kronika-collector/README.md#remote-postgresql).

[Service configuration](docs/services.md) stores the DSN and web credentials in
root-readable environment files. [Collector reference](bins/kronika-collector/README.md)
defines intervals, supported extension layouts and log formats.

## 4. Start web

In a second terminal, set a password and start web with the same recording
directory.

### For `local` mode

Use `KRONIKA_WEB_SOURCES=1` for Linux only, as below; change `1` to `3`
when also collecting PostgreSQL:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

### For `postgresql` mode

```sh
KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  KRONIKA_WEB_SOURCES=2 /usr/local/bin/kronika-web
```

Open <http://127.0.0.1:8080/> and sign in. Web requires write access to the
recording directory to create search indexes (`.idx`). In the `/var/lib/kronika`
example, both programs run as root and other users cannot access the storage.

`KRONIKA_WEB_SOURCES` reports which sources are configured; it does not enable
collection or hide recorded data. See the [web configuration reference](bins/kronika-web/README.md)
for authentication settings.

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
