# PostgreSQL metric reference

[Русская версия](metrics-postgresql.ru.md) · [Reference index](features.md)

## Sources, identity and notation

A snapshot contains the rows read from a source at one collection time. An object’s identity is the set of fields used to match its rows between snapshots. A counter accumulates work; a gauge is a value read at a particular time. A rate measures the counter increase per second between two snapshots. In the notation below, `x₀` and `x₁` are values of the same counter at times `t₀` and `t₁`.

The collector reads the instance named by the first `KRONIKA_PG_DSNS` entry. Database-local views are read through each connectable database; statements and plans come from one discovered installation of each extension. The [layout reference](type-registry/postgresql-metrics.md) gives exact PostgreSQL and extension versions, physical type IDs and collection scopes. The [registry](../crates/kronika-registry/src/codec) defines every stored field.

For samples at Unix microsecond timestamps `t₀ < t₁`, write `Δx = x₁ − x₀`, `d = (t₁ − t₀)/10⁶` seconds and `r(x) = Δx/d`. `x` alone is the recorded gauge or cumulative value at the selected sample. `B` is the positive integer `pg_settings.block_size` in bytes from the snapshot at the selected time (cursor). Buffer columns display `B × r(blocks)` bytes/s; Buffer bytes/call displays `B × blocks_per_call`, where `blocks_per_call` is the number of block accesses per query execution. Without recorded `B`, these byte conversions are unavailable.

Statement/plan interval calculations require matching physical type and identity, positive elapsed time and nondecreasing operands. A missing operand or decreasing counter makes that operand's pair unavailable; subsequent pairs are calculated normally. Ratios require a positive denominator. Recorded `min`, `max`, `mean`, `stddev`, timestamps and ages are gauges, including when adjacent cumulative counters decrease. Relation and Overview reducers have the specific null rules documented below. Source: [interval calculations](../bins/kronika-web/ui/src/postgres-metrics.ts), [byte conversion and column selection](../bins/kronika-web/ui/src/postgres-view.tsx).

| Stream | Object identity within the recording |
|---|---|
| Activity, Locks | Numeric `pid` |
| Database | `datid` |
| Statement | `userid, dbid, queryid`; layouts with `toplevel` add it to identity |
| Plan | `userid, dbid, queryid, planid`, within one physical extension layout |
| Table / Index | `datid, relid` / `datid, indexrelid` |
| I/O | `backend_type, object, context` |
| Settings | `datid, usesysid, name` |

## Activity and Locks

Activity shows PostgreSQL processes and their current queries. Locks connects waiting processes to the processes blocking them.

Activity’s default table hides rows with `state = idle`. It also hides system processes: a nonempty `backend_type` other than `client backend`. A missing or empty `backend_type` does not hide a row. **Idle** and **System** include those rows; an explicitly focused row remains visible. Default order is descending query duration, with transaction duration used when both query durations are unavailable. Source: [Activity columns and filters](../bins/kronika-web/ui/src/postgres-view.tsx), [duration functions](../bins/kronika-web/ui/src/postgres-activity.ts).

| Display / field | Definition and unit |
|---|---|
| Query duration / `query_duration_ms` | `(t − query_start)/1000` ms, only when `state = active` |
| Transaction duration / `transaction_duration_ms` | `(t − xact_start)/1000` ms for any state with a usable transaction start |
| State duration / `state_duration_ms` | `(t − state_change)/1000` ms, omitted for `state = idle` |
| Backend age / `backend_age_ms` | `(t − backend_start)/1000` ms |
| PID, leader PID | Backend OS PID and recorded parallel-group leader PID |
| Database, role, application, client | `datname`, `usename`, `application_name`, `client_addr` from `pg_stat_activity` |
| State, wait event type, wait event | Recorded server strings; a wait event can coexist with `state = active` |
| Query, query ID | Recorded SQL text and `query_id`; query text is bounded to 65,536 characters during collection |
| XID / xmin age | Server `age(backend_xid)` / `age(backend_xmin)`, in transaction IDs |

In each duration formula, `t` is the row's recorded timestamp. Null, nonpositive or future start timestamps yield null. A non-active row's SQL text remains its recorded last query; it has no active query-duration value.

Locks records one row per backend participating in a blocking component. The collector takes one deterministic ungranted `pg_locks` row per waiter and records sorted, distinct `pg_blocking_pids(pid)`. It excludes the collector PID and backends whose `application_name` equals the collector session's name. `blocked_by = 0` denotes a prepared transaction. Source: [lock SQL](../crates/kronika-source-pg/src/locks.rs).

| Locks value / control | Meaning |
|---|---|
| PID tree | Parent blocker before child waiter; extra blockers appear as `+PID`; disconnected components stay together |
| Prepared transaction | A `0` entry has a label but no synthetic backend row or tree edge |
| Table count | Number of displayed backend rows, including blockers; it is not a count of held locks |
| `lock_locktype`, `lock_mode` | Type and requested mode of the selected ungranted lock |
| `lock_target` | Resolved relation name, transaction/virtual-XID, object identifiers or lock type |
| `lock_database`, `lock_relation`, `lock_page`, `lock_tuple` | Database/relation OIDs and optional page/tuple coordinates |
| `lock_classid`, `lock_objid`, `lock_objsubid` | Catalog-object coordinates for object locks |
| `waitstart` | Server lock-wait start timestamp, PostgreSQL 14+; the table displays a timestamp |
| Backend context | Database, role, application, client, state, wait event and query copied in the lock query |
| Search | Matching rows retain their blocker ancestors and extra blocker context |

A relation name resolves only in the collector connection's database. Root blocker rows can have no ungranted lock fields. The Locks inspector contains backend context and `blocked_by`; its PID and relation cells have no direct related-entity navigation. Source: [forest construction/search](../bins/kronika-web/ui/src/postgres-locks.ts), [Locks view](../bins/kronika-web/ui/src/postgres-view.tsx).

## Overview and Databases

Overview's left number summarizes the loaded hour; its right number resolves the chart at the selected time (cursor). Rates are summed by exact later-sample timestamp across identities. The hour total is the sum of each usable counter difference. The reducer skips an unavailable field pair while retaining other usable pairs, and returns null when none exist. Gauge sums/maxima skip null values; an entirely null snapshot stays null. Source: [Overview definitions](../bins/kronika-web/ui/src/postgres-overview.tsx), [reducers](../bins/kronika-web/ui/src/postgres-vitals.ts).

In this table, `Σ` sums databases except where another scope is named; **total** means summed hour differences, **peak** means maximum chart point, **last** means latest nonnull chart point.

| Overview row | Chart / cursor value | Left number |
|---|---|---|
| Client backends | Count of `backend_type = client backend` in Activity; absent backend type uses `client backend` | Peak; `/ max_connections` when recorded |
| Active vs waiting | Active client backends without / with `wait_event_type`; parallel followers excluded | Peak running count |
| Idle in transaction | Count of states starting with `idle in transaction` across Activity | Peak |
| Oldest transaction | Maximum `(t − xact_start)/10⁶` s among client backends excluding parallel followers; clamped at zero | Peak duration |
| Oldest xmin age | Maximum recorded `backend_xmin_age` across Activity | Peak, transaction IDs |
| Prepared transactions | Sum `prepared_count`; age is maximum `max_age_us` | Peak count and peak age |
| Transactions | `Σ[r(xact_commit)+r(xact_rollback)]`; second series `Σr(xact_rollback)` | Total transactions and `100 × total rollbacks / total transactions` |
| Tuples read | `Σr(tup_returned)`; second series `Σr(tup_fetched)` | Total returned tuples |
| Tuples written | `Σ[r(tup_inserted)+r(tup_updated)+r(tup_deleted)]` | Total changed tuples |
| Buffer hit share | `100Σr(blks_hit)/Σ[r(blks_hit)+r(blks_read)]`, % | Same ratio using hour differences |
| Block I/O time | `Σ[r(blk_read_time)+r(blk_write_time)]`, ms/s | Total ms |
| Temp bytes | `Σr(temp_bytes)`, bytes/s | Total bytes |
| Deadlocks / Checksum failures | `Σr(deadlocks)` / `Σr(checksum_failures)`, events/s | Total events |
| Abnormal session ends | `Σ[r(sessions_fatal)+r(sessions_killed)]`, sessions/s | Total sessions |
| WAL generated | `r(pg_stat_wal.wal_bytes)`, bytes/s | Total bytes |
| Checkpoints | Timed / requested checkpoint rates | Total checkpoints and requested count |
| Checkpoint buffer writes | `r(buffers_checkpoint)`; PostgreSQL 17+ fallback `r(pg_stat_checkpointer.buffers_written)`; second series `r(buffers_backend)` where available, blocks/s | Total checkpoint blocks |
| WAL archiver | `r(archived_count)` / `r(failed_count)`, events/s | Both totals |
| WAL buffers full | `r(wal_buffers_full)`, events/s | Total events |
| pg_wal size | Recorded `wal_files_bytes`, bytes | Last |
| Buffer evictions / Buffer reuses | Sum `r(evictions)` / `r(reuses)` over `pg_stat_io` identities, operations/s | Total operations |
| Relation extends / Fsyncs | Sum `r(extends)` / `r(fsyncs)` over I/O identities, operations/s | Total operations |
| Vacuum reads | Sum `r(reads)` where `pg_stat_io.context = vacuum`, operations/s | Total operations |
| Transaction ID age | Maximum database `frozen_xid_age`, transaction IDs | Last; percentage of recorded `autovacuum_freeze_max_age` |
| Multixact age | Maximum database `min_mxid_age`, multixact IDs | Last |
| Autovacuum workers | Count of progress rows with `is_autovacuum = true` | Peak; `/ autovacuum_max_workers` when recorded |

Timed/requested checkpoint sources prefer `pg_stat_checkpointer.num_timed/num_requested`, falling back to `pg_stat_bgwriter.checkpoints_timed/checkpoints_req`. Prepared/progress rows are assigned to the open interval between successive database snapshot timestamps; an empty interval gives zero prepared/worker count and null prepared age. Overview's dashed limits use the latest recorded setting in the loaded hour. Passport setting values resolve at the cursor, using the first hour setting when the cursor precedes it. Live-hour Overview series refresh at most once per minute. The [activity lane source](../crates/kronika-query/src/hour/lanes.rs) defines the client/follower selection.

The **Databases** table shows `numbackends` as a gauge; transaction/session/tuple/buffer/temp/conflict/deadlock counters as adjacent-sample rates; `blk_read_time` and `blk_write_time` in ms/s; `temp_bytes` in bytes/s; `frozen_xid_age = age(pg_database.datfrozenxid)` as a gauge. `blks_read` and `blks_hit` are converted to bytes/s with `B`. `tup_returned` counts tuples returned by scans; `tup_fetched` counts live rows fetched by index scans. `conflicts` counts queries cancelled because of recovery conflicts. `datid = 0` is the shared-object statistics row and is excluded from the database-count display. Source: [database SQL and fields](../crates/kronika-source-pg/src/database.rs), [table columns](../bins/kronika-web/ui/src/postgres-view.tsx).

## Statements and Plans

Statements groups execution statistics by normalized query; Plans shows statistics for recorded execution plans. The Lens selector chooses which measurements appear as columns. **Exec time/s** measures total execution time per second of observation; **Mean/call** measures the average execution time within the selected interval.

### Interval metrics

The display title **Execution** corresponds to lens token `load`. Statements also have **Per call**, **I/O**, **Resources**, **Stability**. Plans have **Execution**, **Timing**, **I/O**, **Identifiers**. Common identity columns are database, role and query/plan IDs; SQL or the plan summary occupies the first column. Source: [lens fields and calculations](../bins/kronika-web/ui/src/postgres-metrics.ts), [display columns](../bins/kronika-web/ui/src/postgres-view.tsx).

Set `E = total_exec_time` for statement layouts 1.8+, `E = total_time` for statement 1.5–1.7 and all plan layouts; `P = total_plan_time`; `C = calls`; `H = shared_blks_hit`; `R = shared_blks_read`; `LH = local_blks_hit`; `LR = local_blks_read`.

| Display / derived field | Formula / recorded value | Unit; lens |
|---|---|---|
| Calls/s | `r(C)` | calls/s; Execution, Per call, Resources, Stability, Timing, Identifiers |
| Exec time/s | `r(E)` | ms/s; Execution, Resources; simultaneous executions add, so values can exceed 1000 |
| Mean/call | `ΔE/ΔC` | ms/call; Execution, Per call |
| Rows/s | `r(rows)` | returned/affected rows/s; Execution |
| Rows/call | `Δrows/ΔC` | rows/call; Per call |
| Buffer bytes/call | `B × [r(H)+r(R)+r(LH)+r(LR)]/r(C)` | bytes/call; Per call, I/O |
| Cache hit ratio / `hit_pct` | `100r(H)/[r(H)+r(R)]` | %; I/O; shared buffers only |
| Shared/local buffer hits, reads, dirtied, written | `B × r(shared_blks_hit/read/dirtied/written)` and equivalent `local_blks_*` | bytes/s; I/O and recorded fields |
| Temp reads / writes | `B × r(temp_blks_read/written)` | bytes/s; I/O, Resources |
| WAL bytes | `r(wal_bytes)` | bytes/s; Resources |
| WAL/call | `Δwal_bytes/ΔC` | bytes/call; Resources |
| Planning time/s | `r(P)` | ms/s; Resources |
| Plan time, % | `100r(P)/[r(P)+r(E)]` | %; Resources |
| WAL records / FPI / buffers full | `r(wal_records/wal_fpi/wal_buffers_full)` | records/full-page images/full-buffer events per second; recorded fields |
| Plans | `r(plans)` | planning operations/s; recorded statement field |
| Shared/local/temp read/write time | `r(physical timing field)` | ms/s; recorded fields |
| Mean / Min / Max / Stddev | Recorded execution-time statistics for the extension's statistics period | ms; Stability, Timing |
| CV | Recorded `stddev / mean` | dimensionless; Stability; null for nonpositive mean |
| Calls | Recorded cumulative `calls` integer | calls; Plans → Identifiers |
| Slow log calls | `r(slow_log_calls)` in vadv layout | calls/s; recorded plan field |
| First call / Last call / Stats since | Recorded `first_call`, `last_call`, `stats_since` timestamps where the layout defines them | absolute time |

`Mean/call` uses interval totals; **Mean** uses the recorded extension gauge. The table's Buffer bytes/call sums available finite block rates and requires at least one; its history calculation requires all projected block rates. Other derived ratios require every listed operand. No byte conversion changes the buffer-count ratios.

| Layout | Execution / planning timing fields | Block timing fields |
|---|---|---|
| Statements 1.5–1.7 | `total_time`; no planning | `blk_read_time`, `blk_write_time` map to shared timing |
| Statements 1.8–1.9 | `total_exec_time`, `total_plan_time` | Same shared aliases |
| Statements 1.10 | Same | Shared aliases plus `temp_blk_read_time`, `temp_blk_write_time` |
| Statements 1.11+ | Same | `shared_blk_*_time`, `local_blk_*_time`, `temp_blk_*_time` |
| OSSC / Datasentinel plans | `total_time`; no planning | Split shared/local/temp timing fields |
| vadv plans | `total_time`, `total_plan_time` | `blk_read_time`, `blk_write_time` map to shared timing |

Statement timing gauges are `min_time/max_time/mean_time/stddev_time` in 1.5–1.7 and `min_exec_time/max_exec_time/mean_exec_time/stddev_exec_time` in later layouts. Plans use the former names. The current lenses and their inspectors select the subsets listed in the lens tables; other recorded fields retain their source definitions. Layout-absent fields are omitted or unavailable. Exact fields: [statement registry](../crates/kronika-registry/src/codec/pg_stat_statements.rs), [plan registry](../crates/kronika-registry/src/codec/pg_store_plans.rs).

### Plan identity, text and related controls

OSSC and Datasentinel `queryid` is the statement query ID. In vadv, `queryid` is extension-internal and remains part of the four-part identity; `queryid_stat_statements` is the last recorded statement attribution and can change independently. A zero attribution has no statement link. Datasentinel additionally records `relids` (relation OID list) and `cmd_type` (command type). Plan text is collected through the extension's readable text interface, bounded to 65,536 characters.

**Query ID** links from Statement to Plans using database, role and query ID. Plans → Statements uses those predicates with the applicable statement attribution. **Plan ID** filters by plan ID. Activity → Statements requires nonzero `datid` and `query_id` plus database name, and uses database/query-ID predicates. The plan inspector's SQL-text lookup uses query ID alone. Source: [navigation predicates](../bins/kronika-web/ui/src/statement-navigation.ts), [plan SQL-text lookup](../bins/kronika-web/ui/src/plan-query.ts), [collection interfaces](../crates/kronika-source-pg/src/store_plans.rs).

**Kronika queries** includes the collector's own statements. They are excluded under the default `workload` scope. A nonempty search, statement context or explicitly selected collector statement forces `all` and disables the checkbox. This scope also applies to the statement activity and summary requests. Source: [scope resolver](../bins/kronika-web/ui/src/postgres-view.tsx), [collector statement matching](../crates/kronika-query/src/statement_scope.rs).

## Tables and Indexes

These tables show how relations are read, changed and maintained, how much storage they occupy, and how often accesses find a block already in PostgreSQL buffers. TOAST stores large values separately from the main table.

### Grouping, storage and null values

**Databases → Schemas → Objects** applies database OID, then database/schema predicates. **Tablespaces → Objects** uses the cluster-wide effective tablespace OID. An index uses its own placement. `reltablespace = 0` resolves to the database's `dattablespace`. Storage-less partitioned table parents have null placement and are excluded from tablespace groups. **Table indexes** and **Table** navigate with exact `datid, relid`. **Index definition** loads recorded `indexdef` for an object row. Source: [table collection](../crates/kronika-source-pg/src/user_tables.rs), [index collection](../crates/kronika-source-pg/src/user_indexes.rs), [navigation](../bins/kronika-web/ui/src/postgres-relations.ts).

A group sums object rates after dividing each object's difference by its own elapsed time: `R(x) = Σᵢ Δxᵢ/dᵢ`. Gauge totals are `G(x) = Σᵢ xᵢ`. Group ratios use those summed numerators and denominators. XID/MXID ages use the maximum. Timestamp groups retain oldest, latest and count of null recorded timestamps (**Never…**). A table without TOAST contributes no TOAST timestamp and no Never-TOAST count.

A missing layout field or unusable object pair makes the corresponding grouped metric null. Two explicit nulls for table index/TOAST counter fields are structurally inapplicable and contribute no value. TOAST gauge nulls are neutral when `toast_bytes` is null. Combined throughput/buffer/storage metrics accept these structural omissions; a component with no applicable value remains null on its own. `reltuples = -1` is unavailable. Source: [relation reducer, `Aggregate::metric`, `counter_input`, `gauge_input`](../crates/kronika-query/src/hour/relation.rs).

| Storage field | Exact source; bytes |
|---|---|
| Table data / `main_fork_bytes` | `pg_relation_size(table_oid)`: heap main fork |
| TOAST / `toast_bytes` | `pg_total_relation_size(reltoastrelid)`: TOAST relation including its auxiliary forks and indexes; null without TOAST |
| Table + TOAST / `displayed_storage_bytes` | Heap main fork + TOAST; excludes user indexes and heap FSM/VM forks |
| Index data / `main_fork_bytes` | `pg_relation_size(index_oid)`: index main fork |

### Table lenses

For one object, `R = r` and `G(x) = x`; for groups use the preceding definitions. Let `D = R(n_tup_ins)+R(n_tup_upd)+R(n_tup_del)`.

| Lens: display / field | Formula, unit and meaning |
|---|---|
| Access: Tuples scanned | `R(seq_tup_read)+R(idx_tup_fetch)`, tuples/s |
| Seq scans / Index scans | `R(seq_scan)` / `R(idx_scan)`, scans/s |
| Seq scans, % | `100R(seq_scan)/[R(seq_scan)+R(idx_scan)]` |
| Tuples per seq/index scan | `R(seq_tup_read)/R(seq_scan)` / `R(idx_tup_fetch)/R(idx_scan)` |
| Last seq/index scan | Recorded timestamps, PostgreSQL 16+; groups expose oldest/latest/Never counts |
| Changes: DML total | `D`, inserted + updated + deleted tuples/s |
| Inserts / Updates / Deletes, % | `100R(n_tup_ins)/D`, `100R(n_tup_upd)/D`, `100R(n_tup_del)/D` |
| HOT updates, % | `100R(n_tup_hot_upd)/R(n_tup_upd)` |
| New-page updates, % | `100R(n_tup_newpage_upd)/R(n_tup_upd)`, PostgreSQL 16+ |
| Dead tuples, % | `100G(n_dead_tup)/[G(n_live_tup)+G(n_dead_tup)]`, snapshot estimates |
| Modified since analyze | `G(n_mod_since_analyze)`, estimated changed tuples since last analyze |
| Inserted since vacuum | `G(n_ins_since_vacuum)`, estimated inserted tuples since last vacuum, PostgreSQL 13+ |
| Maintenance: Manual vacuums / Autovacuums / Manual analyzes / Autoanalyzes | `R(vacuum_count/autovacuum_count/analyze_count/autoanalyze_count)`, operations/s |
| Mean manual vacuum / autovacuum / manual analyze / autoanalyze | `R(total_vacuum_time)/R(vacuum_count)` and corresponding matching time/count fields, ms/operation; PostgreSQL 18+ |
| Last manual vacuum / autovacuum / manual analyze / autoanalyze / TOAST autovacuum | Recorded timestamps; groups expose oldest/latest/Never counts |
| Size and buffers: Table + TOAST / Table data | Corresponding summed storage gauges, bytes |
| TOAST, % | `100G(toast_bytes)/[G(main_fork_bytes)+G(toast_bytes)]` |
| Estimated rows | `G(reltuples)`, `pg_class.reltuples` cast to integer during collection |
| TOAST dead tuples, % | `100G(toast_n_dead_tup)/[G(toast_n_live_tup)+G(toast_n_dead_tup)]` |
| Heap / Index / TOAST / TOAST-index buffer hit share | For prefix `q = heap, idx, toast, tidx`: `100R(q_blks_hit)/[R(q_blks_hit)+R(q_blks_read)]` |
| Buffer hit share | Same formula summed over all four prefixes |
| Buffer read/hit bytes | `B × R(q_blks_read/hit)`, bytes/s; corresponding four storage components |
| Freeze: XID age / MXID age | Maximum `age(pg_class.relfrozenxid)` / `mxid_age(pg_class.relminmxid)`; transaction/multixact IDs |
| Freeze: Inserted since vacuum; last vacuum/autovacuum | Gauges/timestamps defined above |

### Index lenses

| Lens: display / field | Formula, unit and meaning |
|---|---|
| Usage: Index scans | `R(idx_scan)`, scans/s |
| Index tuples read / Tuples fetched | `R(idx_tup_read)` / `R(idx_tup_fetch)`, index entries / live heap tuples per second |
| Tuples per scan / Fetches per scan | `R(idx_tup_read)/R(idx_scan)` / `R(idx_tup_fetch)/R(idx_scan)` |
| Last index scan | Recorded timestamp, PostgreSQL 16+; oldest/latest/Never counts in groups |
| Low activity | Applies `no_scans = true`: usable `Δidx_scan = 0` in the current calculation pair |
| No scans / No-scan count / Known-scan count | Object boolean; group count with `Δidx_scan = 0`; group count with `Δidx_scan > 0` |
| Size and buffers: Index data | `G(main_fork_bytes)`, bytes |
| Buffer hit share | `100R(idx_blks_hit)/[R(idx_blks_hit)+R(idx_blks_read)]` |
| State: valid / ready / unique / primary / exclusion | `pg_index.indisvalid/indisready/indisunique/indisprimary/indisexclusion` booleans |
| State group counts | Count of false valid/ready or true unique/primary/exclusion flags |
| State order / `state_severity` | `2` invalid; `1` valid but unready; `0` valid and ready; group maximum |
| Access method | `pg_am.amname`; Index definition is recorded `pg_get_indexdef(index_oid)` |

The low-activity selection is interval-local. A zero rate does not mean that the index's cumulative counter or earlier recorded rates are zero. Source for both lens tables: [relation definitions and histories](../bins/kronika-web/ui/src/postgres-relations.ts), [query reducer](../crates/kronika-query/src/hour/relation.rs).

## Vacuum progress

This view groups progress snapshots into observed VACUUM runs, called episodes below. It shows each run’s phase, processed blocks and process resource use.

The source is `pg_stat_progress_vacuum`, joined to Activity for `is_autovacuum` and to the connection's catalogs for relation names. Each row identifies `pid, datid, relid`; physical layouts are PostgreSQL 10–16, 17 and 18. Source: [collection](../crates/kronika-source-pg/src/progress_vacuum.rs), [episode calculations](../bins/kronika-web/ui/src/postgres-vacuum.ts), [display](../bins/kronika-web/ui/src/postgres-view.tsx).

| Field / display | Definition |
|---|---|
| Kind | `is_autovacuum = (backend_type = autovacuum worker)`, false when Activity join has no match |
| Phase | Recorded phase string |
| Scan progress | `clamp(100 × heap_blks_scanned / heap_blks_total, 0, 100)`, null for nonpositive/missing total |
| Heap scanned / total | `B × heap_blks_scanned` / `B × heap_blks_total`, bytes; total is heap blocks at scan start |
| Heap vacuumed | `B × heap_blks_vacuumed`, bytes |
| Index cycles | Recorded `index_vacuum_count`; counts greater than 1 receive the repeated-cycle label |
| Index progress | `indexes_processed / indexes_total`, PostgreSQL 17+, displayed only in vacuuming/cleaning-up indexes phases |
| Dead tuple capacity/use, PostgreSQL 10–16 | `max_dead_tuples`, `num_dead_tuples`, tuple counts |
| TID store, PostgreSQL 17+ | `max_dead_tuple_bytes`, `dead_tuple_bytes`, bytes; `num_dead_item_ids`, count |
| Cost delay, PostgreSQL 18 | Cumulative `delay_time`, ms; `+Δdelay_time` uses the episode's last two samples and requires nondecrease |
| In phase | Number of trailing samples with the same phase and index cycle; span from first to last such timestamp |
| No movement | At least 3 trailing unchanged designated-counter samples in that phase/cycle; displays count and recorded span |

Episode key is physical type + PID + database OID + relation OID. A new episode starts when `index_vacuum_count`, `heap_blks_scanned` or `heap_blks_vacuumed` decreases, or adjacent samples are more than `2.5 × recorded postgresql_interval_seconds` apart. With no positive recorded cadence, only identity/counter rules apply. The row is the episode's final sample. **At cursor** means its final timestamp equals the latest progress timestamp at or before the cursor; other episodes show their own last recorded time.

| Phase | Fixed phase label | No-movement operand |
|---|---|---|
| `initializing`, `performing final cleanup` | Ordinary | None |
| `scanning heap` | Ordinary | `heap_blks_scanned` |
| `vacuuming heap` | Heavy | `heap_blks_vacuumed` |
| `vacuuming indexes`, `cleaning up indexes` | Heavy | `indexes_processed`, PostgreSQL 17+ |
| `truncating heap` | Dangerous | Repeated phase samples; phase is set before the conditional exclusive-lock attempt |

Process load resolves this PID's OS samples at or before the episode's first and last timestamps. With recorded clock rate `H` ticks/s, CPU ms = `1000Δ(utime+stime)/H`; CPU share = `min(100,100 × CPU seconds / OS sample elapsed seconds)`; block-wait ms = `1000Δblkdelay_ticks/H`. Read/write bytes and major faults are `Δread_bytes`, `Δwrite_bytes`, `Δmajflt`. Read share = `min(100,100Δread_bytes/(B × final heap_blks_scanned))`. These deltas include all work performed by that PID between the selected OS samples. Missing clock/block size suppresses only the dependent calculation.

## Summary strips and value marks

In the formulas below, `count(...)` counts matching objects, `usable` requires a valid value under that metric’s rules, and `AND` requires both conditions.

The strip above each table summarizes the full set of objects at the selected time. It is separate from the table’s current page and text search.

Summary strips resolve a whole-surface snapshot at or before the cursor, independently of table pagination and text search. Statement and plan summaries use the selected statement scope. Each ratio includes only objects with the required operand pair; numerators and denominators are accumulated together. Here `ΣΔ` denotes these admitted differences at the selected summary snapshot, not an hour total. Source: [summary stream](../crates/kronika-query/src/hour/postgres_summary.rs), [formula implementation](../crates/kronika-query/src/hour/postgres_summary/facts.rs).

| Summary | Formula |
|---|---|
| Active statements / Used plans | Count of usable `Δcalls > 0`; percent of objects with usable `Δcalls` |
| Execution per call | `ΣΔE / ΣΔcalls` |
| WAL per call | `ΣΔwal_bytes / ΣΔcalls` |
| Executions per used plan | `ΣΔcalls / count(Δcalls > 0)` |
| Reads outside buffers | `100ΣΔread / ΣΔ(read+hit)`; Statements/Plans: shared+local; Databases: `blks_*`; Tables: heap+index+TOAST+TOAST-index; Indexes: index only |
| Rollbacks / Temp per transaction | `100ΣΔxact_rollback / ΣΔ(xact_commit+xact_rollback)`; `ΣΔtemp_bytes / ΣΔ(xact_commit+xact_rollback)` |
| Scan methods | `100ΣΔseq_scan / ΣΔ(seq_scan+idx_scan)`; index share is its complement |
| HOT updates / Dead rows | `100ΣΔn_tup_hot_upd/ΣΔn_tup_upd`; `100Σn_dead_tup/Σ(n_live_tup+n_dead_tup)` |
| Vacuumed tables | `100 × count[Δ(vacuum_count+autovacuum_count)>0] / count[usable Δ(vacuum_count+autovacuum_count)]` |
| Storage | `100Σtoast_bytes/Σ(main_fork_bytes+toast_bytes)`; main share is its complement |
| XID boundary | `100 × count(xid_age ≥ 1,600,000,000) / count(usable xid_age)` |
| Scanned indexes / Without scans | Percent of usable `Δidx_scan` that are positive / zero |
| Usable indexes | `100 × count(indisvalid AND indisready) / count(objects with both flags)` |

Value colors are fixed display comparisons from [value-tone.ts](../bins/kronika-web/ui/src/value-tone.ts). Query and transaction duration colors apply only to active client backends. Zero rates use inactive color before these comparisons.

| Value | Good | Warning | Critical |
|---|---|---|---|
| Query duration | — | 1000–<5000 ms | ≥5000 ms |
| Transaction duration | — | 5000–<60000 ms | ≥60000 ms |
| Mean/call, Mean | — | — | ≥5000 ms |
| Cache hit ratio | ≥99% | 90–<99% | <90% |
| CV | <1 | 1–<3 | ≥3 |
| Plan time | <50% | 50–<80% | ≥80% |
| State | — | `idle in transaction` | `idle in transaction (aborted)` |
| Wait event type | — | Any nonempty value | — |

## Settings, reset timestamps and WAL storage

`pg_settings` records the effective collector metric session with database and login-role identity. It is emitted on first success, a change and each new segment. `primary_conninfo` and `ssl_passphrase_command` are excluded. The passport displays server version, max connections, shared buffers, max WAL size, checkpoint timeout, autovacuum and I/O-timing settings. Its changes list compares successive recorded `setting` strings by setting name. Source: [settings collection](../crates/kronika-source-pg/src/settings.rs), [passport lookup](../bins/kronika-web/ui/src/postgres-vitals.ts).

`stats_reset` fields in database, WAL, I/O, writer/checkpointer, archiver and extension-info sections are the server's reset timestamps. `stats_since` is the statement row's statistics-start timestamp when its layout provides it; `first_call`/`last_call` bound the recorded plan's calls. They are absolute recorded timestamps; the current interface does not derive a reset-age value.

`pg_wal_storage.wal_files_bytes = COALESCE(SUM(size), 0)` over the regular files returned by `pg_ls_waldir()` at that query's `statement_timestamp()`. This is current directory file size in bytes. It includes files by their returned sizes without classification by filename. The collector does not scan subdirectories; permission failure omits the section. Source: [WAL storage query](../crates/kronika-source-pg/src/wal_storage.rs).

### PostgreSQL maintenance rules

These equations describe PostgreSQL server scheduling. Kronika records the table statistics and metric-session settings above; it does not collect table `reloptions`, `relpages` or `relallfrozen`, and computes no table eligibility verdict.

Let `N = max(reltuples,0)`, `D = n_dead_tup`, `I = n_ins_since_vacuum`, `M = n_mod_since_analyze`. `V₀/Vₛ`, `I₀/Iₛ`, `A₀/Aₛ` denote effective `autovacuum_vacuum_threshold/scale_factor`, `autovacuum_vacuum_insert_threshold/scale_factor`, `autovacuum_analyze_threshold/scale_factor`. A table storage parameter overrides the corresponding global value.

| Server rule | Equation |
|---|---|
| Dead-tuple vacuum | `D > V₀ + VₛN`; PostgreSQL 18 caps the right side at `autovacuum_vacuum_max_threshold` when it is nonnegative |
| Insert vacuum, PostgreSQL 13–17 | `I > I₀ + IₛN`; disabled when `I₀ = -1` |
| Insert vacuum, PostgreSQL 18 | `I > I₀ + IₛNf`, where `f = 1 − min(relallfrozen,relpages)/relpages` if both inputs are positive, otherwise `f = 1`; disabled when `I₀ = -1` |
| Analyze | `M > A₀ + AₛN` |
| XID / MXID forced vacuum | `age(relfrozenxid) > Xmax` or `mxid_age(relminmxid) > MXmax`, using valid IDs and wraparound-aware server comparisons |

`Xmax` is the smaller of a configured table freeze-max-age and the global maximum; without a table override it is the global maximum. `MXmax` similarly caps the table value by the server's effective multixact maximum, which can be lower than its configured maximum. Exact threshold implementation: [PostgreSQL 17](https://github.com/postgres/postgres/blob/REL_17_STABLE/src/backend/postmaster/autovacuum.c), [PostgreSQL 18](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/postmaster/autovacuum.c), `relation_needs_vacanalyze`.

The first insert rule starts in PostgreSQL 13. Version-specific definitions: [PostgreSQL 13](https://www.postgresql.org/docs/13/routine-vacuuming.html#AUTOVACUUM), [PostgreSQL 17](https://www.postgresql.org/docs/17/routine-vacuuming.html#AUTOVACUUM), [PostgreSQL 18](https://www.postgresql.org/docs/18/routine-vacuuming.html#AUTOVACUUM). Forced age work applies even when ordinary autovacuum is disabled. Freezing advances `relfrozenxid`/`relminmxid`; `VACUUM` reclaims reusable tuple space and maintains visibility, while `ANALYZE` updates planner statistics. These server rules are separate from the Overview's recorded configured limits and the summary's fixed 1.6-billion XID mark.
