# Interface and data reference

[Русская версия](features.ru.md) · [Operator guide](operator-guide.md) · [README](../README.md)

Kronika records Linux snapshots, PostgreSQL statistics and parsed log events. The web application and exported HTML read those recordings through the same query engine.

| Reference | Contents |
| --- | --- |
| [Time, aggregation and Health](metrics-time.md) | Hour/cursor selection, sample intervals, counter differences, heatmaps, chart statistics, Health and marks. |
| [Linux](metrics-linux.md) | Processes, Host, container cgroups, CPU, memory, storage, filesystems, network, topology and USE. |
| [PostgreSQL](metrics-postgresql.md) | Overview, Databases, Activity, Locks, Vacuum, Statements, Plans, Tables, Indexes and Settings. |
| [Events](#events) | Log grouping keys, reductions, units and representative records. |
| [MCP](#mcp) | Fourteen tools, typed inputs, limits and result fields. |
| [Export](#export) | Time editor, inclusive whole seconds and standalone HTML. |
| [Installation](../INSTALL.md) | Prebuilt programs, collection, web access and source builds. |

## Controls

A snapshot is a set of values recorded during one source poll. The cursor selects a time; each table chooses its source snapshot for that time. A view selects the kind of object, such as processes or events. A Lens selects columns and initial sorting. The Inspector is the selected row’s panel: Detail shows fields and Chart shows history.

| Control | State or operation |
| --- | --- |
| Day/hour; previous/next hour | Selected calendar hour in Browser time or UTC. Charts retain the full hour domain. |
| Browser time / UTC | Civil-time display and calendar selection. Stored Unix microseconds retain their value. |
| Workspace clock; timeline pointer | Cursor. Pointer previews a recorded time; release commits it. Leaving a preview restores the committed cursor. |
| ← / →; previous/next observation | Previous/next member of the sorted distinct recorded timestamps loaded for the current view, including different source cadences. Buttons, text inputs, selects and editable content retain their own arrow keys. |
| Refresh | Reload catalog and selected view. Current visible hour refreshes 15 seconds after completion; visibility restoration refreshes immediately. Historical hours do not poll. Pinned cursor stays fixed; following cursor advances to the latest observation. The selected hour remains fixed. |
| View | Processes, Host, PostgreSQL or Events. |
| Lens | Column set and default ordering for one table; metric definitions remain those in the domain reference. |
| Column heading | Sort the complete eligible result before splitting into pages; hierarchy controls retain tree/chain order. |
| Search; Apply / Enter | Commit a valid expression. Invalid drafts keep the previous applied result and URL. Chips remove individual applied terms. |
| Load more / Retry | Fetch the next page / retry the failed request. Pending/error status applies to retained rows until the request completes. |
| Row; Inspector Detail / Chart | Select identity; show recorded facts or selected history metric. Related tabs depend on the row type. |
| Chart metric; series / All | Choose the plotted measure and its series. Legend maps colours to series. Hover reports values without changing selection. |
| Expand / restore chart | Change Inspector chart size. |
| Inspector divider | Resize; keyboard arrows adjust width, Home/End choose limits. Narrow layouts use an overlay or bottom sheet. |
| Activity heading | Load/open whole-hour ranking. Compact view keeps eight ranked rows and folds remaining contributions into Other; full screen offers Top 10/25/50/100, initially 25. |
| Global / Per row | Heatmap colour denominator: common maximum / each row's maximum. Total always has its own scale. |
| Heatmap cell | Set cursor to `cell.to − 1 µs`; the table then selects its source snapshots. |
| Activity entity label | Apply the corresponding table’s entity/group filter where supported; move to its busiest interval when absent at the cursor. Cgroup labels have no table-filter action. |
| `?`; field help; Esc | Open general help / metric definition; close an open panel or selection. |
| Copy exact value | Copy the unrounded value; fallback selects the exact text. |
| Language; theme | EN/RU; light/dark. Stored in browser preferences, together with Activity open state and Inspector width. |
| Sign out | End the live web session. |

The URL carries hour/cursor, view, lens, sort, search and supported row selection. Back/Forward restores them. Ordinary navigation between object types clears `find`; related-object navigation supplies the target expression and preserves time.

Sources: [address and navigation](../bins/kronika-web/ui/src/address.ts), [keyboard](../bins/kronika-web/ui/src/keyboard.ts), [refresh](../bins/kronika-web/ui/src/refresh.ts), [Activity](../bins/kronika-web/ui/src/activity.tsx), [Inspector](../bins/kronika-web/ui/src/inspector.tsx).

## Search

| Syntax | Meaning |
| --- | --- |
| Plain text | Text search in the current table. |
| `field:value` | Named string/identity predicate; quotes protect spaces, `*` and `?` match string patterns. |
| `field>quantity`, `field<quantity` | Strict comparison in the field's accepted units; null does not match a quantity. |
| `AND`, `OR`, parentheses | Case-insensitive operators, `AND` precedence above `OR`; maximum 8 predicates, 31 tokens, 4 nested groups and 1024 characters. |
| `MB`, `MiB`, `/s` | Decimal bytes, binary bytes, per-second quantities. Unit spelling is case-sensitive. |
| Grouped Tables/Indexes | Names filter members before aggregation; quantities filter reduced groups. `AND` joins those phases; an `OR` mixing phases is rejected. |

`NOT`, implicit boolean operators, `=`, `==`, `!=`, `>=` and `<=` are rejected. Search help lists each table’s accepted fields and units, including fields outside the current lens. Filters apply before sort and pagination.

```text
command:postgres* AND rss>100MiB
state:active AND wait_type:Lock
query_id:-665077864269413128
schema:shop AND table_name:orders
```

Sources: [search parser](../bins/kronika-web/ui/src/search.ts), [shared search definitions](../crates/kronika-query/src/snapshot/search.rs).

## Configured and recorded sources

| Input | Meaning |
| --- | --- |
| Collector `KRONIKA_PG_DSNS` | Configured connections enable PostgreSQL collection. |
| Recorded `instance_metadata` | Environment, collection cadence, PostgreSQL enabled flag and optional explicit database CPU capacity override. Used by scope/time/Health calculations. |
| Required web `KRONIKA_WEB_SOURCES` | Unsigned catalog bitset: `0` neither configured flag, `1` OS, `2` PostgreSQL, `3` both. It labels configured sources; it does not filter stored data, control collection, hide tabs or calculate Health. |
| Configured PostgreSQL UI flag | Together with recorded PostgreSQL presence, controls suppression of the PostgreSQL no-data tooltip. The configured OS bit has no UI consumer. |
| Recorded physical layout | Determines available fields for that PostgreSQL version or extension variant. |

Sources: [web config](../bins/kronika-web/src/config.rs), [source availability](../bins/kronika-web/ui/src/source-availability.ts), [collector config](../bins/kronika-collector/src/config.rs).

## Events

Events reads a selected range of at most one hour. For a group, `firstTs=min(t)`, `lastTs=max(t)`. Its 60 minute cells sum occurrence weights with `bucket=floor((t−from)/60,000,000)` for Unix-microsecond timestamps. Group counts can use a different reduction, specified below. Numeric durations are recorded in milliseconds and use the shared adaptive duration formatter.

| Group / recorded section | Group key | Count, metrics and representative |
| --- | --- | --- |
| Errors / `pg_log_errors` | `(severity, category, pattern)` | `Σ count` (missing count contributes 1). Severity, SQLSTATE and category from earliest representative. Database/user shown only when the same nonempty value occurs in every member. |
| Slow queries / `pg_log_slow_queries` | `pattern` | `Σ count`; `totalMs=Σ total_duration_ms`; `maxMs=max(max_duration_ms)`. Missing numeric values contribute 0 for these duration reductions. Representative has maximum duration, earliest time on ties. Threshold is the latest recorded nonnegative `log_min_duration_statement`, converted from s/min to ms where needed. |
| Autovacuum / `pg_log_autovacuum` | `(kind, relation)` | Runs = recorded row count; total duration = sum of available `elapsed_ms`; removed tuples = sum of available `tuples_removed`; dead-not-removable tuples = value in the latest row. `kind=1` denotes analyze. Earliest representative. |
| Checkpoints / `pg_log_checkpoints` | One completion/start group | `starts=count(phase=0)`, `completes=count(phase=1)`, displayed count `max(starts,completes)`. Timed = starts whose reason contains `time`; requested = starts − timed. Maximum `sync_ms` and sum of `buffers_written` from completion rows. First encountered representative. |
| Checkpoint warnings | `phase=2` | Warning count; minimum available `seconds_apart` (seconds); first encountered representative. |
| Lock waits / `pg_log_lock_waits` | Exact `holding_pids` text | Waiting `kind=0` rows establish groups. Acquired rows join the latest earlier waiting row with the same `(pid,lock_target)`. Count = waiting records (minimum 1); waiters = distinct recorded PID strings; max duration = maximum available `duration_ms`; targets = distinct target texts. Unmatched acquired records form a separate group counted by occurrences. |
| Lifecycle / `pg_log_lifecycle` | One group per stored row | Kind, PID, signal and shutdown mode from that row. Count 1. |
| PgBouncer / `pgbouncer_events` | `(level, exact text)` | Row count; earliest representative. Database/user/host/source file shown only if all members share the same nonempty value. |

Optional sums/maxima retain null if no member supplies a value. Earliest representatives use physical encounter order to break equal timestamps. Groups sort by tier, descending count, descending last time, then key.

| Tier | Exact membership |
| --- | --- |
| Critical | PostgreSQL FATAL/PANIC (`severity=1/2`); lifecycle `kind=0`; PgBouncer `level=0`. |
| Notable | Other PostgreSQL errors except WARNING/LOG; slow queries; checkpoint warnings; lock waits; other lifecycle; PgBouncer `level=1/2`. |
| Routine | PostgreSQL WARNING/LOG (`severity=3/4`); autovacuum/analyze; checkpoints; other PgBouncer levels. |

`(nodb)` and `(nouser)` are literal PgBouncer connection-context values for an unset database/user. The collector preserves them; a missing context field is null. Host strips the connection port. PgBouncer contributes console groups without shared-timeline marks.

The source/type summary filters groups; Search matches displayed title/chips with `text`, `kind`, `source`, `category`. Expand shows group metrics; representative selection fetches its complete recorded row. A timeline cluster selects its interval and sources; Show all restores the hour. Threshold and sharp-rise marks occupy a separate list. `pg_log_temp_files` is available through MCP `occurrences`, outside the grouped console.

Sources: [event query and fields](../crates/kronika-query/src/events.rs), [group reductions](../crates/kronika-query/src/events/group.rs), [console controls](../bins/kronika-web/ui/src/events-view.tsx), [PgBouncer parser](../crates/kronika-source-log/src/pgbouncer.rs).

## MCP

`POST /mcp` exposes fourteen stored-data tools over stateless Streamable HTTP. Each tool returns structured JSON. HTTP Basic authentication uses the web credentials unless `KRONIKA_WEB_AUTH=disabled`; user and password remain required configuration in either mode. The endpoint rejects `Origin` and query strings. The tools read stored files and execute no host or PostgreSQL administration commands.

| Tool | Inputs | Result |
| --- | --- | --- |
| `kronika_list_recorded_sections` | Optional `section` | `recorded_from`, exclusive `recorded_to`; sections with source family, row/byte counts, fields, classes and units. |
| `kronika_get_instance` | `settings`: `non_default` (default) or `all` | Latest host metadata and latest PostgreSQL settings with separate `host_as_of`/`settings_as_of`, returned count and scope. `non_default` removes only exact recorded `source="default"`. |
| `kronika_rank_metrics` | `from`, `to`, nonempty `rankings:[{section,fields,top?}]`; 1–4 fields; top 1–500, default 25 | One independent ordered result per field, including repeats. Counter changes or gauge maxima, unit, identities, labels, detail refs, total/other and entity count. Each field has its own top limit. |
| `kronika_find_processes` | Common finder inputs | Process rows. |
| `kronika_find_postgresql_activity` | Common finder inputs | Backend states, waits and start timestamps. |
| `kronika_find_postgresql_locks` | Common finder inputs | Recorded locks and blocker context. |
| `kronika_find_postgresql_vacuum` | Common finder inputs | Vacuum progress rows. |
| `kronika_find_postgresql_databases` | Common finder inputs | Database statistics. |
| `kronika_find_postgresql_statements` | Common finder inputs | Statement interval metrics and identities. |
| `kronika_find_postgresql_plans` | Common finder inputs | Plan interval metrics and identities. |
| `kronika_find_postgresql_tables` | Common finder inputs; required `group`: `object`, `schema`, `database`, `tablespace` | Reduced table rows. |
| `kronika_find_postgresql_indexes` | Common finder inputs; required `group`, same values | Reduced index rows. |
| `kronika_find_events` | `from`, `to`, `limit`; `representation=groups` (default) or `occurrences`; optional `sources` | `groups` or `occurrences` plus `truncated`; range at most one hour. Missing/null sources selects supported sources; `[]` selects none. Temp files require occurrences. |
| `kronika_get_row_detail` | Required unchanged `detail_ref` | Complete stored row; text objects contain `stored_text`, decimal `full_len`, `truncated`, `sha256`. |

Common inputs for the nine point-in-time find tools: optional `at` (default latest timestamp across storage), optional `filters` and `sort:{field,direction}`, required `limit` 1–5000. Direction is `asc`/`desc`, nulls last; omitted sort keeps identity order. Output is `{rows,truncated}` without pagination cursors. Row detail references identify physical recorded objects; aggregates without one underlying row omit them.

Typed filters contain `field`, `op` and `value`, or `values` for `in`. Up to eight filters combine with AND; `in` contains 1–8 values combined with OR. Text accepts case-insensitive `eq`, literal `contains`, `in`; identifiers accept `eq`/`in` with integer or exact decimal-string values; quantities accept strict `gt`/`lt` with nonnegative JSON integers in the field's documented base unit. `tools/list` supplies field names for each object type and units.

Time inputs accept Unix-microsecond integers or canonical signed decimal strings, timezone-qualified RFC 3339, `now`, and `now-Nus/ms/s/m/h/d/w`. `now` is request time. Range tools use `[from,to)`; finders resolve observations at or before `at` using section cadence. Finder/Events/ranking encoded arguments are limited to 65,536 bytes. Errors carry `isError`, `record="error"`, `message` and, where applicable, `valid_options` or `ranking_index`.

The **Connect an AI agent** panel contains endpoint `<origin>/mcp`, credential state, Claude Code/Codex CLI/Cursor selector, generated connection text and Copy. It reads `/api/mcp-access` for the authorization header; unavailable access data produces a placeholder. The panel configures a client connection. [Client commands and configuration](mcp-clients.md).

Sources: [tool schemas](../bins/kronika-web/src/mcp/catalog.rs), [typed filters](../bins/kronika-web/src/mcp/filter.rs), [time](../bins/kronika-web/src/mcp/time.rs), [result envelopes](../bins/kronika-web/src/mcp/semantics.rs), [connection panel](../bins/kronika-web/ui/src/mcp-connect.tsx).

## Export

| Input/control | Definition |
| --- | --- |
| From / To | Inclusive whole Unix seconds `F,T`, `0<F≤T`; duration `T−F+1` seconds. Editor uses the selected Browser time/UTC zone and separate endpoint dates. |
| This hour | `F=floor(hour_us/10⁶)`, `T=F+3599`. |
| Around cursor ±5/15/30 min | For `C=floor(cursor_us/10⁶)` and `N` minutes: `F=C−60N`, `T=C+60N−1`; duration `120N` seconds. |
| Previous/next hour | Add −3600/+3600 to both endpoints. |
| Day and `HH:MM:SS` editors | Resolve civil date/time in the displayed zone. A nonexistent DST clock is rejected; repeated clock asks for its first/second occurrence. |
| Filename | `kronika-YYYY-MM-DD-HHMMSS-YYYY-MM-DD-HHMMSS-utc.html`, both endpoints formatted in UTC. |
| Download | `GET /api/export?from=F&to=T`; exported visible range `[10⁶F,10⁶(T+1))`. All recorded sections in the range are included. |
| Progress | Preparation elapsed seconds, then bytes received/total where known; completed filename, bytes and elapsed time. Previous preparation duration is a browser preference. |

The HTML embeds the recording, UI, fonts and WASM query engine. It opens from a local file and evaluates requests synchronously in WASM on the browser main thread. Its visible range is fixed; live refresh, login, export and MCP connection controls are disabled. Stored query text, plans, log text and command lines travel with the recording.

The four installed programs are `kronika-collector`, `kronika-web`, `kronika-dump` and `kronika-report`. CLI [slice](../bins/kronika-dump/README.md) creates a recording range; [report](../bins/kronika-report/README.md) converts it to HTML. Their `--help` describes installed-program parameters.

Sources: [range arithmetic](../bins/kronika-web/ui/src/export-range.ts), [civil time](../bins/kronika-web/ui/src/export-time.ts), [download dialog](../bins/kronika-web/ui/src/export-dialog.tsx), [offline request transport](../bins/kronika-web/ui/src/report-transport.ts).
