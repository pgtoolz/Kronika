# Install on Linux

[Русская версия](INSTALL.ru.md) · [README](README.md)

Use `kronika-collector` to record your machine and `kronika-web` to view its
history in a browser. The archive also includes `kronika-dump` to inspect or
extract part of a recording and `kronika-report` to create an HTML report.

Steps 1–2 install the binary archive. To compile the programs yourself, use
the [source-build guide](docs/build.md), then return to [collector startup](#3-start-collector).
PostgreSQL collection needs a connection with monitoring permissions.

## 1. Download and extract

[Release 1.0.2](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.2)
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
version=1.0.2
archive="kronika-$version-$target.tar.gz"
release_url="https://github.com/pgtoolz/Kronika/releases/download/v$version"
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

Choose a mode: Linux and optional PostgreSQL (`local`), or PostgreSQL only
on a local or remote server (`postgresql`). Configuration is read when the
program starts.

<a id="3-record-linux"></a>
### Linux and optional PostgreSQL

For `local` mode, create a private recording directory owned by root:

```sh
sudo install -d -m 0700 /var/lib/kronika
```

Without `KRONIKA_PG_DSNS`, the default `local` mode collects Linux only:

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

PostgreSQL on the collector machine:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
  /usr/local/bin/kronika-collector
```

| Setting or connection | Contract |
| --- | --- |
| `KRONIKA_COLLECTOR_MODE` | `local` by default: Linux and optional local PostgreSQL. `postgresql`: PostgreSQL only, without local OS/process/cgroup reads. |
| `KRONIKA_PG_DSNS` | The first DSN enables server metrics. In `local` mode, all semicolon-separated DSNs also discover local logs. Required in `postgresql` mode. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Optional integer `1..4294967295`: PostgreSQL CPU capacity. A shared local machine uses recorded CPUs automatically. Remote/container capacity remains unknown without an explicit value; SQL collection continues. |
| Extension discovery | Supported `pg_stat_statements` and `pg_store_plans` interfaces are detected in connectable databases. Activity, Locks and relation statistics use PostgreSQL's built-in views. |
| Transport | DSN `sslmode=disable`, `prefer` (default) or `require`; TLS validates the CA and server hostname. `KRONIKA_PG_SSL_ROOT_CERT` replaces included public roots with a PEM CA bundle. Direct PostgreSQL and PgBouncer session pooling retain the required session state. |
| Log paths | In `local` mode, `pg_current_logfile()` discovers readable local files; `KRONIKA_PG_LOGS` adds paths/globs. In `postgresql` mode, only explicit `KRONIKA_PG_LOGS` files are read. No remote file download. PgBouncer log settings apply only to `local`. |

### PostgreSQL only — local or remote

PostgreSQL-only collection does not need sudo.

```sh
KRONIKA_COLLECTOR_MODE=postgresql \
  KRONIKA_STORAGE_DIR="$HOME/kronika-data" \
  KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
  /usr/local/bin/kronika-collector
```

If the server has 4 available CPUs, add `KRONIKA_POSTGRES_EFFECTIVE_CPUS=4`. Otherwise leave it
unset: SQL metrics remain available and PostgreSQL Health is unknown.
See [remote PostgreSQL](bins/kronika-collector/README.md#remote-postgresql).

[Service configuration](docs/services.md) stores DSNs and web credentials in
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
recording directory to create search indexes (`.idx`) and a lock file that
prevents concurrent index rebuilds. The `/var/lib/kronika` example runs both programs as root with private storage.

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
