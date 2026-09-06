import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const locks = await importModule('export { buildLockForest, filterLockForest } from "../src/postgres-locks.ts"')

const HOUR = 1_780_000_000_000_000

let ordinal = 0
function row(pid, blockedBy, extra = {}) {
  ordinal += 1
  return {
    logicalName: "pg_locks", ordinal: String(ordinal), segmentId: "a",
    timestamp: HOUR, typeId: "1011002",
    values: { pid, blocked_by: blockedBy, datname: "kronika_demo", ...extra },
  }
}

function pids(forest) { return forest.map((r) => r.values.pid) }
function depths(forest) { return forest.map((r) => r.values.lock_tree_depth) }

test("a simple chain walks root first, each waiter directly under its blocker", () => {
  const rows = [row(77, [76]), row(70, []), row(76, [70])]
  const forest = locks.buildLockForest(rows)
  assert.deepEqual(pids(forest), [70, 76, 77])
  assert.deepEqual(depths(forest), [1, 2, 3])
  assert.equal(forest[0].values.lock_tree_prefix, "")
  assert.equal(forest[0].values.lock_tree_parent_pid, null)
  assert.equal(forest[1].values.lock_tree_prefix, "└─ ")
  assert.equal(forest[1].values.lock_tree_parent_pid, 70)
  assert.equal(forest[2].values.lock_tree_prefix, "   └─ ")
})

test("two independent chains stay separate and ordered by their own root, never interleaved", () => {
  const rows = [
    row(48524, [49574]), row(70, []), row(49574, [47475]),
    row(76, [70]), row(47475, []), row(77, [76]),
  ]
  const forest = locks.buildLockForest(rows)
  assert.deepEqual(pids(forest), [70, 76, 77, 47475, 49574, 48524])
})

test("two waiters on the same blocker render as siblings, only the last gets the closing connector", () => {
  const rows = [row(1, []), row(2, [1]), row(3, [1])]
  const forest = locks.buildLockForest(rows)
  assert.deepEqual(pids(forest), [1, 2, 3])
  assert.equal(forest[1].values.lock_tree_prefix, "├─ ")
  assert.equal(forest[2].values.lock_tree_prefix, "└─ ")
})

test("a continuing ancestor branch draws a vertical bar, not blank indentation", () => {
  const rows = [row(1, []), row(2, [1]), row(3, [1]), row(4, [2])]
  const forest = locks.buildLockForest(rows)
  assert.deepEqual(pids(forest), [1, 2, 4, 3])
  assert.equal(forest[1].values.lock_tree_prefix, "├─ ")
  assert.equal(forest[2].values.lock_tree_prefix, "│  └─ ")
  assert.equal(forest[3].values.lock_tree_prefix, "└─ ")
})

test("a row blocked by two backends is placed once, under the first root walked, and notes the rest as extra", () => {
  const rows = [row(10, []), row(20, []), row(30, [10, 20])]
  const forest = locks.buildLockForest(rows)
  assert.deepEqual(pids(forest), [10, 30, 20])
  assert.deepEqual(forest[1].values.lock_tree_extra_blockers, [20])
  assert.deepEqual(forest[0].values.lock_tree_extra_blockers, [])
})

test("a 0 blocker marks a prepared-transaction wait without becoming a tree edge or a phantom row", () => {
  const rows = [row(10, []), row(11, [10, 0])]
  const forest = locks.buildLockForest(rows)
  assert.equal(forest.length, 2)
  assert.equal(forest[1].values.pid, 11)
  assert.equal(forest[1].values.lock_tree_depth, 2)
  assert.equal(forest[1].values.lock_tree_waits_on_prepared, true)
  assert.deepEqual(forest[1].values.lock_tree_extra_blockers, [])
  assert.equal(forest[0].values.lock_tree_waits_on_prepared, false)
})

test("a caught cycle with no zero-blocker member still renders, rooted at its lowest pid, without looping", () => {
  const rows = [row(20, [10]), row(10, [20])]
  const forest = locks.buildLockForest(rows)
  assert.deepEqual(pids(forest).slice().sort((left, right) => left - right), [10, 20])
  assert.equal(forest.length, 2)
})

test("rows for an unrelated pid referenced as a blocker but never recorded are ignored, not crashed on", () => {
  const rows = [row(5, [999])]
  const forest = locks.buildLockForest(rows)
  assert.deepEqual(pids(forest), [5])
  assert.equal(forest[0].values.lock_tree_depth, 1)
})

test("lock search keeps the complete blocker path to each matched waiter", () => {
  const forest = locks.buildLockForest([
    row(70, [], { query: "root" }),
    row(76, [70], { query: "middle" }),
    row(77, [76], { query: "target" }),
    row(90, [], { query: "other" }),
  ])
  assert.deepEqual(pids(locks.filterLockForest(forest, "target")), [70, 76, 77])
  assert.deepEqual(pids(locks.filterLockForest(forest, "pid:77")), [70, 76, 77])

  const multiple = locks.buildLockForest([row(10, []), row(20, []), row(30, [10, 20], { query: "target" })])
  assert.deepEqual(pids(locks.filterLockForest(multiple, "target")), [10, 30, 20])
})
