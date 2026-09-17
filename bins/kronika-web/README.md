# kronika-web

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

`kronika-web` lets you browse the history recorded by collector and read it
through HTTP API and MCP. It reads the current journal (`active.wal`) and
finished compressed files (`.zms`), and creates search indexes (`.idx`) in the
same directory.

[Storage failures and recovery](../../docs/storage-recovery.md) explains what
happens after an abrupt stop or when a recording cannot be read.

## Configuration

Command-line options override environment variables; environment variables override
built-in defaults. Settings are read and validated before runtime startup. To apply
changed settings, restart the process. Existing environment-only services continue
to work. For systemd use the [service instructions](../../docs/services.md#operations).
Run `kronika-web --help` for all options and examples. Source: [config.rs](src/config.rs).

| Option | Environment fallback | Default | Accepted value and meaning |
| --- | --- | --- | --- |
| `--storage-dir DIR` | `KRONIKA_STORAGE_DIR` | Required | Existing collector storage root containing `active.wal` and dated `YYYY/MM/DD/*.zms` files. Requires read/write access for `.idx` files and `.kronika-index.owner.lock`. |
| `--listen IP:PORT` | `KRONIKA_WEB_LISTEN` | `127.0.0.1:8080` | IP address and port, including IPv6 as `[::1]:8080`. Hostnames are not accepted. Plain HTTP. |
| `--sources SOURCES` | `KRONIKA_WEB_SOURCES` | Required | `none`, `os`, `postgresql`, or `all`. Legacy bitsets also work: `0` neither, `1` OS, `2` PostgreSQL, `3` both. |
| `--user USER` | `KRONIKA_WEB_USER` | Unset | Nonempty user name. |
| `--password PASSWORD` | `KRONIKA_WEB_PASSWORD` | Unset | Nonempty password. |
| `--demo synthetic` | `KRONIKA_WEB_DEMO` | Unset | Marks the catalog and interface as a synthetic recording. Only `synthetic` is accepted. |
| — | `TMPDIR` | System temporary directory, normally `/tmp` | Writable filesystem location for export temporary files. |

When both credentials are unset, the browser, API and MCP require no
authentication. When both are nonempty, authentication is required.
Setting only one credential or an explicitly empty value prevents startup.
Credentials can come from options, environment variables, or a combination of both.
To clear inherited credentials or demo mode, unset the corresponding environment
variables before starting the process.

`--sources` sets catalog `configured` fields. Source names and numeric values work
in both the option and its environment fallback. In the browser, configured
PostgreSQL suppresses its no-data tooltip. Recorded PostgreSQL data also
suppresses it. The OS flag remains catalog metadata. All tabs and recorded
sections remain available. Recorded health uses collector metadata.

## Run

Use the collector's recording directory. The example marks Linux as configured.
Use `--sources all` for Linux and PostgreSQL, or `--sources postgresql` for a
PostgreSQL-only recording.

```sh
sudo /usr/local/bin/kronika-web --storage-dir /var/lib/kronika \
  --listen 0.0.0.0:8080 --sources os
```

Open `http://<server-ip>:8080`.

To require sign-in, add `--user kronika --password 'replace-with-a-random-password'`.
The equivalent environment configuration remains supported:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_LISTEN=0.0.0.0:8080 KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' /usr/local/bin/kronika-web
```

API and MCP then accept HTTP Basic credentials. Protected API requests also
accept the browser session cookie.

For local access or a reverse proxy on the same machine, use
`--listen 127.0.0.1:8080`, which is also the default when unset.

## Endpoints

| Route | Methods | Contract |
| --- | --- | --- |
| `/` | `GET`, `HEAD` | Embedded browser interface. |
| `/auth/session` | `GET`, `POST`, `DELETE` | Check, create from Basic credentials, or clear a browser session. Cookies receive `Secure` over HTTPS. |
| `/api/export?from=<unix_second>&to=<unix_second>` | `GET` | HTML attachment for inclusive whole-second bounds. |
| Other `/api/*` | `GET` | JSON/NDJSON resources for recorded data. |
| `/mcp` | `POST` | Stateless Streamable HTTP. Query strings and `Origin` headers are rejected. [MCP reference](../../docs/mcp-clients.md). |

## Export files

An export creates two temporary files: sliced ZMS, and a file used first for
slice scratch data then for the complete HTML. Both exist simultaneously and
are deleted when closed. The service account needs write access and space for
both. The default temporary directory works without additional setup. If an
existing systemd service restricts writable paths, you can configure a separate
directory. For the root service in the [service guide](../../docs/services.md),
create it first:

```sh
sudo install -d -m 0700 /var/tmp/kronika-web
```

Add these settings to that service:

```ini
[Service]
Environment=TMPDIR=/var/tmp/kronika-web
ReadWritePaths=/var/tmp/kronika-web
```

After changing the service settings, reload systemd and restart that service.
A service running as another user needs ownership or write access to the chosen
directory. Each process prepares at most one export at a time. Sources: [export.rs](src/export.rs),
[config.rs](src/config.rs).

## Process interface

`-h` prints a brief option reference, `--help` adds examples and operational notes,
and `--version` prints the version. All three write to stdout and exit before
configuration validation, storage access or runtime startup. Request, connection
and export errors and export timings go to stderr. Web has no log-level setting. `Ctrl+C` or
`SIGTERM` terminates web. Startup/configuration errors exit nonzero.
