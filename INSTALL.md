# Install on Linux

[Русская версия](INSTALL.ru.md) · [README](README.md)

Use `kronika-collector` to record Linux or PostgreSQL metrics and `kronika-web`
to view their history in a browser. The archive also includes `kronika-dump` to
inspect or extract part of a recording and `kronika-report` to create an HTML report.

## 1. Download and extract

Version **1.1.0 is unreleased**. For the examples below, [build and install this
1.1.0 source](docs/build.md), then continue with [collector startup](#3-start-collector).
If a successful development build exists for the same source revision, follow
its [download, verification and extraction instructions](docs/releases.md#development-builds).

In the extracted archive directory, check the binary before installing:

```sh
./kronika-collector --version
```

It must report `1.1.0` for the new `KRONIKA_PG_DSN` examples.

## 2. Install

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

Without `KRONIKA_PG_DSN`, the default `local` mode collects Linux only:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

Use a real storage directory, not a symlink. Root can read protected process
I/O counters and local logs. Processes are sampled every 5 seconds and core
Linux metrics every 10 seconds.

The scheduled segment closing period is 900 seconds. The first may be shorter,
a size limit can close one earlier, and ongoing collection can delay closing.
Web reads new data from `active.wal` without waiting for a finished segment.
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

For each PostgreSQL server, including primary and standby, start a separate
`kronika-collector` process with its own DSN and storage directory. All processes
use the same binary. Each process collects the accessible databases on its
server; no separate collector per database is needed. See the
[two-server example](bins/kronika-collector/README.md#several-postgresql-servers).

PostgreSQL with OS from the same VM or pod:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSN='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

| Setting or connection | Purpose |
| --- | --- |
| `KRONIKA_COLLECTOR_MODE` | `local` by default: Linux and optional local PostgreSQL. `postgresql`: PostgreSQL only, without local OS/process/cgroup reads. |
| `KRONIKA_PG_DSN` | One connection string selects one PostgreSQL server and its accessible databases. The same DSN discovers local logs in `local` mode. Required in `postgresql` mode. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Optional integer `1..4294967295`: PostgreSQL CPU capacity. A shared local machine uses recorded CPUs automatically. Remote/container capacity remains unknown without an explicit value; SQL collection continues. |
| Extension discovery | Supported `pg_stat_statements` and `pg_store_plans` interfaces are detected in connectable databases. Activity, Locks and relation statistics use PostgreSQL's built-in views. |
| Transport | DSN `sslmode=disable`, `prefer` (default) or `require`; TLS validates the CA and server hostname. `KRONIKA_PG_SSL_ROOT_CERT` replaces included public roots with a PEM CA bundle. Direct PostgreSQL and PgBouncer session pooling retain the required session state. |
| Log paths | In `local` mode, `pg_current_logfile()` discovers readable local files; `KRONIKA_PG_LOGS` adds paths/globs. In `postgresql` mode, only explicit `KRONIKA_PG_LOGS` files are read. No remote file download. PgBouncer log settings apply only to `local`. |

### PostgreSQL only — local or remote

Choose this mode for a remote server or when OS data is unnecessary.
PostgreSQL-only collection does not need sudo.

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
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
