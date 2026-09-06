# kronika-web

[Русская версия](README.ru.md) · [Install](../../INSTALL.md)

`kronika-web` lets you browse the history recorded by collector and read it
through HTTP API and MCP. It reads the current journal (`active.wal`) and
finished compressed files (`.zms`), and creates search indexes (`.idx`) in the
same directory. The reading library is `kronika-reader`.

[Storage failures and recovery](../../docs/storage-recovery.md) explains what
happens after an abrupt stop or when a recording cannot be read.

## Configuration

Environment is validated before the listener binds.
Source: [config.rs](src/config.rs).

| Variable | Default | Accepted value and meaning |
| --- | --- | --- |
| `KRONIKA_STORAGE_DIR` | Required | Existing real collector storage root; read/write access for `.idx` files and `.kronika-index.owner.lock`. |
| `KRONIKA_WEB_LISTEN` | `127.0.0.1:8080` | IP address and port, including IPv6 as `[::1]:8080`. Plain HTTP. |
| `KRONIKA_WEB_SOURCES` | Required | Decimal bitset `0..3`: bit 0 marks OS configured; bit 1 marks PostgreSQL configured. `0` neither, `1` OS, `2` PostgreSQL, `3` both. |
| `KRONIKA_WEB_USER` | Required | Nonempty user name, also required with authentication disabled. |
| `KRONIKA_WEB_PASSWORD` | Required | Nonempty password, also required with authentication disabled. |
| `KRONIKA_WEB_AUTH` | `required` | `required` checks credentials/session; `disabled` permits unauthenticated access. |
| `KRONIKA_WEB_DEMO` | Unset | Only set value: `synthetic`; marks the catalog and interface as a synthetic recording. |
| `TMPDIR` | System temporary directory, normally `/tmp` | Writable filesystem location for export temporary files. |

The source bitset sets catalog `configured` fields. In the browser, the
PostgreSQL bit suppresses its no-data tooltip; recorded PostgreSQL data also
suppresses it. The OS bit remains catalog metadata. All tabs and recorded
sections remain available. Recorded health uses collector metadata.

## Run

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

On startup, stdout receives `ready <addr>`. Open <http://127.0.0.1:8080/> and
sign in with the configured account. API and MCP accept HTTP Basic credentials;
protected API requests also accept the browser session cookie.

## Endpoints

| Route | Methods | Contract |
| --- | --- | --- |
| `/` | `GET`, `HEAD` | Embedded browser interface. |
| `/auth/session` | `GET`, `POST`, `DELETE` | Check, create from Basic credentials, or clear a browser session. Cookies receive `Secure` over HTTPS. |
| `/api/export?from=<unix_second>&to=<unix_second>` | `GET` | Authenticated HTML attachment for inclusive whole-second bounds. |
| Other `/api/*` | `GET` | JSON/NDJSON resources for recorded data. |
| `/mcp` | `POST` | Stateless Streamable HTTP; same authentication. Query strings and `Origin` headers are rejected. [MCP reference](../../docs/mcp-clients.md). |

## Export files

An export creates two temporary files: sliced ZMS, and a file used first for
slice scratch data then for the complete HTML. Both exist simultaneously and
are deleted when closed. The service account needs write access and space for
both. A restricted systemd unit can use:

```ini
[Service]
Environment=TMPDIR=/var/tmp/kronika-web
ReadWritePaths=/var/tmp/kronika-web
```

Create that directory for the service account in the
[service setup](../../docs/services.md). Query preparation is limited to one
export at a time per process. Sources: [export.rs](src/export.rs),
[config.rs](src/config.rs).

## Process interface

`-h`, `--help` and `--version` print to stdout and exit before configuration,
storage access or listener startup. Request, connection and export errors and
export timings go to stderr; web has no log-level setting. `Ctrl+C` or
`SIGTERM` terminates web. Startup/configuration errors exit nonzero.
