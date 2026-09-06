# Linux metric reference

[Русская версия](metrics-linux.ru.md) · [Reference index](features.md) · [Time and heatmaps](metrics-time.md)

Linux metrics describe CPU, memory, storage and network use by a machine, container or process. The tables give each value’s meaning, recorded fields and units. A snapshot records values at one time; a cumulative counter records an increasing total, whose change gives a rate. Let `x₀`, `x₁` be two recorded values and `Δt` their elapsed seconds: `R(x) = (x₁ − x₀) / Δt`. `H` is the recorded accounting ticks per second `clock_ticks_per_sec`. `N(t)` counts recorded online host logical CPUs at time `t`; `N_lane` is the distinct CPU count used by the CPU usage chart, with its separate rule below. [Observation-pair and unavailable-value rules](metrics-time.md#pair-rules-and-units) depend on the calculation.

## Recorded environment and scope

Scope identifies whose resources a row describes. A cgroup is a Linux process group with shared resource accounting and limits; each controller handles a resource such as CPU or memory.

| Recorded fact | Meaning and use |
|---|---|
| `instance_metadata.environment` | Collector's recorded machine/container environment. It controls whether the container resource rows and cgroup collection apply. A VM belongs to machine. |
| OS `scope` | `0`: host; `1`: pod; `2`: pod network namespace; `3`: container; `4`: unknown. |
| Host CPU, memory, pressure, block devices | Kernel values visible through configured procfs/sysfs roots. Container recordings retain host resource context where those files expose it. |
| Process table | Live PIDs visible through the configured procfs root; PID is numeric identity within the selected hour. |
| Network in a container | Interfaces and traffic visible in the network namespace, recorded with pod-network scope. |
| Cgroup resource lanes | Exact collector membership path for the controller and matching recorded scope. Workload tables can contain other directly populated cgroups visible to the collector. |
| Filesystems | Data mounts visible in the collector's mount namespace, with capacity from `statvfs` on the visible mount path. |

The collector records no workload cgroup sections on machine/VM. In a container it samples paths directly containing visible live processes, bounded to 512 controller/path candidates, 512 KiB of candidate paths, and 1,024 cgroup/device I/O rows per tick. Candidate overflow omits workload sections for that tick; I/O overflow omits I/O. Sources: [scope](../crates/kronika-source-os/src/scope.rs), [collection](../bins/kronika-collector/src/os_sources/cgroups.rs), [cgroup reader](../crates/kronika-source-os/src/cgroup.rs), [UI environment](../bins/kronika-web/ui/src/system-view.tsx).

## Processes

### Lenses and fields

A lens selects a named set of columns. PID is the numeric process ID; PPID is its parent’s ID. RSS is memory resident in physical RAM; virtual memory is the process address space, and swap is memory moved to the swap area.

| Lens | Columns |
|---|---|
| General | PID, command, PPID, real/effective user, GID/EGID, threads, TTY, exit signal, state |
| CPU | PID, command, user/system CPU, run delay, block delay, voluntary/involuntary context switches, CPU number, nice, priority, real-time priority, scheduling policy, state |
| Memory | PID, command, RSS, virtual memory, swap, minor/major faults, state |
| Disk | PID, command, read/write bytes, read/write syscalls, logical read/write bytes, cancelled writes, block delay, state |
| Tree | Real user, PID, CPU %, memory %, virtual memory, RSS, TTY, STAT, start time, cumulative CPU TIME, parent-first command tree |

Sources: [`LENS_FIELDS`](../bins/kronika-web/ui/src/process-table.tsx), [inspector history fields](../bins/kronika-web/ui/src/detail.tsx), [tree](../bins/kronika-web/ui/src/process-tree.ts).

| Field or display | Recorded source and calculation | Unit and meaning |
|---|---|---|
| PID, PPID | `/proc/PID/stat`: `pid`, `ppid` | Numeric process/parent identity |
| Command | `/proc/PID/cmdline`; fallback `/proc/PID/comm` | Recorded command text |
| Real/effective user | `/proc/PID/status` real/effective UID; collector's passwd mapping | Recorded username; numeric UID if name is absent. GID/EGID are real/effective group IDs. |
| State | `/proc/PID/stat.state` | Kernel state letter; `R` runnable, `S` sleeping, `D` uninterruptible sleep, `Z` zombie, `T` stopped, `I` idle kernel thread |
| Threads | `num_threads` | Current thread count |
| TTY, exit signal | `tty_nr`, `exit_signal` from `stat` | Controlling terminal encoding and termination signal |
| User CPU / System CPU | `R(utime) / H`, `R(stime) / H` | Core equivalents: 1 means one CPU second per wall second |
| Run delay | `R(rundelay_ns)` from `/proc/PID/schedstat` | ns/s waiting to run; formatted as duration per second |
| Block delay | `R(blkdelay_ticks)` from `stat.delayacct_blkio_ticks` | Jiffies/s in the table/history; the structured-search `block_io_delay` quantity converts to `1,000 × R(blkdelay_ticks) / H` ms/s |
| Voluntary / Involuntary switches | `R(nvcsw)`, `R(nivcsw)` from `status` | Switches/s |
| CPU, nice, priority, RT priority, policy | `curcpu`, `nice`, `prio`, `rtprio`, `policy` from `stat` | Current/last CPU number and recorded scheduler settings |
| RSS | `rmem_kb × 1,024` | Resident bytes; shared resident pages appear in each process that maps them |
| Virtual memory | `vmem_kb × 1,024` | Virtual address-space bytes |
| Swap | `vswap_kb × 1,024` from `status.VmSwap` | Process swap bytes |
| Minor / Major faults | `R(minflt)`, `R(majflt)` from `stat` | Faults/s; minor faults require no storage read, major faults require page-in I/O |
| Read / Write | `R(read_bytes)`, `R(write_bytes)` from `/proc/PID/io` | Bytes/s charged to storage I/O |
| Read / Write syscalls | `R(syscr)`, `R(syscw)` from `io` | Calls/s |
| Logical read / write | `R(rchar)`, `R(wchar)` from `io` | Bytes/s transferred through counted read/write operations, including traffic served through cache |
| Cancelled writes | `R(cancelled_write_bytes)` from `io` | Bytes/s of cancelled dirty-page writes |
| Tree CPU % | `100 × (R(utime) + R(stime)) / H` | Percentage of one logical CPU; values can exceed 100% |
| Tree memory % | `100 × rmem_kb / mem_total` | Share of recorded host `MemTotal`; positive denominator required |
| Tree TIME | `(utime + stime) / H` from cumulative `cpu_time_ticks` | Lifetime CPU seconds, formatted as a clock duration |
| Tree start / STAT | Recorded Unix start timestamp; state followed by `<` for negative nice, `N` for positive nice, `l` for multiple threads | Observed start time and flags; parent ordering uses the selected process snapshot |

`/proc/PID/io` fields are nullable. The collector tries the process's filesystem credentials and its original credentials; access failure leaves those counters unavailable. A successfully read I/O file initializes missing or malformed numeric entries to zero. An unreadable `schedstat` records zero run delay; unprovided status counters retain their parser defaults. Sources: [process registry](../crates/kronika-registry/src/codec/os_process.rs), [proc parser](../crates/kronika-source-os/src/proc/process/parse.rs), [process reader](../crates/kronika-source-os/src/proc/process.rs), [I/O reader](../crates/kronika-source-os/src/proc/process/process_io.rs), [tree calculations](../bins/kronika-web/ui/src/process-tree.ts).

`rchar` measures logical read traffic; `read_bytes` measures storage traffic charged to the process. Process lenses expose both rates separately.

### Summary and history

The summary operates on the complete process snapshot, independently of the table page and search. General and Tree show process count, sum of threads, count of `state = R`, and count of PIDs present in the latest recorded PostgreSQL activity snapshot at or before that process timestamp. CPU shows summed user/system cores, summed run delay in ms/s, and summed voluntary plus involuntary switches/s. Memory shows summed RSS, virtual memory, swap, and major faults/s. Disk shows summed read/write bytes/s and read/write calls/s.

Each rate is calculated per PID before summation. Only a PID present in the immediately preceding process snapshot with the same `starttime` contributes a rate. Available values contribute independently; a sum without any available value is null. Summed RSS counts shared pages once per recorded process mapping. Source: [`summaries`, `add_row`, `ExactSum`, `RateSum`](../crates/kronika-query/src/hour/process_summary.rs).

Inspector history uses the selected lens's temporal fields. Scheduler settings, identities, and command text are reference fields. CPU history also offers minor/major faults; Tree offers CPU, memory, and disk history fields. Process activity heatmaps group by `comm`; CPU ranks by CPU-time increase, RSS by the [shared-timestamp mean](metrics-time.md#rss-grid-mean).

## Host CPU and pressure

CPU shares show how processor time was spent between two observations. For the aggregate host `/proc/stat` row, let `u,n,s,i,w,q,f,z` be the changes of `user,nice,system,idle,iowait,irq,softirq,steal`, respectively, in clock ticks. These represent user code, niced user code, kernel code, idle time, I/O wait, hardware interrupts, software interrupts and time taken by the hypervisor. `T = u+n+s+i+w+q+f+z` is total elapsed processor time; `B = T−i−w` excludes idle and I/O wait. The symbol `Σ` below means a sum over the stated rows or policies.

| Display | Formula or recorded field | Unit |
|---|---|---|
| CPU usage / USE Busy | `100 × R(user+nice+system+irq+softirq+steal) / (H × N_lane)` | % of host CPU capacity |
| CPU used, cores | `N(t) × B / T` | Core equivalents |
| CPU cores | Count of recorded distinct nonnegative `cpu_id` at the timestamp, host scope | Logical CPUs |
| User CPU | `100 × (u+n) / T` | % |
| System CPU | `100 × s / T` | % |
| IRQ | `100 × (q+f) / T` | % |
| I/O wait / Steal / Idle | `100 × w/T`, `100 × z/T`, `100 × i/T` | % |
| Actual frequency | `Σ(actual_frequency_hz × online_cpus) / Σonline_cpus / 10⁶` | MHz, weighted across CPUFreq policies |
| Scaling frequency | Same weighted formula for `scaling_cur_freq_hz` | MHz |
| Procs running / Procs blocked | `/proc/stat.procs_running`, `procs_blocked` | Runnable / I/O-blocked process gauges |
| Context switches | `R(ctxt)` from `/proc/stat` | Switches/s |
| Load 1m / 5m / 15m | `/proc/loadavg`: `load1`, `load5`, `load15` | Kernel load averages, runnable and uninterruptible tasks |
| Runnable tasks / Tasks | `/proc/loadavg`: `running`, `total` | Current scheduler task counts |
| CPU / Memory / I/O PSI, 10s | `/proc/pressure/{cpu,memory,io}`: `some_avg10` | Kernel-reported % of time with at least one task stalled over the 10-second average |
| CPU PSI / I/O PSI interval lane | `100 × Δsome_total / (10⁶ × Δt)` | % of elapsed sample interval |

CPU composition requires all eight nonnegative differences and positive `T`. Used cores also requires positive `N(t)`. CPU usage lane uses recorded clock rate and host CPU count; composition uses the eight-counter total as its denominator. Actual frequency requires every policy's value and online count, a positive total online count, and the same recorded hardware source across policies. The reader prefers `cpuinfo_avg_freq`, then `cpuinfo_cur_freq`; scaling frequency reads `scaling_cur_freq` separately. Sources: [host formulas](../bins/kronika-web/ui/src/system-view.tsx), [USE/timeline lanes](../crates/kronika-query/src/hour/lanes.rs), [CPUFreq](../crates/kronika-source-os/src/cpufreq.rs).

For the CPU usage lane, `N_lane` is the set size of all distinct nonnegative `cpu_id` values encountered across the source segment, accumulated before row scope and timestamp checks. CPU composition and the CPU cores gauge count CPUs at each timestamp. The 10-second PSI chart selects `os_psi` by resource; its request has no additional scope filter. Host/container interval PSI lanes apply their explicit scope selection.

PSI also records `some_avg60`, `some_avg300`, cumulative `some_total`, and memory/I/O `full_avg10/60/300`, `full_total`. `some` counts time at least one task is stalled; `full` counts time all non-idle tasks are stalled. The stored averages are kernel gauges; the interval lanes and [health](metrics-time.md#health) derive from cumulative `some_total`. Source: [PSI reader](../crates/kronika-source-os/src/proc/pressure.rs).

## Host memory

Memory values show how much RAM is used and how much remains available without swapping. Values below come from `/proc/meminfo` in KiB (1,024 bytes) before display conversion.

| Display | Field or formula | Meaning |
|---|---|---|
| In use | `100 × (mem_total − mem_available) / mem_total` | Host memory utilization; positive total required |
| MemTotal / MemAvailable / MemFree | `mem_total`, `mem_available`, `mem_free` | Usable physical memory / kernel estimate available without swapping / unused memory |
| AnonPages | `anon_pages` | Anonymous pages |
| Page cache | `cached + buffers` | File cache and block-device buffers |
| Reclaimable slab / Unreclaimable slab | `s_reclaimable`, `s_unreclaim` | Reclaimable / currently unreclaimable slab |
| Other memory | `mem_total − mem_free − cached − buffers − anon_pages − s_reclaimable − s_unreclaim` | Residual; null if any operand is missing or the result is negative |
| Free swap / Total swap | `swap_free`, `swap_total` | Free / configured swap |
| Swapped pages | `R(pswpin + pswpout)` from `/proc/vmstat` | Pages/s moved in both directions |
| OOM kills | `R(oom_kill)` from `/proc/vmstat` | Kills/s |

`MemAvailable` overlaps reclaimable memory categories and is not an additional component of the memory composition. Sources: [memory parser](../crates/kronika-source-os/src/proc/meminfo.rs), [VM counters](../crates/kronika-source-os/src/proc/vmstat.rs), [composition](../bins/kronika-web/ui/src/system-view.tsx).

## Storage and filesystems

### Device I/O

Device metrics describe the number, size and duration of storage operations. Recorded `major:minor` numbers identify the device, alongside its name. Counters come from `/proc/diskstats`; a sector is 512 bytes. `Δreads` and `Δwrites` are increases in completed operations; `Δread_time_ms` and `Δwrite_time_ms` are increases in cumulative operation time, in milliseconds.

| Device field | Formula | Unit |
|---|---|---|
| Reads / Writes | `R(reads)`, `R(writes)` | Completed operations/s |
| Read / Write bytes | `512 × R(read_sectors)`, `512 × R(write_sectors)` | B/s |
| Read / Write latency | `Δread_time_ms / Δreads`, `Δwrite_time_ms / Δwrites` | ms/operation; null for zero operations |
| Device busy | `100 × Δio_time_ms / (1,000 × Δt)` | % of interval with I/O active |
| Queue depth | `Δio_weighted_time_ms / (1,000 × Δt)` | Average requests active or waiting |
| Active I/O | `io_in_progress` | Current requests |

Host **Device busy** and **Queue depth** charts take the maximum per-device value at each timestamp. Their breakdown lines are individual devices; devices that remain zero throughout the hour are hidden if any device is active. The USE Storage cells instead use `min(100, 100 × R(Σio_time_ms)/1,000)` and `R(Σio_weighted_time_ms)/1,000`. **Active I/O** in the host overview is `Σio_in_progress`; **Block devices** is the number of recorded device rows.

Sources: [diskstats parser](../crates/kronika-source-os/src/proc/diskstats.rs), [`SYSTEM_ENTITIES`, `latencyPoints`, `peakDeviceRate`](../bins/kronika-web/ui/src/system-view.tsx), [`read_disk`, `points`](../crates/kronika-query/src/hour/lanes.rs).

### Mounted filesystems

These values describe storage available through each visible mount. `statvfs` supplies `f_blocks` (total blocks), `f_frsize` (bytes per block), `f_bavail` (blocks available to unprivileged writes), `f_files` (total inode count) and `f_favail` (available inode count). An inode identifies a filesystem object.

| Field | Source/formula | Unit and scope |
|---|---|---|
| Mount point, root, source, type, device | `/proc/self/mountinfo` | Visible mount identity; root is the filesystem subtree exposed by the mount |
| Total bytes | `f_blocks × f_frsize` | Bytes |
| Free bytes / Available | Recorded `free_bytes = f_bavail × f_frsize` | Bytes available to unprivileged writes; reserved free blocks are excluded |
| Available % | `100 × free_bytes / total_bytes` | Positive total required |
| Used bytes in the paired chart | `total_bytes − free_bytes` | Bytes outside the available portion |
| Total / available inodes | `f_files`, `f_favail` | Inode/file-serial counts |
| Available inode % | `100 × available_inodes / total_inodes` | Positive total required |
| Used inodes in the paired chart | `total_inodes − available_inodes` | Inodes outside the available portion |
| Minimum filesystem free | Minimum available-byte percentage across recorded mounts at the timestamp | %; a missing required mount value makes this aggregate unavailable |
| Filesystems | Number of recorded mount rows | Count |
| Kubernetes infrastructure | `is_k8s_infra` from recorded mount path | Classifies known infrastructure bind mounts |

The reader excludes pseudo filesystems and mounts inside `/proc` or `/sys`; data-bearing `tmpfs` and `overlay` remain eligible. `statvfs` products saturate at `i64::MAX`. Sources: [filesystem capacity](../crates/kronika-source-os/src/fs.rs), [mount selection](../crates/kronika-source-os/src/mount.rs), [paired charts](../bins/kronika-web/ui/src/system-view.tsx).

### Topology reference

CPU topology lists logical CPU, socket, core, NUMA node, model, and maximum MHz. CPUFreq reference lists policy membership, driver, actual-frequency source, and hardware limits. Storage topology relates `major:minor` devices, partition/stack edges, parent/slave relationships, and visible mounts. These are identity/configuration records selected at the cursor; topology fields do not become history metrics. Sources: [CPU topology](../crates/kronika-source-os/src/proc/cpuinfo.rs), [block topology](../crates/kronika-source-os/src/block_topology.rs), [reference views](../bins/kronika-web/ui/src/system-view.tsx).

## Network

RX means received traffic; TX means transmitted traffic. The totals cover the recorded network interfaces and their network namespace.

| Display | Source/formula | Unit |
|---|---|---|
| Per-interface RX / TX | `R(rx_bytes)`, `R(tx_bytes)` from `/proc/net/dev` | B/s |
| RX / TX packets | `R(rx_packets)`, `R(tx_packets)` | Packets/s |
| RX / TX errors | `R(rx_errs)`, `R(tx_errs)` | Errors/s |
| RX / TX drops | `R(rx_drop)`, `R(tx_drop)` | Packets/s |
| Speed / duplex | Recorded sysfs link speed `speed_mbit`, `duplex` | Mbit/s and duplex setting; reference fields |
| Host/namespace RX / TX | `R(Σrx_bytes)`, `R(Σtx_bytes)` | B/s across recorded interfaces |
| Net errors | `R(Σ(rx_errs + tx_errs))` | Errors/s |
| Drops chart | `R(Σ(rx_drop + tx_drop))` | Drops/s |
| USE Drops | `R(Σ(rx_drop + tx_drop + rx_fifo + tx_fifo))` | Drops/FIFO errors per second |
| Interfaces | Number of recorded interface rows | Count |

The aggregate sums each timestamp's recorded counters before differentiation. Link speed does not normalize the RX/TX values to percentages. Sources: [network parser](../crates/kronika-source-os/src/proc/net_dev.rs), [network aggregate charts](../bins/kronika-web/ui/src/system-view.tsx), [USE network lanes](../crates/kronika-query/src/hour/lanes.rs).

## Container cgroups

### Capacity and membership

`os_cgroup_context` records cgroup version, exact collector CPU/memory/I/O paths, effective cpuset count, tightest CPU quota/period ratio on the applicable hierarchy, and effective memory ceiling. With positive CPU-time quota `Q` and period `P` in microseconds, and usable allowed-CPU count `S` from cpuset, CPU capacity is `min(Q/P, S)`; if `S` is absent, it is `Q/P`. A recorded quota of `−1` selects `S`; unknown quota hierarchy leaves capacity null. Memory capacity is the recorded validated hierarchical ceiling. Positive local controller limits and effective ancestor limits are distinct fields.

Cgroup v2 validates applicable files from configured hierarchy root through exact membership. A missing mount-root control file is accepted as unbounded only for non-root membership; required descendant files must be valid. Cgroup v1 binds an unambiguous controller root; memory uses validated `hierarchical_memory_limit` consistently with the leaf limit. The effective capacity/context is attached only to the table row matching collector path and scope. Sources: [hierarchy reader](../crates/kronika-source-os/src/cgroup.rs), [context contract](../crates/kronika-registry/src/codec/os_cgroup_context.rs), [`cgroup_cpu_capacity`](../crates/kronika-query/src/hour/lanes.rs), [`systemEntityRows`](../bins/kronika-web/ui/src/system-view.tsx).

### Controller metrics

A controller accounts for its resource across the group. In the formulas, `used_cores` is CPU use in cores, `effective_capacity` is the available cores defined above, `current` is the relevant controller’s current value and `max` its local limit. `effective_memory_max` is the memory limit including ancestor restrictions.

| Display or recorded field | Calculation/source | Unit |
|---|---|---|
| CPU used / user / system | `R(usage_usec) / 10⁶`, `R(user_usec) / 10⁶`, `R(system_usec) / 10⁶` | Core equivalents; v2 `cpu.stat`, v1 `cpuacct` |
| Other CPU | `R(usage_usec − user_usec − system_usec) / 10⁶`, calculated from the three differences | Cores; null if a component difference or residual is negative |
| CPU share | `100 × used_cores / effective_capacity` | %; unavailable without capacity |
| CPU quota / period | `quota_usec`, `period_usec`; displayed quota cores `Q/P` when positive | Local controller ceiling; quota `−1` is unlimited |
| Throttled | `100 × R(throttled_usec) / 10⁶` | % of wall interval; no capacity division or 100% cap |
| Throttling events | Recorded cumulative `nr_throttled` | Count; cgroup CPU record |
| CPU / memory / I/O PSI | `100 × R(some_total) / 10⁶` for collector cgroup pressure | % of sample interval |
| Memory current | v2 `memory.current`; v1 `memory.usage_in_bytes` | Bytes |
| Memory share | `100 × current / effective_memory_max` | % of positive hierarchical ceiling |
| Local memory max | v2 `memory.max`; v1 `memory.limit_in_bytes` | Bytes; unlimited represented as null |
| Anon / File / Slab | `anon`, `file`, `slab` from `memory.stat` | Bytes |
| Other kernel | `kernel − slab` | Bytes; null for absent input or negative difference |
| Unclassified memory | `current − anon − file − kernel` | Bytes; null for absent input or negative difference |
| Shared memory, where recorded | `shmem` in the newer memory layout | Bytes included in `file` |
| Memory events | Cumulative `low_events`, `high_events`, `max_events`, `oom_events`, `oom_kill`; v1 `memory.failcnt` maps to `max_events` | Counts; OOM lane is `R(oom_kill)` kills/s |
| I/O read/write | `R(rbytes)`, `R(wbytes)` from v2 `io.stat` or v1 blkio service-byte files | B/s per cgroup and device |
| I/O operations | `R(rios)`, `R(wios)` | Operations/s per cgroup and device |
| Threads (TIDs) / Local pids.max | Direct `pids.current`, `pids.max` | Threads (TIDs) in the cgroup subtree and local subtree limit |
| Of pids.max | `100 × current / max` for positive local max | %; literal `max` records null unlimited limit |

The cgroup I/O lane sums device counters for the collector's exact I/O path before calculating read/write rates. Each I/O counter can remain available independently. The device table records `major:minor`, stacked-device chain, visible mount associations, and folded lower-layer counters in the inspector. These associations retain the recorded cgroup/device scope. The `pids.current` count includes descendants and each process's main thread; a process-row count uses a different unit. Both `pids.current` and `pids.max` must be valid for a threads row to be recorded. Sources: [controller parsing](../crates/kronika-source-os/src/cgroup.rs), [CPU](../crates/kronika-registry/src/codec/os_cgroup_cpu.rs), [memory](../crates/kronika-registry/src/codec/os_cgroup_memory.rs), [I/O](../crates/kronika-registry/src/codec/os_cgroup_io.rs), [Threads](../crates/kronika-registry/src/codec/os_cgroup_pids.rs), [device associations](../bins/kronika-web/ui/src/cgroup-device.ts).

## USE table and verdicts

USE columns are Utilization (U), Saturation (S), and Errors (E). Cells read their lane at or before the cursor. Resource rows map to the following exact values:

| Resource | U | S | E |
|---|---|---|---|
| Host CPU | CPU usage % | CPU PSI interval % | Unavailable |
| Host memory | In use % | Swapped pages/s | OOM kills/s |
| Host storage | Capped summed busy % | Summed average queue | Unavailable |
| Host/namespace network | RX and TX B/s | Drops including FIFO/s | RX + TX errors/s |
| Cgroup CPU | Capacity share %, fallback used cores | Throttled % and CPU PSI % | Unavailable |
| Cgroup memory | Effective-limit share %, fallback current bytes | Memory PSI % | OOM kills/s |
| Cgroup I/O | Read and write B/s | I/O PSI % | Unavailable |
| Cgroup Threads | Local-limit share %, fallback current count | Unavailable | Unavailable |

| Verdict | Exact reduction |
|---|---|
| U | Highest available percentage-valued U cell at the cursor. Byte/count fallbacks and network throughput do not compete. Equal values keep the earlier resource in row order. |
| S | Lists every positive maximum over the selected hour from all available S lanes, including secondary lanes. The linked resource is the one with the greatest percentage maximum; nonpercentage lanes use comparison value `−1`. Available all-zero lanes produce Quiet; no available S lane produces `—`. |
| E | For each error lane, sum `rate(tᵢ) × (tᵢ−tᵢ₋₁)/10⁶` over its sorted points, then sum resources. A null/nonfinite current rate contributes nothing. The result displays a rounded event count. The linked resource has the largest positive integrated count. Available zero totals produce Quiet; no error lane produces `—`. |

Source: [`USE_RESOURCES`, `resolveCell`, `ledgerVerdicts`, `integrateRate`](../bins/kronika-web/ui/src/use-table.tsx).

Primary versus fallback U lanes are selected by whether any finite sample exists in the whole hour. If the primary lane has an hourly sample but is null at the cursor, the cell remains null. A secondary S lane participates only after its primary lane resolves. Resource order is cgroup CPU, memory, I/O, Threads, then host CPU, memory, storage, network; unavailable container rows are omitted. Strict comparisons preserve this order for equal verdict values.

## Fixed marks and cell colors

Linux and overall-health timeline marks use these predicates on the supported recorded layouts:

| Mark | Predicate |
|---|---|
| Host CPU | `100 × B ≥ 80 × T`, with positive `T` and nonnegative eight-counter differences |
| Host load | `load1 ≥ 2 × online_CPU_count` at the same timestamp |
| Host memory | `100 × MemAvailable ≤ 10 × MemTotal`, valid `0 ≤ available ≤ total`, positive total |
| Host filesystem | `100 × (total_bytes − free_bytes) ≥ 90 × total_bytes`, valid bounds, positive total |
| Host / cgroup OOM | Later recorded `oom_kill` exceeds the previous value for its scope/identity |
| Overall health | Known value `< 50`; the overall-health chart also draws the 50 threshold |

Process cell colors are separate: state `R` good, `D` warning, `Z` critical, `I` inactive; Tree CPU is warning at `≥50%` and critical at `≥90%`; a zero rate is inactive. Sources: [fixed predicates](../crates/kronika-index/src/detect/direct.rs), [cell tones](../bins/kronika-web/ui/src/value-tone.ts), [health chart threshold](../bins/kronika-web/ui/src/timeline.tsx).
