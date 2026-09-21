# Install on Linux

[Русская версия](INSTALL.ru.md) · [README](README.md)

Use `kronika-collector` to record Linux or PostgreSQL metrics and `kronika-web`
to view their history in a browser. The archive also includes `kronika-dump` to
inspect or extract part of a recording and `kronika-report` to create an HTML report.

## 1. Download and extract

Download the [1.2.3 release archive](https://github.com/pgtoolz/Kronika/releases/tag/v1.2.3)
for your architecture. The commands below use x86-64. For ARM64, set
`target=aarch64-unknown-linux-musl`.

```sh
target=x86_64-unknown-linux-musl
archive="kronika-1.2.3-$target.tar.gz"
curl -fLO "https://github.com/pgtoolz/Kronika/releases/download/v1.2.3/$archive"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
```

[Archive contents and checksums](docs/releases.md#download). To build the programs
yourself, follow the [source-build guide](docs/build.md), then continue with
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
sudo /usr/local/bin/kronika-collector \
  --storage-dir /var/lib/kronika
```

Processes are sampled every 5 seconds and core Linux metrics every 10 seconds.

`Ctrl+C` stops collection. Run the same command to resume.

The default [storage limit](bins/kronika-collector/README.md#storage) is **2 GiB** (`--retention`).

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
server with its `--pg-dsn` and a separate `--storage-dir`. See the
[two-server example](bins/kronika-collector/README.md#several-postgresql-servers).

PostgreSQL and Linux metrics from the same VM or pod:

```sh
sudo /usr/local/bin/kronika-collector \
  --storage-dir /var/lib/kronika \
  --pg-dsn 'host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable'
```

On a machine shared with PostgreSQL, the CPU count is determined automatically.
Leave `--postgres-effective-cpus` and `KRONIKA_POSTGRES_EFFECTIVE_CPUS` unset.
Installed `pg_stat_statements` and `pg_store_plans` extensions supply query
and plan statistics. Activity, Locks,
and table and index statistics use PostgreSQL's built-in views.

### PostgreSQL only — local or remote

Choose this mode for a remote server or when you do not need Linux metrics.

```sh
sudo install -d -m 0700 -o "$(id -u)" /var/lib/kronika

/usr/local/bin/kronika-collector \
  --mode postgresql \
  --storage-dir /var/lib/kronika \
  --pg-dsn 'host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres'
```

If you know the server’s available CPU count, add `--postgres-effective-cpus 4`,
replacing `4` with that number. If unknown, leave it unset: SQL metrics remain
available and PostgreSQL Health is unknown.
See [remote PostgreSQL](bins/kronika-collector/README.md#remote-postgresql).

[Service configuration](docs/services.md) stores the DSN and web credentials in
root-readable environment files. [Collector reference](bins/kronika-collector/README.md)
defines intervals, supported extension layouts and log formats.

## 4. Start web

Run `kronika-web` with the collector’s data directory.

### For `local` mode

```sh
sudo /usr/local/bin/kronika-web --storage-dir /var/lib/kronika \
  --listen 0.0.0.0:8080
```

### For `postgresql` mode

```sh
/usr/local/bin/kronika-web --storage-dir /var/lib/kronika \
  --listen 0.0.0.0:8080
```

Open `http://<server-ip>:8080`.

To require sign-in, add `--user kronika --password 'replace-with-a-random-password'`.
Options override their environment fallbacks; existing `KRONIKA_WEB_*` service
settings continue to work. See `kronika-web --help` for the full option list.

Web requires write access to the recording directory to create search indexes
(`.idx`).

See the [web configuration reference](bins/kronika-web/README.md) for all options.

The examples use `--listen 0.0.0.0:8080` to listen on all IPv4 interfaces.
The default listen address is `127.0.0.1:8080`.

MCP uses the same listener and authentication at `/mcp`.
[Client setup](docs/mcp-clients.md) is also available in the **AI**
panel. [Systemd](docs/services.md) defines persistent services.

## Reference

[Controls](docs/features.md) · [Worked examples](docs/operator-guide.md) ·
[Source build](docs/build.md) · [Dump](bins/kronika-dump/README.md) ·
[HTML reports](bins/kronika-report/README.md)
