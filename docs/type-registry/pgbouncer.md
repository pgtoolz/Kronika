# Class 2: PgBouncer log events

[Русская версия](pgbouncer.ru.md)

The type range is `2_100_001`–`2_199_999`; `pgbouncer_events` contains one row per recognized event. The [codec](../../crates/kronika-registry/src/codec/pgbouncer_events.rs) defines fields; the [Events reference](../features.md#events) defines UI grouping.

## Source and fields

Using the connections in `KRONIKA_PGBOUNCER_DSNS`, the collector discovers `logfile` through `SHOW CONFIG`. Paths and patterns in `KRONIKA_PGBOUNCER_LOGS` add files explicitly. The files must be readable on the collector host and use the layout below. The source collects log events; `SHOW POOLS`, `SHOW STATS`, `SHOW CLIENTS` and `stats:` lines are not collected.

| Field | Nullable | Definition |
| --- | --- | --- |
| `ts` | no | Line timestamp, Unix microseconds. |
| `source_file` | no | Name of the file read; source identifier. |
| `level` | no | `0` FATAL, `1` ERROR, `2` WARNING, `3` LOG, `4` DEBUG, `5` NOISE. |
| `database` | yes | Database section name in `pgbouncer.ini`, from socket context. |
| `username` | yes | User or peer literal from socket context. |
| `host` | yes | Client/server address without port. |
| `text` | no | Normalized message with continuations, up to 5 KiB. |

## Layout and normalization

```text
2026-08-07 12:34:56.789 MSK [12345] LOG C-0x55f1: db/user@10.0.0.1:41537 closing because: query timeout (age=42s)
```

| Component | Parser rule |
| --- | --- |
| Time | Collector host's local zone; printed zone designation skipped. Fractional precision up to microseconds; first occurrence of an ambiguous clock. |
| Socket context | `C-` or `S-` prefix, then `db/user@host:port`. Without context, database, username and host are absent. |
| `(nodb)`, `(nouser)` | Retained as literal values. |
| `peer-7@host:port` | `database=null`, `username="peer-7"`; address retained without port. |
| Address | Suffix after the final `:` removed; IPv6 brackets and `unix(<pid>)` retained. |
| `closing because: ` | Wrapper and trailing ` (age=Ns)` removed. |
| `pooler error: ` | Line omitted. |
| Continuation | A leading-tab line attaches to the preceding record. |

The example produces `database="db"`, `username="user"`, `host="10.0.0.1"`, `text="query timeout"`. There is no separate `kind` field.

## Recognized prefixes

After wrapper removal, the message must start with one of the table values. Other records are omitted.

| Family | Exact prefixes |
| --- | --- |
| Server connection | `cannot connect`, `connect failed`, `server conn crashed?`, `server DNS lookup failed`, `server login failed`, `server login has been failing` |
| Capacity and eviction | `evicted`, `bouncer resources exhaustion`, `out of memory`, `no memory for pool`, `no memory for authentication pool`, `too many servers in the pool`, `no more connections allowed (max_client_conn)`, `client connections exceeded (max_db_client_connections)`, `client connections exceeded (max_user_client_connections)` |
| Queue and timeouts | `query_wait_timeout`, `query_timeout`, `query timeout`, `idle transaction timeout`, `transaction timeout`, `cancel_wait_timeout`, `connect timeout`, `client_login_timeout`, `suspend_timeout` |
| Connection reuse | `idle server got dirty`, `SV_IDLE server got dirty`, `SV_USED server got dirty`, `reset query failed`, `test query failed`, `exec_on_connect query failed`, `var change failed`, `invalid server parameter` |
| Pooler process | `pooler is shutting down`, `client connections dropped, exiting`, `server connections dropped, exiting`, `accept() failed`, `cannot listen on`, `kernel file descriptor limit`, `process up`, `TLS configuration could not be reloaded, keeping old configuration`, `RELOAD Failed, see logs for more details` |
| Authentication and protocol | `password authentication failed`, `SASL authentication failed`, `LDAP authentication failed`, `PAM authentication failed`, `certificate authentication failed`, `no such user`, `no such database`, `broken auth file`, `error response from auth_query`, `unable to send auth_query`, `bad packet`, `bad pkt header`, `failed to parse packet`, `old V2 protocol not supported`, `TLS handshake error` |

Read bounds and offsets: [log reading](postgresql.md#read-bounds). Sources: [parser](../../crates/kronika-source-log/src/pgbouncer.rs), [prefixes](../../crates/kronika-source-log/src/pgbouncer/events.rs), [time](../../crates/kronika-source-log/src/timestamp.rs).
