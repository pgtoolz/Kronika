import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const vitals = await importModule('export { anchoredSeries, counterGroups, countSeries, gaugeSeries, lastValue, peakPoint, settingAt, settingChanges, shareOfTotals, sumCounterVital } from "../src/postgres-vitals.ts"', { plugins: [registryPlugin([])] })

const T0 = 1_780_000_000_000_000
const SECOND = 1_000_000

function row(ordinal, timestamp, values, typeId = "1005004") {
  return { segmentId: "7", logicalName: "pg_stat_database", typeId, ordinal: String(ordinal), timestamp, values }
}

test("counters sum across databases into one instance rate and hour total", () => {
  const rows = [
    row(0, T0, { datid: 1, xact_commit: 100 }),
    row(1, T0, { datid: 2, xact_commit: 50 }),
    row(2, T0 + 10 * SECOND, { datid: 1, xact_commit: 200 }),
    row(3, T0 + 10 * SECOND, { datid: 2, xact_commit: 80 }),
    row(4, T0 + 20 * SECOND, { datid: 1, xact_commit: 260 }),
  ]
  const vital = vitals.sumCounterVital(vitals.counterGroups(rows), ["xact_commit"])
  assert.equal(vital.points.length, 2)
  assert.equal(vital.points[0].timestamp, T0 + 10 * SECOND)
  assert.equal(vital.points[0].value, (100 + 30) / 10)
  assert.equal(vital.points[1].value, 60 / 10)
  assert.equal(vital.total, 100 + 30 + 60)
})

test("a multi-field counter sums its fields", () => {
  const rows = [
    row(0, T0, { datid: 1, xact_commit: 10, xact_rollback: 1 }),
    row(1, T0 + 10 * SECOND, { datid: 1, xact_commit: 30, xact_rollback: 5 }),
  ]
  const vital = vitals.sumCounterVital(vitals.counterGroups(rows), ["xact_commit", "xact_rollback"])
  assert.equal(vital.total, 24)
  assert.equal(vital.points[0].value, 2.4)
})

test("a counter that goes backwards yields no delta for that pair", () => {
  const rows = [
    row(0, T0, { datid: 1, deadlocks: 50 }),
    row(1, T0 + 10 * SECOND, { datid: 1, deadlocks: 2 }),
    row(2, T0 + 20 * SECOND, { datid: 1, deadlocks: 4 }),
  ]
  const vital = vitals.sumCounterVital(vitals.counterGroups(rows), ["deadlocks"])
  assert.equal(vital.points.length, 1)
  assert.equal(vital.total, 2)
})

test("gauge series reduce each snapshot by sum or maximum", () => {
  const rows = [
    row(0, T0, { datid: 1, frozen_xid_age: 100 }),
    row(1, T0, { datid: 2, frozen_xid_age: 900 }),
    row(2, T0 + 10 * SECOND, { datid: 1, frozen_xid_age: 120 }),
  ]
  assert.deepEqual(vitals.gaugeSeries(rows, "frozen_xid_age", "max").map(({ value }) => value), [900, 120])
  assert.deepEqual(vitals.gaugeSeries(rows, "frozen_xid_age", "sum").map(({ value }) => value), [1000, 120])
})

test("count series counts matching rows per moment including zero", () => {
  const activity = (ordinal, timestamp, state) => row(ordinal, timestamp, { state }, "1001004")
  const points = vitals.countSeries([
    activity(0, T0, "active"),
    activity(1, T0, "idle"),
    activity(2, T0 + 10 * SECOND, "idle"),
  ], (candidate) => candidate.values.state === "active")
  assert.deepEqual(points.map(({ value }) => value), [1, 0])
})

test("peak and last readings skip nulls", () => {
  const points = [
    { segmentId: "7", timestamp: T0, value: 3 },
    { segmentId: "7", timestamp: T0 + 1, value: null },
    { segmentId: "7", timestamp: T0 + 2, value: 9 },
    { segmentId: "7", timestamp: T0 + 3, value: null },
  ]
  assert.equal(vitals.peakPoint(points).value, 9)
  assert.equal(vitals.lastValue(points), 9)
  assert.equal(vitals.peakPoint([]), null)
})

test("share of totals divides the hour totals", () => {
  assert.equal(vitals.shareOfTotals({ points: [], total: 5 }, { points: [], total: 50 }), 0.1)
  assert.equal(vitals.shareOfTotals({ points: [], total: null }, { points: [], total: 50 }), null)
  assert.equal(vitals.shareOfTotals({ points: [], total: 5 }, { points: [], total: 0 }), null)
})

test("the first setting value is the baseline and later values are changes", () => {
  const setting = (ordinal, timestamp, name, stored) => row(ordinal, timestamp, { name, setting: stored }, "1019001")
  const rows = [
    setting(0, T0, "work_mem", "4MB"),
    setting(1, T0, "max_connections", "100"),
    setting(2, T0 + 60 * SECOND, "work_mem", "64MB"),
    setting(3, T0 + 120 * SECOND, "work_mem", "4MB"),
  ]
  assert.deepEqual(vitals.settingChanges(rows), [
    { timestamp: T0 + 60 * SECOND, name: "work_mem", from: "4MB", to: "64MB" },
    { timestamp: T0 + 120 * SECOND, name: "work_mem", from: "64MB", to: "4MB" },
  ])
  assert.equal(vitals.settingAt(rows, "work_mem", T0 + 70 * SECOND), "64MB")
  assert.equal(vitals.settingAt(rows, "work_mem", T0 + 130 * SECOND), "4MB")
  assert.equal(vitals.settingAt(rows, "shared_buffers", T0 + 130 * SECOND), null)
  assert.equal(vitals.settingAt(rows, "work_mem", T0 - SECOND), "4MB")
})

test("a gauge moment whose rows are all null keeps a null point", () => {
  const rows = [
    row(0, T0, { datid: 1, frozen_xid_age: 100 }),
    row(1, T0 + 10 * SECOND, { datid: 1, frozen_xid_age: null }),
  ]
  assert.deepEqual(vitals.gaugeSeries(rows, "frozen_xid_age", "max").map(({ value }) => value), [100, null])
})

test("anchored series reads zero for empty passes and assigns rows to the preceding anchor", () => {
  const anchor = (ordinal, timestamp) => row(ordinal, timestamp, { datid: 1 })
  const vacuum = (ordinal, timestamp) => row(ordinal, timestamp, { pid: 7 }, "1012001")
  const anchors = [anchor(0, T0), anchor(1, T0 + 10 * SECOND), anchor(2, T0 + 20 * SECOND)]
  const points = vitals.anchoredSeries(
    [vacuum(0, T0 + SECOND), vacuum(1, T0 + SECOND + 1), vacuum(2, T0 + 11 * SECOND)],
    anchors,
    (pass) => pass.length,
  )
  assert.deepEqual(points.map(({ timestamp, value }) => [timestamp - T0, value]), [
    [0, 2],
    [10 * SECOND, 1],
    [20 * SECOND, 0],
  ])
  assert.deepEqual(vitals.anchoredSeries([], anchors, (pass) => pass.length).map(({ value }) => value), [0, 0, 0])
  assert.deepEqual(vitals.anchoredSeries([], [], (pass) => pass.length), [])
})

test("composite identities keep pg_stat_io rows of one backend, object and context apart", () => {
  const io = (ordinal, timestamp, values) => row(ordinal, timestamp, values, "1009002")
  const rows = [
    io(0, T0, { backend_type: "client backend", object: "relation", context: "normal", evictions: 10 }),
    io(1, T0, { backend_type: "autovacuum worker", object: "relation", context: "vacuum", evictions: 4 }),
    io(2, T0 + 10 * SECOND, { backend_type: "client backend", object: "relation", context: "normal", evictions: 30 }),
    io(3, T0 + 10 * SECOND, { backend_type: "autovacuum worker", object: "relation", context: "vacuum", evictions: 9 }),
  ]
  const groups = vitals.counterGroups(rows, ["backend_type", "object", "context"])
  assert.equal(groups.size, 2)
  const vital = vitals.sumCounterVital(groups, ["evictions"])
  assert.equal(vital.total, 25)
  assert.equal(vital.points[0].value, 2.5)
})
