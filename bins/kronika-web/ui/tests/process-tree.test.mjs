import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const tree = await importModule('export { buildProcessForest, filterProcessForest } from "../src/process-tree.ts"')

const HOUR = 1_780_000_000_000_000
const CLOCK_HZ = 100

let ordinal = 0
function row(pid, ppid, extra = {}) {
  ordinal += 1
  return {
    logicalName: "os_process", ordinal: String(ordinal), segmentId: "a",
    timestamp: HOUR, typeId: "1100001",
    values: { pid, ppid, comm: "worker", ...extra },
  }
}

function pids(forest) { return forest.map((r) => r.values.pid) }

test("a simple parent/child chain walks the parent first, each child directly under it", () => {
  const rows = [row(3, 2), row(1, 0), row(2, 1)]
  const forest = tree.buildProcessForest(rows, null, CLOCK_HZ)
  assert.deepEqual(pids(forest), [1, 2, 3])
  assert.equal(forest[0].values.process_tree_prefix, "")
  assert.equal(forest[0].values.process_tree_depth, 0)
  assert.equal(forest[0].values.process_tree_parent_pid, null)
  assert.equal(forest[1].values.process_tree_prefix, " \\_ ")
  assert.equal(forest[1].values.process_tree_parent_pid, 1)
  assert.equal(forest[2].values.process_tree_prefix, "     \\_ ")
  assert.equal(forest[2].values.process_tree_depth, 2)
})

test("siblings hang off the same parent at the same depth, the way ps draws them", () => {
  const rows = [row(1, 0), row(2, 1), row(3, 1)]
  const forest = tree.buildProcessForest(rows, null, CLOCK_HZ)
  assert.deepEqual(pids(forest), [1, 2, 3])
  assert.equal(forest[1].values.process_tree_prefix, " \\_ ")
  assert.equal(forest[2].values.process_tree_prefix, " \\_ ")
})

test("a continuing ancestor branch draws a vertical bar, not blank indentation", () => {
  const rows = [row(1, 0), row(2, 1), row(3, 1), row(4, 2)]
  const forest = tree.buildProcessForest(rows, null, CLOCK_HZ)
  assert.deepEqual(pids(forest), [1, 2, 4, 3])
  assert.equal(forest[2].values.process_tree_prefix, " |   \\_ ")
})

test("two unrelated process trees (different sessions/services) stay separate and ordered by root pid", () => {
  const rows = [row(50, 10), row(10, 0), row(1, 0), row(2, 1)]
  const forest = tree.buildProcessForest(rows, null, CLOCK_HZ)
  assert.deepEqual(pids(forest), [1, 2, 10, 50])
})

test("a pid whose recorded parent is not in this snapshot renders as its own root, not dropped", () => {
  const rows = [row(2, 1)]
  const forest = tree.buildProcessForest(rows, null, CLOCK_HZ)
  assert.deepEqual(pids(forest), [2])
  assert.equal(forest[0].values.process_tree_prefix, "")
})

test("a two-process cycle still renders both processes, rooted at the lower pid, without looping", () => {
  const rows = [row(20, 10), row(10, 20)]
  const forest = tree.buildProcessForest(rows, null, CLOCK_HZ)
  assert.deepEqual(pids(forest).slice().sort((left, right) => left - right), [10, 20])
  assert.equal(forest.length, 2)
})

test("%CPU is the recorded jiffies per second against one core, the way top counts it", () => {
  // 40 + 10 jiffies per second against a 100 Hz clock is half a core.
  const [busy] = tree.buildProcessForest([row(1, 0, { utime: 40, stime: 10, rmem_kb: 512_000 })], 2_048_000, 100)
  assert.equal(busy.values.cpu_percent, 50)
  assert.equal(busy.values.mem_percent, 25)
})

test("%CPU is missing rather than guessed when a half or the clock rate is missing", () => {
  const at = (values, ticksPerSecond = 100) => tree.buildProcessForest([row(1, 0, values)], null, ticksPerSecond)[0].values.cpu_percent
  assert.equal(at({ utime: 0, stime: 0 }), 0)
  assert.equal(at({ utime: 40 }), null)
  assert.equal(at({ utime: null, stime: 10 }), null)
  assert.equal(at({ utime: 40, stime: 10 }, null), null)
  assert.equal(at({ utime: 40, stime: 10 }, 0), null)
})

test("STAT carries the state letter and the flags the record can tell", () => {
  const stat = (values) => tree.buildProcessForest([row(1, 0, values)], null, CLOCK_HZ)[0].values.process_stat
  const sleeping = "S".charCodeAt(0)
  assert.equal(stat({ state: sleeping, nice: 0, num_threads: 1 }), "S")
  assert.equal(stat({ state: sleeping, nice: -5, num_threads: 1 }), "S<")
  assert.equal(stat({ state: sleeping, nice: 10, num_threads: 1 }), "SN")
  assert.equal(stat({ state: sleeping, nice: 0, num_threads: 20 }), "Sl")
  assert.equal(stat({ state: sleeping, nice: -5, num_threads: 20 }), "S<l")
  assert.equal(stat({ state: "R".charCodeAt(0) }), "R")
  assert.equal(stat({ nice: -5 }), null)
})

test("TIME is the recorded lifetime CPU counter against the clock rate", () => {
  const [row1] = tree.buildProcessForest([row(1, 0, { cpu_time_ticks: 27_618 })], null, CLOCK_HZ)
  assert.equal(row1.values.cpu_time_seconds, 276.18)
  const seconds = (values, ticksPerSecond = CLOCK_HZ) => tree.buildProcessForest([row(1, 0, values)], null, ticksPerSecond)[0].values.cpu_time_seconds
  assert.equal(seconds({ cpu_time_ticks: 0 }), 0)
  assert.equal(seconds({ utime: 40, stime: 10 }), null)
  assert.equal(seconds({ cpu_time_ticks: 27_618 }, null), null)
  assert.equal(seconds({ cpu_time_ticks: 27_618 }, 0), null)
})

test("Tree search keeps matched rows with their parent chain", () => {
  const forest = tree.buildProcessForest([
    row(1, 0, { cmdline: "/sbin/init", rmem_kb: 512, user: "root" }),
    row(2, 1, { cmdline: "postgres", rmem_kb: 4_096, user: "postgres" }),
    row(3, 2, { cmdline: "autovacuum worker", rmem_kb: 1_024, user: "postgres" }),
    row(4, 1, { cmdline: "nginx", rmem_kb: 768, user: "www-data" }),
  ], null, CLOCK_HZ)
  assert.deepEqual(pids(tree.filterProcessForest(forest, "autovacuum", 100)), [1, 2, 3])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "user:postgres", 100)), [1, 2, 3])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "rss>2MiB", 100)), [1, 2])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "pid:4 OR rss>2MiB", 100)), [1, 2, 4])
})

test("Tree numeric search uses API rate units", () => {
  const forest = tree.buildProcessForest([
    row(1, 0, { blkdelay_ticks: 25, read_bytes: 2_000_000, rundelay_ns: 40_000_000, stime: 10, utime: 20 }),
    row(2, 1, { blkdelay_ticks: 1, read_bytes: 100, rundelay_ns: 1_000, stime: 1, utime: 1 }),
  ], null, CLOCK_HZ)
  assert.deepEqual(pids(tree.filterProcessForest(forest, "cpu_cores>0.2 AND disk_read_rate>1MB/s", 100)), [1])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "run_delay>30ms/s", 100)), [1])
  assert.deepEqual(pids(tree.filterProcessForest(forest, "block_io_delay>200ms/s", 100)), [1])
})
