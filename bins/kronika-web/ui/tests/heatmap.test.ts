import assert from "node:assert/strict"
import test from "node:test"

import { HEATMAP_STEPS, collapseHeatmapView, heatmapIntensity, heatmapViewMax } from "../src/heatmap.ts"

const HOUR = 7_200_000_000_000

test("intensity uses square-root steps with a distinct zero", () => {
  assert.equal(heatmapIntensity(0, 100), 0)
  assert.equal(heatmapIntensity(100, 100), HEATMAP_STEPS)
  assert.equal(heatmapIntensity(25, 100), HEATMAP_STEPS / 2)
  assert.equal(heatmapIntensity(0.0001, 100), 1)
})

test("collapsing a view folds the tail rows into the others band", () => {
  const view = {
    cumulative: true,
    summary: "sum",
    intervals: [{ start: HOUR, end: HOUR + 3_600_000_000 - 1 }],
    rows: [
      { typeId: "1", identity: ["a"], labels: {}, total: 100, cells: [2] },
      { typeId: "1", identity: ["b"], labels: {}, total: 50, cells: [1] },
      { typeId: "1", identity: ["c"], labels: {}, total: 25, cells: [null] },
    ],
    totals: { total: 200, cells: [4] },
    others: { total: 25, cells: [1] },
    othersCount: 1,
    entityCount: 4,
  }
  const collapsed = collapseHeatmapView(view, 1)
  assert.equal(heatmapViewMax(view), 2)
  assert.equal(collapsed.rows.length, 1)
  assert.equal(collapsed.othersCount, 3)
  assert.equal(collapsed.others.total, 100)
  assert.deepEqual(collapsed.others.cells, [2])
  assert.equal(collapseHeatmapView(view, 3), view)
})

test("compact RSS adds the hidden means using their shared snapshot denominator", () => {
  const view = {
    cumulative: false,
    summary: "mean",
    intervals: [],
    rows: [
      { typeId: "1", identity: ["first"], labels: {}, total: 600, cells: [800, 400] },
      { typeId: "1", identity: ["second"], labels: {}, total: 300, cells: [600, 0] },
      { typeId: "1", identity: ["third"], labels: {}, total: 100, cells: [0, 200] },
    ],
    totals: { total: 1050, cells: [1500, 600] },
    others: { total: 50, cells: [100, 0] },
    othersCount: 1,
    entityCount: 4,
  }
  const collapsed = collapseHeatmapView(view, 1)
  assert.equal(collapsed.others.total, 450)
  assert.deepEqual(collapsed.others.cells, [700, 200])
  assert.equal(collapsed.rows[0].total + collapsed.others.total, view.totals.total)
  assert.equal(collapseHeatmapView({ ...view, summary: "max" }, 1).others.total, 300)
  assert.equal(collapseHeatmapView({ ...view, rows: view.rows.map((row) => ({ ...row, total: null })), others: { cells: [null, null], total: null } }, 1).others.total, null)
  assert.equal(collapseHeatmapView({ ...view, rows: view.rows.map((row) => ({ ...row, total: 0 })), others: { cells: [0, 0], total: 0 } }, 1).others.total, 0)
})

test("cut scales fall back to raw counts without scale metadata", async () => {
  const { cutScale } = await import("../src/activity-cuts.ts")
  assert.deepEqual(cutScale({ id: "x", fields: ["f"], kind: "bytes", scaleBy: "block_size" }, { blockSize: 8192, clockTicks: null }), { scale: 8192, kind: "bytes" })
  assert.deepEqual(cutScale({ id: "x", fields: ["f"], kind: "bytes", scaleBy: "block_size" }, { blockSize: null, clockTicks: null }), { scale: 1, kind: "count" })
  assert.deepEqual(cutScale({ id: "x", fields: ["f"], kind: "seconds", scaleBy: "clock_ticks" }, { blockSize: null, clockTicks: 100 }), { scale: 0.01, kind: "seconds" })
  assert.deepEqual(cutScale({ id: "x", fields: ["f"], kind: "seconds", scaleBy: "clock_ticks" }, { blockSize: null, clockTicks: null }), { scale: 1, kind: "count" })
  assert.deepEqual(cutScale({ id: "x", fields: ["f"], kind: "bytes", scaleBy: "kib" }, { blockSize: null, clockTicks: null }), { scale: 1024, kind: "bytes" })
  assert.deepEqual(cutScale({ id: "x", fields: ["f"], kind: "count" }, { blockSize: null, clockTicks: null }), { scale: 1, kind: "count" })
})
