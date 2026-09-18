# Kronika development demo

[Русская версия](README.ru.md)

Build this demo from source to explore the interface or develop Kronika.
Docker Compose starts PostgreSQL 15, PgBouncer, collector, web and synthetic
workloads in one service. The `kronika-demo` program is not included in prebuilt
archives; those contain collector, web, dump and report.

## Start and inspect

Requirements: Make and Docker with Compose v2 on an amd64 or arm64 Linux host.
Run the commands below from the repository root.

```sh
make demo-up
```

The command builds the image, starts the service, waits for its health check,
and prints the URL. Open <http://127.0.0.1:8080/> and sign in with:

```text
Username: demo
Password: forensics
```

Navigation and metric definitions: [controls](../../docs/features.md),
[Linux](../../docs/metrics-linux.md), [PostgreSQL](../../docs/metrics-postgresql.md).
The interface marks this recording as `DEMO · synthetic data`.

To set another loopback port:

```sh
DEMO_PORT=18081 make demo-up
```

Inspect health and follow service logs with:

```sh
make demo-status
make demo-logs
```

## Stop and remove data

Stop the container while preserving collected Kronika history:

```sh
make demo-stop
```

Run `make demo-up` to start it again. PostgreSQL and PgBouncer use ephemeral
tmpfs filesystems and are recreated on every container start. The named volume
contains Kronika history and, while the demo is running, one bounded system
workload scratch file. A clean stop removes that file. The history retention target is 512 MiB.

Remove the container, network, and named demo-data volume:

```sh
make demo-clean
```

The next `make demo-up` creates a clean demo. Image construction downloads the
pinned base images, locked Cargo dependencies, and the exact pg_store_plans
source revision. Normal runtime does not require network access beyond the
browser connecting to the published loopback port.

<a id="kronika-demo-binary"></a>
## The `kronika-demo` program

The program runs `kronika-collector` for a configured duration and reports
compressed recording size, current journal size, peak process memory (RSS),
and CPU time. The image uses it as the supervisor
for the collector, the default system workload, and the optional PostgreSQL
workload.

Use `kronika-demo --help` for the generated option reference. Every setting below
has a CLI option; explicit options override environment variables. Configuration
is validated before any workload or collector starts.

```sh
kronika-demo --duration-s 1m --dir demo-data --system-workload-enabled false
```

Duration values accept integers in the option's named unit or a suffix such as
`1m`, `5s`, or `4000ms`. They must resolve to whole seconds or milliseconds as
specified by the option. Memory and scratch sizes accept integers in MiB or an
explicit byte size such as `32MiB`; disk and network rates similarly accept
integers in KiB/s or byte sizes per second such as `32KiB`. Sizes must be exact
multiples of the named unit. Existing defaults and resource limits still apply.

| Option / environment variable | Default | Meaning |
| --- | ---: | --- |
| `--dir` / `KRONIKA_DEMO_DIR` | `demo-data` | Where the collector log and `report.json` are written. |
| `--storage-dir` / `KRONIKA_STORAGE_DIR` | `$KRONIKA_DEMO_DIR/segments` | Collector storage directory. |
| `--duration-s` / `KRONIKA_DEMO_DURATION_S` | 60 | Run duration in seconds. `0` runs until `SIGTERM` or `SIGINT`. |
| `--collector-log` / `KRONIKA_DEMO_COLLECTOR_LOG` | `file` | `file` writes `collector.log`; `stderr` uses inherited stderr. The image uses `stderr` with bounded Docker log rotation. |
| `--collector-bin` / `KRONIKA_COLLECTOR_BIN` | `kronika-collector` beside this binary | Collector binary to run. |

Other `KRONIKA_*` collector variables pass through unchanged.

### Bounded system workload

This workload is enabled by default and does not depend on
`KRONIKA_DEMO_WORKLOAD_DSN`. The Compose entrypoint enables it explicitly, so
it continues to run when the PostgreSQL workload is disabled. Invalid or blank
controls stop `kronika-demo` at startup and name the offending variable. When
the workload is disabled, its other controls are ignored.

| Option / environment variable | Default | Accepted values |
| --- | ---: | --- |
| `--system-workload-enabled` / `KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED` | `true` | Exactly `true` or `false`. |
| `--system-workload-dir` / `KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR` | `$KRONIKA_DEMO_DIR/system-activity` | A non-blank directory separate from `KRONIKA_STORAGE_DIR`. Compose uses `/var/lib/kronika/data/system-activity`. |
| `--system-cpu-percent` / `KRONIKA_DEMO_SYSTEM_CPU_PERCENT` | 12 | Peak percent of one CPU core, 1–25. |
| `--system-memory-mib` / `KRONIKA_DEMO_SYSTEM_MEMORY_MIB` | 32 | Anonymous working set, 8–128 MiB. |
| `--system-file-mib` / `KRONIKA_DEMO_SYSTEM_FILE_MIB` | 8 | Fixed scratch-file size, 1–32 MiB. |
| `--system-disk-kib-per-s` / `KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S` | 32 | Peak payload in each of the read and write directions, 1–256 KiB/s. |
| `--system-network-kib-per-s` / `KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S` | 32 | Peak one-way loopback payload, 1–256 KiB/s. |
| `--system-flush-interval-s` / `KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S` | 5 | Per-file flush interval, 1–10 seconds. Peak disk rate times this interval must fit in the scratch file. |

Four named threads run inside `kronika-demo`: `krn-demo-cpu`,
`krn-demo-memory`, `krn-demo-disk`, and `krn-demo-loop`. CPU work is bounded by
100 ms frames; the memory thread owns one fixed allocation and touches every
operating-system page once a second. The loopback worker joins two UDP sockets
bound to `127.0.0.1` on ephemeral ports. It opens no external route or service
port.

CPU, disk, and network follow a fixed 60-second waveform with six 10-second
phases at 25%, 50%, 75%, 100%, 75%, and 50% of the configured peak. With the
defaults, CPU therefore moves through 3%, 6%, 9%, 12%, 9%, and 6% of one core;
disk and loopback payload move through 8, 16, 24, 32, 24, and 16 KiB/s. For ideal scheduling, the mean phase weight is
`(0.25 + 0.50 + 0.75 + 1 + 0.75 + 0.50) / 6 = 0.625` of the peak:

- CPU: 270 CPU-seconds per hour, or 7.5% of one core on average (3.75% of the
  two-core Compose allowance).
- Memory: 32 MiB of touched anonymous memory. The 8 MiB file is the hard limit
  for its scratch data and cache pages.
- Disk: 73,728,000 bytes (70.3125 MiB) per hour in writes and the same read
  payload, at most 140.625 MiB combined. Filesystem metadata can add a small
  amount. Delayed worker scheduling can only lower the payload.
- Loopback: 73,728,000 bytes (70.3125 MiB) per hour of payload appears in each
  namespace RX and TX counter, 140.625 MiB combined, plus UDP/IP headers.

The disk worker owns exactly
`kronika-demo-system-activity.bin`. It sets the length once and overwrites pages
as a ring; it never appends. Each cadence uses `sync_data` on that file, asks
the kernel to discard only the flushed pages, and reads those pages back so
both physical read and write counters can advance. It never calls global
`sync`. A stale regular file with that exact name is replaced at startup;
symlinks and non-files are refused. A clean stop joins all workers and removes
only that file. An individual worker error is logged and leaves the collector,
PostgreSQL workload, and other system workers running.

#### 75-second system smoke

This isolates `kronika-demo` from PostgreSQL and external networking. It checks
two samples of the demo PID, the loopback interface, and the fixed scratch
file, then checks clean removal.

```bash
set -eu
make demo-image
smoke_dir=$(mktemp -d /var/tmp/kronika-system-smoke.XXXXXX)
smoke_name="kronika-system-smoke-$$"
cleanup() { docker rm -f "$smoke_name" >/dev/null 2>&1 || true; }
trap cleanup EXIT

cid=$(docker run --detach \
  --name "$smoke_name" \
  --no-healthcheck \
  --network none \
  --read-only \
  --cpus 2 \
  --memory 1g \
  --pids-limit 512 \
  --tmpfs /tmp:rw,nosuid,nodev,noexec,size=64m \
  --mount type=bind,src="$smoke_dir",dst=/data \
  -e KRONIKA_DEMO_DIR=/data \
  -e KRONIKA_STORAGE_DIR=/data/segments \
  -e KRONIKA_DEMO_DURATION_S=0 \
  -e KRONIKA_DEMO_COLLECTOR_LOG=stderr \
  -e KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED=true \
  -e KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR=/data/system-activity \
  --entrypoint /usr/local/bin/kronika-demo \
  kronika-demo:local)

sample() {
  docker exec "$cid" sh -ceu '
    test "$(cat /proc/1/comm)" = kronika-demo
    set -- $(cat /proc/1/stat)
    cpu=$(( ${14} + ${15} ))
    rss=$(awk "/^VmRSS:/ { print \$2 }" /proc/1/status)
    anon=$(awk "/^RssAnon:/ { print \$2 }" /proc/1/status)
    threads=$(awk "/^Threads:/ { print \$2 }" /proc/1/status)
    rb=$(awk "/^read_bytes:/ { print \$2 }" /proc/1/io)
    wb=$(awk "/^write_bytes:/ { print \$2 }" /proc/1/io)
    rx=$(cat /sys/class/net/lo/statistics/rx_bytes)
    tx=$(cat /sys/class/net/lo/statistics/tx_bytes)
    size=$(stat -c %s /data/system-activity/kronika-demo-system-activity.bin)
    printf "%s %s %s %s %s %s %s %s %s\n" \
      "$cpu" "$rss" "$anon" "$rb" "$wb" "$threads" "$rx" "$tx" "$size"
  '
}

sleep 10
read -r cpu0 rss0 anon0 rb0 wb0 threads0 rx0 tx0 size0 <<EOF
$(sample)
EOF
sleep 65
read -r cpu1 rss1 anon1 rb1 wb1 threads1 rx1 tx1 size1 <<EOF
$(sample)
EOF

test "$cpu1" -gt "$cpu0"
test "$rss1" -gt 0
test "$anon1" -ge 30000
test "$rb1" -gt "$rb0"
test "$wb1" -gt "$wb0"
test "$threads1" -gt 1
test "$rx1" -gt "$rx0"
test "$tx1" -gt "$tx0"
test "$size0" -eq 8388608
test "$size1" -eq "$size0"

docker exec "$cid" sh -c 'for f in /proc/1/task/*/comm; do cat "$f"; done'
docker exec "$cid" sh -c 'test ! -r /sys/fs/cgroup/io.stat || cat /sys/fs/cgroup/io.stat'
docker stop --time 50 "$cid" >/dev/null
test "$(docker inspect --format '{{.State.ExitCode}}' "$cid")" -eq 0
test ! -e "$smoke_dir/system-activity/kronika-demo-system-activity.bin"
docker logs "$cid"
docker rm "$cid" >/dev/null
trap - EXIT
printf 'Smoke data retained at %s\n' "$smoke_dir"
```

### Optional PostgreSQL workload

`KRONIKA_DEMO_WORKLOAD_DSN` enables the PostgreSQL workload. If unset, the
PostgreSQL workload is disabled and its other controls are ignored; the system
workload remains enabled by default.

| Option / environment variable | Default | Meaning |
| --- | ---: | --- |
| `--workload-dsn` / `KRONIKA_DEMO_WORKLOAD_DSN` | unset | Workload connection, normally through PgBouncer. |
| `--workload-direct-dsn` / `KRONIKA_DEMO_WORKLOAD_DIRECT_DSN` | required with workload | Direct PostgreSQL connection for the plan-change workload and session-scoped Vacuum settings. It must not point at transaction-pooled PgBouncer. The image sets this to its embedded PostgreSQL. |
| `--workload-schemas` / `KRONIKA_DEMO_WORKLOAD_SCHEMAS` | 1 | Commerce schemas to create, from 1 through 8. |
| `--workload-tables-per-schema` / `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` | 8 | Tables per schema, from the 8 commerce tables through 64 total tables. |
| `--workload-ddl-concurrency` / `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` | 4 | Concurrent setup connections, from 1 through 16. |
| `--workload-sessions` / `KRONIKA_DEMO_WORKLOAD_SESSIONS` | 4 | Long-lived OLTP clients, from 1 through 16. |
| `--workload-tps` / `KRONIKA_DEMO_WORKLOAD_TPS` | 20 | Maximum aggregate OLTP transactions per second, from 1 through 64. |
| `--workload-max-orders` / `KRONIKA_DEMO_WORKLOAD_MAX_ORDERS` | 10000 | Reusable live OLTP order slots, from the client count through 50000. |
| `--workload-lock-chains` / `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` | 1 | Independent lock chains in each bounded round, from 1 through 4. |
| `--workload-lock-chain-depth` / `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` | 4 | Transactions in each lock chain, from 2 through 8. Together with the hold time, this must let an earlier waiter acquire the row and a later waiter reach the fixed 10-second statement timeout. |
| `--workload-lock-hold-ms` / `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` | 4000 | Lock hold time per link in a lock round, milliseconds. |
| `--workload-lock-round-interval-s` / `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` | 120 | Quiet pause after each lock round, seconds. |
| `--workload-event-round-interval-s` / `KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S` | 180 | Quiet pause after one slow query, one bad statement, and one bad-database attempt. |
| `--workload-plan-rows` / `KRONIKA_DEMO_WORKLOAD_PLAN_ROWS` | 300000 | Rows maintained in `shop.orders` for the plan-change workload, from 1 through 500000. |
| `--workload-plan-workers` / `KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS` | 4 | Concurrent `checkout-api` sessions exercising the same query, from 1 through 8. |
| `--workload-plan-baseline-s` / `KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S` | 12 | Indexed baseline and recovery window, seconds. |
| `--workload-plan-regression-s` / `KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S` | 30 | Window without the supporting checkout index, seconds. |
| `--workload-plan-round-interval-s` / `KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S` | 120 | Quiet pause after a complete plan-change round, seconds. |
| `--workload-vacuum-rows` / `KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS` | 100000 | Rows in the dedicated Vacuum workload table, from 1 through 250000. |
| `--workload-vacuum-round-interval-s` / `KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S` | 180 | Quiet pause after each Vacuum episode, seconds. |
| `--workload-vacuum-statement-timeout-s` / `KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S` | 30 | Finite timeout for each update and Vacuum statement, seconds. |

The PostgreSQL workload uses two Tokio runtime threads.
The default steady workload uses four long-lived `shop-oltp-*` clients through
the PgBouncer connection and runs at most 20 short transactions per second in
total. Each transaction reads one customer and product through their keys,
locks and changes one inventory row, and writes a related order, item, payment,
event, and application session. A client reuses only its own order slots. The
order, item, payment, and linked-event tables therefore retain at most 10000
live OLTP rows each while continuing to produce WAL, buffer, table, and index
activity. A slow transaction reduces the achieved rate; the client does not
replay missed work in a burst.

Reference data contains 20,000 customers and 2,048 products. OLTP order slots
occupy IDs above the plan and Vacuum fixtures.

| Fixture | Operation and start time |
| --- | --- |
| Plans | Repeats the same checkout query on `shop.orders` before, during and after removal/restoration of its supporting index. |
| Locks | Row-lock chain starts after 65 seconds. |
| Vacuum | Maintenance episode starts after 95 seconds. |
| Events | Explicit slow-query/error/connection events start after 140 seconds. |

The image collects PostgreSQL every 5 seconds. Workload statements and
transactions have finite timeouts. Source: [workload](src/workload).

For a native run with the defaults, use `make demo-run`. To configure an
existing PostgreSQL and PgBouncer workload, build the binaries and pass options
to `kronika-demo` as shown below. The [source build guide](../../docs/build.md)
lists build dependencies. Replace the example connections with your workload
connections:

```sh
make collector demo
kronika_target=$(rustc +1.96.0 -vV | sed -n 's/^host: //p')
"target/$kronika_target/debug/kronika-demo" \
    --duration-s 1m \
    --workload-dsn 'host=127.0.0.1 port=6432 user=kronika_demo dbname=kronika_demo' \
    --workload-direct-dsn 'host=127.0.0.1 port=5432 user=kronika_demo dbname=kronika_demo'
```

`SIGTERM` and `SIGINT` stop the workload and collector, retain the collector journal,
and write the final report before exit.

Sources: [run orchestration](src/run.rs), [collector lifecycle](src/collector.rs), [configuration](src/config.rs), [system workload](src/system_activity), [Compose](../../compose.demo.yml).
