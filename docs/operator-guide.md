# Recorded-hour operations

[Русская версия](operator-guide.ru.md) · [Controls](features.md#controls) · [Metric reference](features.md)

The four cases use the recorded Linux/PostgreSQL demo workload on **5 September 2026, 19:00–20:00 UTC**. Figures are from the linked HTML; displayed values are rounded. [Install and record your own host](../INSTALL.md).

## Select time and an object

1. Select **UTC**, then the recorded day/hour.
2. Set the cursor using the timeline or ←/→. The chart domain is the selected hour; each source resolves its own observation as described in [Time](metrics-time.md).
3. Select the view and table mode (Lens), which chooses columns and initial sorting. Apply Search with Enter; select a row to open its Inspector panel. Detail shows recorded fields and Chart shows history. The chart metric sets the vertical axis; the cursor selects the time to read.
4. Open Activity to rank objects by their contribution over the whole hour. Global uses a common intensity scale, Per row uses each row's maximum. A cell selects `cell.to−1 µs`; a supported row label applies the object/group filter.

| Operation | Resulting selection |
| --- | --- |
| Processes → row → Activity | PostgreSQL sample nearest to the cursor for this exact process number (PID). |
| Statements → Query ID / Open plans | Plans filtered by database, role and public Query ID at the same hour/cursor. |
| Plans → Related statements | Statements filtered by database, role and recorded related Query ID. |
| Tables → Indexes | Indexes for the selected recorded relation. |
| Locks → row | Blocker PIDs, lock target and backend context in Inspector. |
| Events → group → representative | Complete stored log occurrence and its timestamp. |

## 1. Container and host resource denominators

[Host, 19:00:33 UTC](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.0.html?at=1788634833931637&view=host)

![Container, Network namespace and Host readings](images/host-scopes.png)

1. Open **Host → Container → CPU**. Read CPU usage and the recorded effective CPU limit; select the CPU history.
2. Open **Host → CPU**, then Container Memory and Host Memory. Each selected measure has its own scope and denominator.
3. Open **Network namespace** for RX/TX at the same cursor.

| Readout | Calculation / scope |
| --- | --- |
| Container CPU **66.8%** | `100 × used cgroup cores / effective cgroup CPU capacity`. |
| Host CPU **17.5%** | `100 × R(user+nice+system+irq+softirq+steal)/(H×N)`, where `R` is the counter increase per second, `H` is recorded accounting ticks/s, and `N` counts distinct nonnegative CPU IDs recorded across the source segment. |
| Container memory **53.8%** | `100 × memory.current / effective memory limit`. |
| Host memory **12.9%** | Host used memory share; operands in [Linux](metrics-linux.md). |
| Throttled **34.9%** | Cgroup throttled time / observed wall interval × 100. |
| CPU PSI **4.3%** | Cgroup CPU `some_total` increase / observed wall interval × 100, with both durations in microseconds. |
| RX **284 KiB/s**, TX **284 KiB/s** | Separate byte counter rates of the recorded network namespace. |

The [Linux reference](metrics-linux.md) defines effective ceilings, PSI units, device identities and USE reductions.

## 2. Command contribution and one PID

[Processes CPU, 19:03:39 UTC](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.0.html?at=1788635019201666&lens=cpu)

![Whole-hour command Activity and Processes CPU](images/processes.png)

1. Open **Processes → CPU → Activity → CPU time** with **Global** scale.
2. Click `postgres` to apply its command filter. Clear the chip, select PID **64**, command `/usr/local/bin/kronika-demo`, and open its CPU history.
3. Select **Activity → RSS** for mean memory per command; select **Memory** for PID 64's cursor value. Select **Disk** for its storage/logical I/O counters.

| Readout | Calculation / population |
| --- | --- |
| `postgres`: **517 PIDs**, **9.44 min** | Distinct numeric PIDs observed under the command during the hour; sum of process CPU counter changes divided by recorded accounting ticks/s. |
| `kronika-demo`: **4.75 min**; Total **14.6 min** | Command CPU sum; Total includes all commands. |
| Cell `postgres` **953 ms/s**, `kronika-demo` **79.8 ms/s** | CPU seconds per observed cell interval: displayed rates equal about **0.953** and **0.0798** used cores. |
| PID 64 user **0.12 cores**, system **0.006 cores** | Adjacent same-PID/same-starttime `Δutime/(HZ×Δt)` and `Δstime/(HZ×Δt)`, where `HZ` is accounting ticks/s and `Δt` is elapsed seconds; combined displayed contribution ≈ **0.126 cores**. |
| RSS **Average** | Sum of recorded command RSS divided by the shared count of timestamps containing usable process RSS values. |

The command summary spans the hour; the PID table uses its adjacent observation pair. [Heatmap and RSS operands](metrics-time.md).

## 3. Statement interval and stored plan

[Statements, 19:00:33 UTC](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.0.html?at=1788634833931637&view=pg.statements)

1. Open **Activity → Execution time**. Apply `query_id:-665077864269413128`; select the customer-order lookup.

```sql
select id, status, total_cents from shop.orders
where customer_id = $1 order by placed_at desc limit $2
```

![Statement SQL, interval rates and hour contribution](images/statements.png)

| Readout | Formula |
| --- | --- |
| Hour execution contribution **16.7 min** | Accumulated `total_exec_time` change, converted from milliseconds to minutes. |
| Interval **19:00:28 → 19:00:33** | Statement's two recorded observations; exact timestamps determine `Δt` in seconds. |
| **120 calls/s** | `Δcalls / Δt`: executions per second. |
| **1.42 s/s** | `Δtotal_exec_time / (1000×Δt)`. Concurrent elapsed execution durations add. |
| **11.9 ms/call** | `Δtotal_exec_time / Δcalls`; calculated before display rounding. |

2. Select **Per call**, **I/O** and **Resources** for interval measures. **Stability** shows recorded Mean/Min/Max/Stddev and their coefficient of variation, CV = Stddev/Mean, for the extension statistics period; [all operands](metrics-postgresql.md).
3. Use **Open plans** and choose Plan ID **`1544266440`**. Inspector contains `Parallel Seq Scan on orders`, `Sort` on `placed_at DESC`, `Gather Merge`, `Limit`, `Workers Planned: 1` and `(customer_id = 4244)`.
4. Use **Related statements** or browser Back. Open **Tables**, apply `schema:shop AND table_name:orders`, select **Access**, **Size and buffers**, then the relation's Indexes.

![Stored plan text and related query](images/plans.png)

## 4. Blocker chain and backend state

[Locks, 19:00:33 UTC](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.0.html?at=1788634833931637&view=pg.locks)

![Root holder and two waiters](images/locks.png)

| Recorded field | Value |
| --- | --- |
| Root PID / state | **3765**, `idle in transaction`. |
| Waiting PIDs / state | **4761**, **4762**, `active`. |
| Wait type / event | `Lock` / `transactionid`. |
| Mode / target | `ShareLock` / transaction **4700**. |
| Wait starts | **19:00:19** for both waiters. |
| Application | `checkout-api`. |

1. Select PID 4761's waiting row; read exact blocker PIDs, target and backend text in Inspector. Select the root row for its state/context.
2. Open **Activity** manually at the preserved cursor and filter `pid:4761`. Query time is `sample−query_start` for `active`; transaction time is `sample−xact_start`; time in state is `sample−state_change` except exact `idle`. Here `sample` is the recorded snapshot time. The start/state timestamps are Unix microseconds; their differences are elapsed microseconds. Divide the differences by 1000 for milliseconds.
3. Open **Events**, select lock waits and open the representative record for logged wait duration in milliseconds and holder PID text.
4. Step to the next recorded instant with →. Activity and Locks select their own source observations.

## Additional operations

| Sequence | Exact reading |
| --- | --- |
| Tables → schema/database/tablespace group → object → Maintenance / Freeze | Group reducers and each table's maintenance counters, timestamps and XID ages; [PostgreSQL formulas](metrics-postgresql.md). |
| Vacuum → episode → phase / Process | Recorded phase runs, heap scan fraction and endpoint-matched process deltas. |
| Host → Storage → I/O / Filesystems / Topology | Per-device counter rates, mount capacity gauges, recorded device edges. |
| PostgreSQL → Overview → `pg_wal` | Current directory bytes with history; WAL generation has a separate counter rate. |
| Events → source → group → minute strip | Grouped occurrence weight in that minute, then representative text. |
| Export → This hour / From / To → Download | Inclusive whole-second range and standalone HTML, [Export inputs](features.md#export). |
| Connect an AI agent → client → Copy | Client connection configuration for stored-data MCP tools, [tool inputs](features.md#mcp). |
