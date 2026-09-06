import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const blocks = [
  "shared_blks_hit", "shared_blks_read", "shared_blks_dirtied", "shared_blks_written",
  "local_blks_hit", "local_blks_read", "local_blks_dirtied", "local_blks_written",
  "temp_blks_read", "temp_blks_written",
]
const statementBase = ["datname", "usename", "query", "calls", "rows", ...blocks]
const oldTiming = ["blk_read_time", "blk_write_time"]
const splitTiming = [
  "shared_blk_read_time", "shared_blk_write_time", "local_blk_read_time", "local_blk_write_time",
  "temp_blk_read_time", "temp_blk_write_time",
]
const planning = ["plans", "total_plan_time"]
const wal = ["wal_records", "wal_fpi", "wal_bytes"]

function layout(typeId, logicalName, identity, columns) {
  return {
    typeId, logicalName, identity,
    columns: ["ts", ...columns, "private_text"],
  }
}

const registry = [
  layout("1002001", "pg_stat_statements", ["queryid", "userid", "dbid"], ["queryid", "userid", "dbid", ...statementBase, "total_time", "min_time", "max_time", "mean_time", "stddev_time", ...oldTiming]),
  layout("1002002", "pg_stat_statements", ["queryid", "userid", "dbid"], ["queryid", "userid", "dbid", ...statementBase, "total_exec_time", "min_exec_time", "max_exec_time", "mean_exec_time", "stddev_exec_time", ...planning, ...oldTiming, ...wal]),
  layout("1002003", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], ["queryid", "userid", "dbid", "toplevel", ...statementBase, "total_exec_time", "min_exec_time", "max_exec_time", "mean_exec_time", "stddev_exec_time", ...planning, ...oldTiming, ...wal]),
  layout("1002004", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], ["queryid", "userid", "dbid", "toplevel", ...statementBase, "total_exec_time", "min_exec_time", "max_exec_time", "mean_exec_time", "stddev_exec_time", ...planning, ...oldTiming, "temp_blk_read_time", "temp_blk_write_time", ...wal]),
  layout("1002005", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], ["queryid", "userid", "dbid", "toplevel", ...statementBase, "total_exec_time", "min_exec_time", "max_exec_time", "mean_exec_time", "stddev_exec_time", ...planning, ...splitTiming, ...wal, "stats_since"]),
  layout("1002006", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], ["queryid", "userid", "dbid", "toplevel", ...statementBase, "total_exec_time", "min_exec_time", "max_exec_time", "mean_exec_time", "stddev_exec_time", ...planning, ...splitTiming, ...wal, "wal_buffers_full", "stats_since"]),
  layout("1003001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], ["userid", "dbid", "queryid", "planid", "datname", "usename", "plan", "calls", "total_time", "min_time", "max_time", "mean_time", "stddev_time", "rows", ...blocks, ...splitTiming, "first_call", "last_call"]),
  layout("1004001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], ["userid", "dbid", "queryid", "planid", "queryid_stat_statements", "datname", "usename", "plan", "calls", "slow_log_calls", "total_time", "min_time", "max_time", "mean_time", "stddev_time", "rows", ...blocks, ...oldTiming, "total_plan_time", "first_call", "last_call"]),
  layout("1018001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], ["userid", "dbid", "queryid", "planid", "datname", "usename", "plan", "relids", "cmd_type", "calls", "total_time", "min_time", "max_time", "mean_time", "stddev_time", "rows", ...blocks, ...splitTiming, "first_call", "last_call"]),
  layout("1016001", "pg_store_plans_info", [], ["dealloc", "stats_reset"]),
]

const metrics = await importModule('export * from "../src/postgres-metrics.ts"', {
  loader: "ts",
  plugins: [registryPlugin(registry)],
})

function row(typeId, timestamp, values, segmentId = "s") {
  return { segmentId, logicalName: typeId.startsWith("1002") ? "pg_stat_statements" : "pg_store_plans", typeId, ordinal: "0", timestamp, values }
}

test("all six statement layouts keep exact registry identity, aliases, projections, and order", () => {
  const matrix = [
    ["1002001", ["queryid", "userid", "dbid"], "total_time", "blk_read_time", null, null],
    ["1002002", ["queryid", "userid", "dbid"], "total_exec_time", "blk_read_time", null, null],
    ["1002003", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "blk_read_time", null, null],
    ["1002004", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "blk_read_time", null, "temp_blk_read_time"],
    ["1002005", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time"],
    ["1002006", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time"],
  ]
  for (const [typeId, identity, execution, shared, local, temp] of matrix) {
    assert.deepEqual(metrics.postgresIdentity(typeId), identity)
    assert.equal(metrics.physicalField(typeId, "execution_ms_per_second"), execution)
    assert.equal(metrics.physicalField(typeId, "shared_blk_read_ms_per_second"), shared)
    assert.equal(metrics.physicalField(typeId, "local_blk_read_ms_per_second"), local)
    assert.equal(metrics.physicalField(typeId, "temp_blk_read_ms_per_second"), temp)
    const projection = metrics.postgresProjection(typeId)
    assert.deepEqual(projection.slice(0, identity.length), identity)
    assert.ok(projection.includes("query"))
    assert.ok(projection.includes(execution))
    assert.equal(projection.includes(execution === "total_time" ? "total_exec_time" : "total_time"), false)
    assert.equal(projection.includes("mean_time"), typeId === "1002001")
    assert.equal(projection.includes("private_text"), false)
  }
  assert.equal(metrics.statementRequest("load").defaultOrder.includes("total_time"), true)
  assert.equal(metrics.postgresProjection("1002004").includes("stats_since"), false)
  assert.equal(metrics.postgresProjection("1002005").includes("stats_since"), true)
})

test("the three plan layouts and info use their exact physical variants", () => {
  const identity = ["userid", "dbid", "queryid", "planid"]
  const matrix = [
    ["1003001", "shared_blk_read_time", "local_blk_read_time", false],
    ["1004001", "blk_read_time", null, true],
    ["1018001", "shared_blk_read_time", "local_blk_read_time", false],
  ]
  for (const [typeId, shared, local, isVadv] of matrix) {
    assert.deepEqual(metrics.postgresIdentity(typeId), identity)
    assert.equal(metrics.physicalField(typeId, "execution_ms_per_second"), "total_time")
    assert.equal(metrics.physicalField(typeId, "shared_blk_read_ms_per_second"), shared)
    assert.equal(metrics.physicalField(typeId, "local_blk_read_ms_per_second"), local)
    assert.equal(metrics.postgresProjection(typeId).includes("queryid_stat_statements"), isVadv)
    assert.equal(metrics.postgresProjection(typeId).includes("plan"), true)
    assert.equal(metrics.planRequest("identity").fieldsByType[typeId].filter((field) => field === "calls").length, 1)
  }
  assert.deepEqual(metrics.postgresProjection("1016001"), ["dealloc", "stats_reset"])
  assert.deepEqual(metrics.postgresIdentity("1016001"), [])
})

test("snapshot decoration preserves physical cells and derives only available rates", () => {
  const decorated = metrics.decoratePostgresIntervalRow(row("1002005", 10, {
    calls: 4,
    rows: 12,
    total_exec_time: 30,
    total_plan_time: 2,
    shared_blk_read_time: 6,
    query: "select 1",
  }))
  assert.equal(decorated.values.total_exec_time, 30)
  assert.equal(decorated.values.query, "select 1")
  assert.equal(decorated.values.calls_per_second, 4)
  assert.equal(decorated.values.rows_per_second, 12)
  assert.equal(decorated.values.execution_ms_per_second, 30)
  assert.equal(decorated.values.mean_exec_ms_per_call, 7.5)
  assert.equal(decorated.values.shared_blk_read_ms_per_second, 6)
  assert.equal(Object.hasOwn(decorated.values, "local_blk_read_ms_per_second"), false)
})

test("unavailable composite values and orders stay absent", () => {
  const old = metrics.decoratePostgresIntervalRow(row("1002001", 10, {
    calls: 4, total_time: 20, rows: 8,
  }))
  assert.equal(Object.hasOwn(old.values, "wal_per_call"), false)
  assert.equal(Object.hasOwn(old.values, "plan_time_pct"), false)
  assert.equal(metrics.supportsPostgresDerivedOrder("1002001", "derived.mean_exec_ms_per_call"), true)
  assert.equal(metrics.supportsPostgresDerivedOrder("1002001", "derived.wal_per_call"), false)
  assert.equal(metrics.supportsPostgresDerivedOrder("1002001", "derived.plan_time_pct"), false)
  assert.equal(metrics.supportsPostgresDerivedOrder("1002002", "derived.wal_per_call"), true)
})

test("history subtracts exact counters before conversion and rejects unusable intervals", () => {
  const before = row("1002002", 1_000_000, {
    calls: "9007199254740992", rows: "9007199254740990", total_exec_time: 100.25,
  })
  const after = row("1002002", 3_000_000, {
    calls: "9007199254740995", rows: "9007199254740994", total_exec_time: 130.25,
  })
  assert.equal(metrics.exactCounterDelta(before.values.calls, after.values.calls), 3n)
  assert.equal(metrics.intervalMetric(before, after, "calls"), 1.5)
  assert.deepEqual(metrics.postgresInterval(before, after), {
    calls_per_second: 1.5,
    execution_ms_per_second: 15,
    mean_exec_ms_per_call: 10,
    rows_per_second: 2,
    planning_ms_per_second: null,
    shared_blk_read_ms_per_second: null,
    shared_blk_write_ms_per_second: null,
    local_blk_read_ms_per_second: null,
    local_blk_write_ms_per_second: null,
    temp_blk_read_ms_per_second: null,
    temp_blk_write_ms_per_second: null,
  })
  const history = metrics.postgresHistory([after, before])
  assert.equal(history[0].mean_exec_ms_per_call, null)
  assert.equal(history[1].mean_exec_ms_per_call, 10)

  const reset = row("1002002", 4_000_000, { calls: "2", total_exec_time: 1 })
  assert.equal(metrics.postgresInterval(after, reset).mean_exec_ms_per_call, null)
  assert.equal(metrics.postgresInterval(after, reset).calls_per_second, null)
  const missing = row("1002002", 4_000_000, { total_exec_time: 140 })
  assert.equal(metrics.postgresInterval(after, missing).mean_exec_ms_per_call, null)
  const unavailable = row("1002002", 4_000_000, { calls: null, total_exec_time: null })
  assert.equal(metrics.postgresInterval(after, unavailable).mean_exec_ms_per_call, null)
  const zero = row("1002002", 4_000_000, { calls: after.values.calls, rows: after.values.rows, total_exec_time: after.values.total_exec_time })
  const zeroInterval = metrics.postgresInterval(after, zero)
  assert.equal(zeroInterval.calls_per_second, 0)
  assert.equal(zeroInterval.execution_ms_per_second, 0)
  assert.equal(zeroInterval.rows_per_second, 0)
  assert.equal(zeroInterval.mean_exec_ms_per_call, null)
  assert.deepEqual(metrics.postgresHistory([before, after, zero]).map(({ calls_per_second, mean_exec_ms_per_call }) => [calls_per_second, mean_exec_ms_per_call]), [
    [null, null], [1.5, 10], [0, null],
  ])
  const backwards = row("1002002", 2_000_000, { calls: "9007199254740996", total_exec_time: 140 })
  assert.equal(metrics.postgresInterval(after, backwards).mean_exec_ms_per_call, null)
  const executionReset = row("1002002", 4_000_000, { calls: "9007199254740996", total_exec_time: 1 })
  assert.equal(metrics.postgresInterval(after, executionReset).execution_ms_per_second, null)
  assert.equal(metrics.postgresInterval(after, executionReset).mean_exec_ms_per_call, null)
})

test("a physical statement spike selects interval mean execution time", () => {
  assert.equal(metrics.findingSemanticField("1002001", "total_time"), "mean_exec_ms_per_call")
  assert.equal(metrics.findingSemanticField("1002005", "total_exec_time"), "mean_exec_ms_per_call")
  assert.deepEqual(metrics.physicalFields(metrics.PG_STAT_STATEMENTS_TYPE_IDS, "execution_ms_per_second"), {
    "1002001": "total_time",
    "1002002": "total_exec_time",
    "1002003": "total_exec_time",
    "1002004": "total_exec_time",
    "1002005": "total_exec_time",
    "1002006": "total_exec_time",
  })
})

test("statement lenses project only their exact physical operands", () => {
  const perCall = metrics.statementRequest("per_call")
  assert.equal(perCall.pageSize, 200)
  assert.deepEqual(perCall.defaultOrder, ["calls"])
  assert.equal(metrics.statementDefaultOrder("per_call"), "calls_per_second")
  assert.ok(perCall.fieldsByType["1002001"].includes("rows"))
  assert.ok(perCall.fieldsByType["1002001"].includes("calls"))
  assert.ok(perCall.fieldsByType["1002001"].includes("shared_blks_hit"))
  assert.equal(perCall.fieldsByType["1002001"].includes("wal_bytes"), false)
  assert.equal(perCall.fieldsByType["1002001"].includes("shared_blks_dirtied"), false)
  assert.deepEqual(perCall.order.mean_exec_ms_per_call, ["derived.mean_exec_ms_per_call"])
  assert.deepEqual(perCall.order.rows_per_call, ["derived.rows_per_call"])
  assert.deepEqual(perCall.order.blocks_per_call, ["derived.blocks_per_call"])

  const io = metrics.statementRequest("io")
  assert.ok(io.fieldsByType["1002001"].includes("shared_blks_dirtied"))
  assert.equal(io.fieldsByType["1002001"].includes("blk_read_time"), false)
  assert.deepEqual(io.order.hit_pct, ["derived.hit_pct"])
  assert.deepEqual(io.order.shared_blks_dirtied, ["shared_blks_dirtied"])

  const resources = metrics.statementRequest("resources")
  assert.ok(resources.fieldsByType["1002006"].includes("temp_blks_written"))
  assert.equal(resources.fieldsByType["1002006"].includes("temp_blks_read"), false)
  assert.deepEqual(resources.defaultOrder, ["wal_bytes"])
  assert.equal(metrics.statementDefaultOrder("resources"), "wal_bytes")
  assert.deepEqual(resources.order.wal_per_call, ["derived.wal_per_call"])
  assert.deepEqual(resources.order.plan_time_pct, ["derived.plan_time_pct"])

  const stability = metrics.statementRequest("stability")
  assert.ok(stability.fieldsByType["1002001"].includes("mean_time"))
  assert.ok(stability.fieldsByType["1002001"].includes("stddev_time"))
  assert.ok(stability.fieldsByType["1002006"].includes("mean_exec_time"))
  assert.ok(stability.fieldsByType["1002006"].includes("stddev_exec_time"))
  assert.deepEqual(stability.defaultOrder, ["calls"])
  assert.deepEqual(stability.order.cv, ["derived.cv"])
  assert.deepEqual(stability.order.mean_exec_time_ms, ["mean_time", "mean_exec_time"])
})

test("plan lenses keep bounded rows and direct per-plan statistics", () => {
  for (const lens of ["load", "timing", "io", "identity"]) {
    const request = metrics.planRequest(lens)
    assert.equal(request.pageSize, 200)
    assert.ok(request.fieldsByType["1003001"].includes("plan"))
  }
  const timing = metrics.planRequest("timing")
  for (const field of ["min_time", "max_time", "mean_time", "stddev_time", "first_call", "last_call"]) {
    assert.ok(timing.fieldsByType["1003001"].includes(field))
  }
  assert.equal(timing.fieldsByType["1003001"].includes("queryid_stat_statements"), false)
  assert.equal(timing.fieldsByType["1004001"].includes("queryid_stat_statements"), true)
  assert.deepEqual(timing.order.mean_exec_time_ms, ["mean_time"])
  assert.deepEqual(timing.order.first_call, ["first_call"])
  const identity = metrics.planRequest("identity")
  assert.ok(identity.fieldsByType["1004001"].includes("queryid_stat_statements"))
  assert.ok(identity.fieldsByType["1003001"].includes("calls"))
  assert.deepEqual(identity.defaultOrder, ["calls"])
  assert.equal(identity.fieldsByType["1003001"].includes("mean_time"), false)
  const decorated = metrics.decoratePostgresIntervalRow(
    row("1003001", 10, { calls: 2, total_time: 20, min_time: 2, max_time: 15, mean_time: 10, stddev_time: 3 }, "a"),
  )
  assert.equal(decorated.values.mean_exec_time_ms, 10)
  assert.equal(decorated.values.max_exec_time_ms, 15)
  assert.equal(Object.hasOwn(decorated.values, "plan_count"), false)
  assert.equal(Object.hasOwn(decorated.values, "time_ratio"), false)
})
