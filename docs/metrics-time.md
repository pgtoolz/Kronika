# Recorded time, calculations, and heatmaps

[Русская версия](metrics-time.ru.md) · [Reference index](features.md) · [Linux](metrics-linux.md)

## Time and selection

A snapshot contains values collected at one time. A gauge, such as process RSS (memory resident in physical RAM), describes that moment. A cumulative counter, such as bytes read, gives a rate when two observations are compared. Recorded `ts` values are Unix microseconds. Let `t₀` and `t₁` be the observation times, `x₀ = x(t₀)` and `x₁ = x(t₁)` their field values. Then `Δt = (t₁ − t₀) / 1,000,000` is elapsed seconds, `Δx = x₁ − x₀` is the change, and `R(x) = Δx / Δt` is change per second. Each calculation selects its own observation pair, as described below.

| Control or value | Definition |
|---|---|
| Calendar hour | Selects the recorded interval `[h, h + 3,600,000,000)` in microseconds. The displayed calendar and clock use the selected display timezone. |
| Cursor | Requested position inside that hour. A table resolves the latest eligible snapshot at or before the cursor. Sections with different cadences can resolve different timestamps. PostgreSQL sections partitioned by database resolve each database independently. |
| Sample time | Actual timestamp of the selected data. The common time label displays the resolved sample time; a requested cursor between samples does not create a sample. |
| Previous/next sample | Moves through the merged, sorted, distinct observation timestamps of the current surface. The selected timeline lane does not set the navigation cadence. |
| Chart hover/readout | Reads recorded chart points. Chart selection updates the common cursor. A null chart point remains null and terminates the corresponding drawn path. |
| Heatmap cell click | Sets the cursor to the cell's exclusive upper boundary minus one microsecond; the table then applies its snapshot selection. |
| Entity row | Selects an entity and its Inspector detail panel. A history button selects its plotted field; a lens is a named set of fields such as CPU or Memory. |
| Live refresh | The visible current hour refreshes every 15 seconds. A hidden document stops the timer; visibility restoration refreshes the current hour. A cursor following the newest point advances; a manually selected cursor stays fixed. Completed hours have no periodic refresh. |

Sources: [snapshot selection](../crates/kronika-query/src/snapshot/mod.rs), [surface selector](../crates/kronika-query/src/snapshot/selector.rs), [cursor timestamps](../bins/kronika-web/ui/src/cursor-timestamps.ts), [refresh](../bins/kronika-web/ui/src/refresh.ts), [heatmap cursor](../bins/kronika-web/ui/src/activity.tsx).

Collector intervals are seconds. A per-source zero interval makes that source due on each timer wake; it does not advance the wake by itself. `KRONIKA_INTERVAL_S` is the maximum collection-timer sleep, default 5 seconds; positive source deadlines or segment age can shorten it. `KRONIKA_INTERVAL_S=0` disables timed collection; `SIGUSR2` forces all sources due. Rotation retains its separate timer. The denominator of a displayed rate is elapsed recorded time.

| Source | Environment variable | Default, s |
|---|---|---:|
| Core Linux counters | `KRONIKA_OS_CORE_INTERVAL_S` | 10 |
| Processes | `KRONIKA_OS_PROCESS_INTERVAL_S` | 5 |
| Process status | `KRONIKA_OS_PROCESS_STATUS_INTERVAL_S` | 30 |
| Mounts and topology | `KRONIKA_OS_MOUNTTOPO_INTERVAL_S` | 60 |
| Cgroup controllers | `KRONIKA_OS_CGROUP_INTERVAL_S` | 30 |
| PID-to-cgroup mapping | `KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S` | 30 |
| Logs | `KRONIKA_LOG_INTERVAL_S` | 10 |
| PostgreSQL | `KRONIKA_PG_INTERVAL_S` | 30 |
| PostgreSQL relations | `KRONIKA_PG_RELATIONS_INTERVAL_S` | 300 |

Sources: [scheduler defaults](../bins/kronika-collector/src/scheduler.rs), [configuration](../bins/kronika-collector/src/config.rs), [`timer_sleep_delay`](../bins/kronika-collector/src/main.rs).

## Pair rules and units

| Calculation path | Pair and unavailable result |
|---|---|
| Process snapshot rates | Same numeric process ID (PID) in the preceding process snapshot, with equal recorded `starttime`. Missing predecessor, changed `starttime`, absent optional value, decreasing counter, or nonpositive `Δt` gives null for the affected rate. Equal counters give zero. |
| Process summary | Sums usable per-process rates at each process snapshot. Gauges sum present values. A metric with no contributing value is null; process/runnable/PostgreSQL counts can be zero. |
| Process inspector history | Adjacent recorded observations of the selected PID and field. An explicit null clears its predecessor; a negative difference or nonpositive elapsed time produces null. This history calculation does not test `starttime`. |
| Host/container rate lanes | Adjacent entries of that lane's counter series. A negative difference produces null for that pair. Explicit null in a nullable series clears the predecessor. |
| Device/cgroup derived history | Integer subtraction precedes floating-point conversion. Missing operands, negative component differences, or nonpositive elapsed time give null. Latency also requires a positive operation difference. |
| Heatmap counter summary | Last counter minus first counter for the identity in the requested range. It requires two timestamps and a nonnegative endpoint difference. It does not sum only the positive adjacent differences. |
| Health | Uses its own rules, scope, and PostgreSQL age bound in the health section below. |

Within an hour, the recorded process identity is numeric PID alone. The `starttime` check above belongs specifically to snapshot and summary rate calculation. Source definitions: [process identity](../crates/kronika-registry/src/codec/os_process.rs), [snapshot predecessor](../crates/kronika-query/src/snapshot/mod.rs), [summary reducers](../crates/kronika-query/src/hour/process_summary.rs), [inspector history](../bins/kronika-web/ui/src/detail.tsx), [lane rates](../crates/kronika-query/src/hour/lanes.rs), [device calculations](../bins/kronika-web/ui/src/system-view.tsx).

| Display conversion | Rule |
|---|---|
| Process CPU | Recorded jiffies divided by recorded positive `clock_ticks_per_sec`; CPU seconds per wall second are core equivalents. |
| Linux memory | Recorded KiB multiplied by 1,024. |
| PostgreSQL blocks | Recorded block count multiplied by recorded `block_size`. Heatmap cuts fall back to raw counts if block size is absent; derived byte metrics that require block size remain unavailable. |
| Byte formatting | Binary steps of 1,024; up to one decimal below 100 scaled units, otherwise whole units. |
| Percent formatting | Up to one decimal; `0 < x < 0.1` displays `<0.1%`. |
| Core formatting | Up to three decimals. |
| Duration formatting | Converts the declared input unit to ns, µs, ms, s, min, or h according to magnitude. `TIME` uses whole CPU seconds in `MM:SS`, `H:MM:SS`, or `D-HH:MM:SS`. |
| Null | Displays `—`; zero remains a numeric zero. |

Formatting is applied after calculation. Source: [formatters](../bins/kronika-web/ui/src/model.ts), [heatmap cut scaling](../bins/kronika-web/ui/src/activity-cuts.ts).

## Chart statistics

Chart statistics describe the distribution of each drawn line. Only its finite numeric values in the rendered frame participate; null, NaN and infinite values are excluded. Every observation has equal weight, regardless of time to the next observation. Let `n` be the number of values and `x₁ … xₙ` those values sorted ascending. The percentile at fraction `q` is `p(q) = x[max(1, ceil(q × n))]`, where `ceil` rounds upward and `q = 0.50, 0.90, 0.99` selects P50, P90 or P99. `Min = x₁`, `Max = xₙ`; `Last` is the final finite value in time order. There is no interpolation or duration weighting. An empty input produces no statistics row. Source: [`seriesStats`, `chartStatsRows`](../bins/kronika-web/ui/src/uplot-chart.tsx).

## Heatmaps

### Cells and ranking

An activity heatmap shows each entity’s contribution over the selected hour and when it was busiest. Processes, Statements, Plans, databases, cgroup CPU and cgroup I/O use 60 columns; Tables and Indexes use 12. Let `h` be the hour start in Unix microseconds, `C` the column count and `j` a boundary number from 0 to `C`. The boundary is `bⱼ = h + floor(j × 3,600,000,000 / C)`, where `floor` rounds down. Cell `j` covers `[bⱼ, bⱼ₊₁)`, including its start and excluding its end.

The engine assigns an observation to the column containing the midpoint between that observation and its previous observation for the identity; the first observation uses its own timestamp. A counter entering a new column carries the previous observation into the column's calculation. It allocates the interval to one column; it does not split the counter difference proportionally over every crossed boundary.

| Value | Counter | Gauge |
|---|---|---|
| Entity cell | `(last − first) / elapsed_seconds` where `first`/`last` are the first/last observations in the cell, including its carried predecessor, and `elapsed_seconds` is their elapsed time | Last observation accumulated in the cell |
| Entity ranking and right summary | Last minus first counter over the requested range | Maximum observed value over the range, except RSS Grid below |
| Group ranking | Sum of member entity summaries | Sum of member entity summaries |
| Group cell | Sum of available member cells | Sum of available member cells |
| Total cell | Sum of available cells across all entities | Sum of available cells across all entities |
| Other cell | Sum for entities/groups outside the displayed top list | Same |
| Total/Other right summary | Sum of corresponding counter summaries | Maximum of the corresponding cell sums; RSS Grid uses the additive mean below |

No contributing value produces null. A single counter observation cannot form a rate or counter total. Counter summaries inspect their two endpoints; an intermediate decrease does not independently invalidate the whole range. `RankingOnly` results expose the maximum individual gauge summary as `totals_total`; Grid Total is the maximum aggregate cell value. Sources: [`Obs`, `Accumulator`, `column_of_span`](../crates/kronika-query/src/heatmap/execution.rs), [typed queries](../crates/kronika-query/src/heatmap/query.rs).

### RSS Grid mean

For `os_process.rmem_kb` in Grid mode, let `T` be the set of distinct timestamps at which the query observes a usable RSS value, and `N = |T|`. The summary of PID `p` is `meanRSS(p) = Σ recordedRSS(p,t) / N`. A PID absent at a timestamp contributes nothing to the numerator; the denominator is shared by all PIDs. Group, Total, and Other summaries sum these means and retain the same denominator. Multiply KiB by 1,024 for bytes. This is a sample mean, without time weighting. Cells retain their gauge rule. `RankingOnly`, including MCP rankings, retains gauge maxima. Source: [`RssMean`, `score`, `additive_summary`](../crates/kronika-query/src/heatmap/execution.rs); existing checks: [RSS artifact test](../bins/kronika-web/src/tests/artifacts/heatmap_rss.rs).

### Cuts, grouping, and scales

Processes group by recorded `comm`; clicking a group label applies that text to process search. Group membership is assigned when the engine first encounters the entity in the range. Cgroup activity labels have no table-filter action. Table/index grouping follows the selected relation level. Compact heatmaps display eight rows; expanded top choices are 10, 25, 50, and 100. Hidden rows are folded into Other.

| Heatmap | Available cuts: recorded operands |
|---|---|
| Processes | CPU: `utime + stime`; RSS: `rmem_kb`; Read: `read_bytes`; Write: `write_bytes`; Major faults: `majflt`; Run delay: `rundelay_ns` |
| Statements | Execution time: `total_exec_time`; Calls: `calls`; Rows: `rows`; Shared read: `shared_blks_read`; Shared dirtied: `shared_blks_dirtied`; Temp written: `temp_blks_written`; WAL: `wal_bytes` |
| Plans | Execution time: `total_time`; Calls: `calls`; Rows: `rows`; Shared read: `shared_blks_read`; Temp written: `temp_blks_written` |
| Tables | Writes: `n_tup_ins + n_tup_upd + n_tup_del`; Sequential read: `seq_tup_read`; Heap read: `heap_blks_read`; Dead tuples: `n_dead_tup` (gauge); Autovacuum time: `total_autovacuum_time` |
| Indexes | Scans: `idx_scan`; Tuples read: `idx_tup_read`; Blocks read: `idx_blks_read` |
| Databases | Commits: `xact_commit`; Rollbacks: `xact_rollback`; Read: `blks_read`; Temp bytes: `temp_bytes`; Deadlocks: `deadlocks` |
| Cgroup CPU | CPU: `usage_usec`; Throttled: `throttled_usec` |
| Cgroup I/O | Read/write bytes: `rbytes`, `wbytes`; Read/write operations: `rios`, `wios` |

For a positive cell value `value` and scale maximum `scaleMax`, heatmap intensity is `min(6, max(1, ceil(6 × sqrt(value / scaleMax))))`; `sqrt` is the square root and `ceil` rounds upward. Zero has intensity zero and null draws no cell. Global scale uses the maximum cell across displayed rows and Other. Row scale uses the maximum within that row. Total always uses its own maximum. These controls change color thresholds, not the calculated values. Sources: [cuts](../bins/kronika-web/ui/src/activity-cuts.ts), [grouping and controls](../bins/kronika-web/ui/src/activity.tsx), [color scale](../bins/kronika-web/ui/src/heatmap.ts).

## Health

Health is an integer percentage describing the recorded load inputs. Its OS component uses time when tasks waited for CPU, memory or I/O. PSI is the kernel’s record of this waiting time. Let `E = t₁ − t₀` be elapsed microseconds and `S_cpu`, `S_memory`, `S_io` the increases of each resource’s PSI `some_total`, also in microseconds. The largest increase is capped at the elapsed time: `W = min(E, max(S_cpu, S_memory, S_io))`. In the formula below, `floor` rounds down.

`OS health = 100 − floor((100 × W + floor(E / 2)) / E)`.

All three PSI components and a positive interval are required; a decreasing component makes that pair null. A null PSI snapshot clears the previous snapshot. Machine recordings use host PSI (`scope = 0`); container recordings use pod/container PSI (`scope = 1` or `3`). The recorded environment and boot identity select the scope and bind predecessor samples. Ambiguous metadata yields unknown health.

Let `A` be the count of recorded `pg_stat_activity` rows whose state is exactly
`active`, and `C` be PostgreSQL CPU capacity in cores. Every active row
contributes, including an active non-client backend. `C` can be fractional;
service slots `K = 2 × C`.

`PG penalty = 0` if `A = 0`; otherwise `round(100 × max(A − 2 × C, 0) / A)`.

`PostgreSQL health = 100 − PG penalty`.

`round` selects the nearest integer, with halves rounded up. For `C = 8`,
`A = 16` and `A = 20` give Health `100` and `80`; for `C = 1.5`, `A = 3` and
`A = 4` give `100` and `75`.

Capacity is selected at each PostgreSQL sample timestamp in this order:

| Source | Value of `C` |
| --- | --- |
| Explicit positive `instance_metadata.postgresql_effective_cpus` | Recorded `KRONIKA_POSTGRES_EFFECTIVE_CPUS` (`1..4294967295`), overriding automatic calculation |
| Machine/VM, no override | Count of distinct `os_cpu.cpu_id ≥ 0` in the latest complete CPU snapshot at or before the PostgreSQL timestamp; excludes aggregate `cpu_id = −1` |
| Container, no override | Latest `os_cgroup_context` for collector's own resource scope at or before the PostgreSQL timestamp: positive quota `Q` divided by positive period `P`, capped by positive `cpuset_cpus` when recorded |
| Container with recorded quota `−1` | Positive `cpuset_cpus`, or `null` when absent |
| Container with missing or invalid quota/period | `null` |

`Q` and `P` are recorded microseconds; `150000/100000 = 1.5` cores. Context
already records the tightest visible ancestor quota. A changed quota or CPU
snapshot affects subsequent PostgreSQL samples only. VM capacity uses neither
CPUFreq policy count nor a union of CPU IDs over the segment. Unknown container
capacity stays null.

Omitting the override assigns PostgreSQL the collector's VM/container resource
scope. Remote PostgreSQL or a different cgroup requires an explicit target
PostgreSQL capacity, including a different cgroup on the same host. This is a
deployment contract; DSN, hostname and PID do not verify it. `KRONIKA_PG_DSNS`
enables PostgreSQL collection. Missing active-count input or capacity gives null.
Conflicting activity layouts at the same timestamp give an unknown count.

Health and active-backend marks use the same resolved `C`. Calculation reads
recorded WAL/ZMS facts; resources of the machine opening the recording do not
participate. An HTML report keeps the query engine embedded when it was generated;
generate a new report to use a newer calculation.

At each OS health timestamp, overall health uses the latest PostgreSQL health at or before it, no older than recorded `postgresql_interval_seconds`:

`Overall health = max(0, OS health − PG penalty)`.

Disabled PostgreSQL contributes zero penalty. Enabled PostgreSQL with unknown or older input makes overall health null; unknown OS health always makes overall health null. Web source flags do not participate in these formulas. Sources: [formulas](../crates/kronika-index/src/health.rs), [CPU capacity](../crates/kronika-index/src/cpu_capacity.rs), [scope, activity counts, and time selection](../crates/kronika-index/src/build.rs), [collector metadata](../bins/kronika-collector/src/service_sections.rs).

## Timeline marks

Each mark stores a source locator: segment, physical layout, row, field, timestamp, and kind. `KnownBad` uses fixed predicates. Linux CPU/load/memory/filesystem/OOM and overall-health predicates are listed in [Linux marks](metrics-linux.md#fixed-marks-and-cell-colors); PostgreSQL predicates are:

| Source | Predicate |
|---|---|
| Activity | Active-row count `> 2 × C`; requires a usable capacity and an unambiguous activity layout at that timestamp |
| Locks | Recorded `blocked_by` list is nonempty |
| Database deadlocks | Later `deadlocks` exceeds the predecessor for the same layout and `datid` |
| Database checksum failures | Later available `checksum_failures` exceeds the preceding available value for that database/layout; an explicit null breaks the pair |
| Database fatal/killed sessions | Positive increase of `sessions_fatal` or `sessions_killed`, tested independently for the database/layout |
| Database transaction-ID age | `frozen_xid_age ≥ 1,600,000,000` or `min_mxid_age ≥ 1,600,000,000`; each field tested independently |
| Archiver | Later `failed_count` exceeds the predecessor |
| Slow-query log group | Finite `max_duration_ms ≥ 5,000` |
| Error log group | Recorded category `5` (`data_corruption`) |

The age mark uses the fixed 1.6-billion boundary. It does not read a server's current freeze/failsafe settings. All counter-increase predicates require a later timestamp; a single counter sample creates no increase mark. `Event` marks locate recorded rows in the six supported indexed PostgreSQL log layouts: errors, checkpoints, autovacuum/autoanalyze, slow queries, lock waits, lifecycle. A data-corruption row also receives a KnownBad locator. Locators are sorted, deduplicated, then limited to 4,096 per physical-section block; the block retains the pre-limit count.

`Spike` / “Sharp rise” is a reserved locator kind retained for stored-index compatibility. The current index builder emits no Spike marks and implements no sharp-rise threshold or formula. Source: [all emitted predicates](../crates/kronika-index/src/detect/direct.rs), [event layout selection and block construction](../crates/kronika-index/src/detect/mod.rs), [locator kinds and limit](../crates/kronika-index/src/findings.rs).
