import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { renderToStaticMarkup } from "react-dom/server"

import { importModule, registryPlugin } from "./import-module.mjs"
import { parseDictionary, validateDictionaries } from "../scripts/i18n.mjs"

const BLOCK_COUNTERS = [
  "shared_blks_hit", "shared_blks_read", "shared_blks_dirtied", "shared_blks_written",
  "local_blks_hit", "local_blks_read", "local_blks_dirtied", "local_blks_written",
  "temp_blks_read", "temp_blks_written",
]
const STATEMENT_BASE = ["ts", "queryid", "userid", "dbid", "datname", "usename", "query", "calls", "rows", ...BLOCK_COUNTERS]
const STATEMENT_V2 = [...STATEMENT_BASE, "plans", "total_exec_time", "total_plan_time", "blk_read_time", "blk_write_time", "wal_records", "wal_fpi", "wal_bytes"]
const PLAN_BASE = ["ts", "userid", "dbid", "queryid", "planid", "datname", "usename", "plan", "calls", "total_time", "rows", ...BLOCK_COUNTERS]
const TEST_REGISTRY = [
  layout("1002001", "pg_stat_statements", ["queryid", "userid", "dbid"], [...STATEMENT_BASE, "total_time", "blk_read_time", "blk_write_time"]),
  layout("1002002", "pg_stat_statements", ["queryid", "userid", "dbid"], STATEMENT_V2),
  layout("1002003", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], [...STATEMENT_V2, "toplevel"]),
  layout("1002004", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], [...STATEMENT_V2, "toplevel", "temp_blk_read_time", "temp_blk_write_time"]),
  layout("1002005", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], statementV5()),
  layout("1002006", "pg_stat_statements", ["queryid", "userid", "dbid", "toplevel"], [...statementV5(), "wal_buffers_full"]),
  layout("1003001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], [...PLAN_BASE, "shared_blk_read_time", "shared_blk_write_time", "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time"]),
  layout("1004001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], [...PLAN_BASE, "queryid_stat_statements", "slow_log_calls", "blk_read_time", "blk_write_time", "total_plan_time"]),
  layout("1018001", "pg_store_plans", ["userid", "dbid", "queryid", "planid"], [...PLAN_BASE, "relids", "cmd_type", "shared_blk_read_time", "shared_blk_write_time", "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time"]),
  layout("1016001", "pg_store_plans_info", [], ["ts", "dealloc", "stats_reset"]),
  layout("1020001", "pg_wal_storage", [], ["ts", "wal_files_bytes"]),
]
const helpers = await importModule(
  'export { ACTIVITY_COLUMNS, ACTIVITY_DEFAULT_ORDER, ACTIVITY_DETAIL_COLUMNS, activityColumns, activityDurationHistory, activityDurationMs, chartColumnAvailable, chartFormat, chartPointValue, chartScale, chartUnit, chartableColumn, columnsFor, DATABASE_COLUMNS, denseHistoryFields, denseMetricHistory, isIdleActivity, isKronikaMonitorStatement, isSystemActivity, isTimestampField, lockDetailColumns, lockRowLabel, LOCK_COLUMNS, PLAN_COLUMNS, planColumns, postgresBlockSize, postgresByteColumns, postgresDatabaseCount, postgresMetricHistory, postgresMetricHistoryRequest, postgresMetricHistorySamples, vacuumColumns, vacuumDetailColumns, sameEntity, selectedEntity, STATEMENT_COLUMNS, statementColumns, tableState, transactionDurationMs, visibleActivityRows } from "../src/postgres-view.tsx"; export { decoratePostgresIntervalRow, findingSemanticField, physicalField, planDefaultOrder, planRequest, postgresIdentity, postgresProjection, statementDefaultOrder, statementRequest } from "../src/postgres-metrics.ts"; export { humanDuration } from "../src/model.ts"',
  { plugins: [registryPlugin(TEST_REGISTRY)] },
)

function layout(typeId, logicalName, identity, fields) {
  return { typeId, logicalName, identity, columns: [...new Set(fields)] }
}

function statementV5() {
  return [
    ...STATEMENT_V2.filter((field) => field !== "blk_read_time" && field !== "blk_write_time"),
    "toplevel", "shared_blk_read_time", "shared_blk_write_time", "local_blk_read_time", "local_blk_write_time",
    "temp_blk_read_time", "temp_blk_write_time", "stats_since",
  ]
}

function row(typeId, values, logicalName = "pg_stat_statements") {
  return { logicalName, ordinal: "0", segmentId: "a", timestamp: 1, typeId, values }
}

function activityRow(ordinal, values, timestamp = 10_000_000) {
  return { logicalName: "pg_stat_activity", ordinal, segmentId: "a", timestamp, typeId: "1001004", values }
}

test("PostgreSQL durations are not formatted as Unix timestamps", () => {
  assert.equal(helpers.isTimestampField("write_time"), false)
  assert.equal(helpers.isTimestampField("stats_reset"), true)
  assert.equal(helpers.columnsFor([row("1", { write_time: 123.4 })])[0].kind, "milliseconds")
  assert.equal(helpers.columnsFor([row("1", { max_age_us: 123.4 })])[0].kind, "microseconds")
})

test("Kronika's own statements are recognised only by the exact monitor prefix", () => {
  const workload = row("1002003", { query: "select * from shop.orders" })
  const monitor = { ...row("1002003", { query: "/* kronika:1.0.0 statements */ select 1" }), ordinal: "1" }
  const differentCase = { ...row("1002003", { query: "/* Kronika:1.0.0 statements */ select 1" }), ordinal: "2" }
  const leadingSpace = { ...row("1002003", { query: " /* kronika:1.0.0 statements */ select 1" }), ordinal: "3" }
  const missing = { ...row("1002003", { query: null }), ordinal: "4" }
  const rows = [monitor, workload, differentCase, leadingSpace, missing]

  assert.equal(helpers.isKronikaMonitorStatement(monitor), true)
  assert.equal(helpers.isKronikaMonitorStatement(workload), false)
  assert.equal(helpers.isKronikaMonitorStatement(differentCase), false)
  assert.equal(helpers.isKronikaMonitorStatement(leadingSpace), false)
  assert.equal(helpers.isKronikaMonitorStatement(missing), false)
  // The page itself is scoped by the server; the client only recognises rows it already holds.
  assert.deepEqual(rows.filter(helpers.isKronikaMonitorStatement), [monitor])
})

test("PostgreSQL chart actions accept only numeric values and declare semantic units", () => {
  for (const kind of ["number", "estimated_rows", "bytes", "kib", "milliseconds", "duration", "microseconds", "percent"]) {
    assert.equal(helpers.chartableColumn({ field: kind, kind }), true, kind)
  }
  for (const kind of ["id", "text", "timestamp", "boolean", undefined]) {
    assert.equal(helpers.chartableColumn({ field: String(kind), kind }), false, String(kind))
  }
  assert.equal(helpers.chartUnit({ field: "calls", kind: "number", rate: true }), "/s")
  assert.equal(helpers.chartUnit({ field: "rows", kind: "number" }), "count")
  assert.equal(helpers.chartUnit({ field: "wal", kind: "bytes", rate: true }), "")
  assert.equal(helpers.chartUnit({ field: "latency", kind: "milliseconds" }), "")
  assert.equal(helpers.chartUnit({ field: "latency", kind: "milliseconds", rate: true }), "/s")
  assert.equal(helpers.chartUnit({ field: "cpu", kind: "percent" }), "%")
  assert.equal(helpers.chartPointValue(2, { field: "buffers", kind: "kib" }), 2048)
  assert.equal(helpers.chartPointValue(2500, { field: "latency", kind: "microseconds" }), 2.5)
  assert.equal(helpers.chartPointValue(0, { field: "calls", kind: "number" }), 0)
  assert.equal(helpers.chartPointValue(null, { field: "calls", kind: "number" }), null)
  assert.equal(helpers.chartScale({ field: "cpu", kind: "percent" }), "percent")
  assert.equal(helpers.chartScale({ field: "calls", kind: "number" }), "nonnegative")
  assert.equal(helpers.chartFormat("percent")(0.099, "en"), "<0.1%")
  assert.equal(helpers.chartFormat("percent")(12, "ru"), "12 %")
})

test("PostgreSQL block counters use the recorded exact block size and human byte dimensions", () => {
  const settings = [row("settings", { name: "block_size", setting: "8192" }, "pg_settings")]
  assert.equal(helpers.postgresBlockSize(settings, 1), 8192)
  assert.equal(helpers.postgresBlockSize([row("settings", { name: "block_size", setting: "invalid" }, "pg_settings")], 1), null)

  const io = helpers.statementColumns("io", 8192)
  const read = io.find(({ field }) => field === "shared_blks_read")
  const perCall = io.find(({ field }) => field === "blocks_per_call")
  assert.deepEqual([read.kind, read.valueScale, perCall.kind, perCall.valueScale], ["bytes", 8192, "bytes", 8192])
  assert.equal(helpers.chartPointValue(787, read), 6_447_104)
  assert.equal(helpers.chartFormat("bytes", "/s")(6_447_104, "en"), "6.1 MiB/s")
  assert.equal(helpers.statementColumns("io").find(({ field }) => field === "shared_blks_read").valueScale, null)
})

test("PostgreSQL generic histories preserve absent, null, zero, storage, and counter semantics", () => {
  const stored = (segmentId, timestamp, values, ordinal = String(timestamp)) => ({ logicalName: "pg_stat_checkpointer", ordinal, segmentId, timestamp, typeId: "overview-1", values })
  const rows = [
    stored("a", 1_000_000, { writes: 10, latency_ms: 3 }),
    stored("a", 2_000_000, { other: 1 }),
    stored("b", 3_000_000, { writes: 16, latency_ms: null }),
    stored("b", 4_000_000, { writes: null, latency_ms: 0 }),
    stored("b", 5_000_000, { writes: 1 }),
    stored("b", 6_000_000, { writes: 3 }),
  ]
  assert.deepEqual(helpers.postgresMetricHistory(rows, { field: "writes", kind: "number", rate: true }, true).map(({ value }) => value), [null, 3, null, null, 2])
  assert.deepEqual(helpers.postgresMetricHistory(rows, { field: "latency_ms", kind: "milliseconds" }, false).map(({ value }) => value), [3, null, 0])
  const proofRows = [
    stored("a", 1_000_000, { writes: 100, stats_reset: "500000" }),
    stored("a", 2_000_000, { writes: 110, stats_reset: "500000" }),
    stored("a", 3_000_000, { writes: 120, stats_reset: "2500000" }),
  ]
  assert.deepEqual(helpers.postgresMetricHistory(proofRows, { field: "writes", kind: "number", rate: true }, true).map(({ value }) => value), [null, 10, 10])
})

test("dense statement histories cover per-call and percentage lens metrics", () => {
  const stored = (timestamp, values) => ({ ...row("1002001", values), ordinal: String(timestamp), timestamp })
  const rows = [
    stored(1_000_000, { calls: 10, rows: 20, shared_blks_hit: 10, shared_blks_read: 0, local_blks_hit: 0, local_blks_read: 0 }),
    stored(2_000_000, { calls: 12, rows: 28, shared_blks_hit: 13, shared_blks_read: 1, local_blks_hit: 0, local_blks_read: 0 }),
    stored(3_000_000, { calls: 12, rows: 28, shared_blks_hit: 13, shared_blks_read: 1, local_blks_hit: 0, local_blks_read: 0 }),
  ]
  assert.deepEqual(helpers.denseHistoryFields("1002001", "rows_per_call"), ["rows", "calls"])
  assert.deepEqual(helpers.denseMetricHistory(rows, "1002001", { field: "calls_per_second", kind: "number", rate: true }).map(({ value }) => value), [null, 2, 0])
  assert.deepEqual(helpers.denseMetricHistory(rows, "1002001", { field: "rows_per_call", kind: "number" }).map(({ value }) => value), [null, 4, null])
  assert.deepEqual(helpers.denseMetricHistory(rows, "1002001", { field: "blocks_per_call", kind: "number" }).map(({ value }) => value), [null, 2, null])
  assert.deepEqual(helpers.denseMetricHistory(rows, "1002001", { field: "hit_pct", kind: "percent" }).map(({ value }) => value), [null, 75, null])
  const v2 = (timestamp, values) => ({ ...row("1002002", values), ordinal: String(timestamp), timestamp })
  const laterZero = [
    v2(1_000_000, { calls: 10, total_exec_time: 50, total_plan_time: 5, wal_bytes: 100 }),
    v2(2_000_000, { calls: 12, total_exec_time: 60, total_plan_time: 7, wal_bytes: 110 }),
    v2(3_000_000, { calls: 12, total_exec_time: 60, total_plan_time: 7, wal_bytes: 110 }),
  ]
  assert.deepEqual(helpers.denseMetricHistory(laterZero, "1002002", { field: "wal_per_call", kind: "bytes" }).map(({ value }) => value), [null, 5, null])
  assert.deepEqual(helpers.denseMetricHistory(laterZero, "1002002", { field: "plan_time_pct", kind: "percent" }).map(({ value }) => value), [null, 100 * 2 / 12, null])
})


test("Overview multirow histories keep complete fixed identities", () => {
  const io = row("1009002", { backend_type: "client backend", object: "relation", context: "normal", reads: 4 }, "pg_stat_io")
  assert.equal(helpers.sameEntity(io, { ...io, values: { ...io.values, reads: 9 } }, "pg_stat_io"), true)
  assert.equal(helpers.sameEntity(io, { ...io, values: { ...io.values, context: "vacuum" } }, "pg_stat_io"), false)
  const prepared = row("1010001", { datname: "app", prepared_count: 1 }, "pg_prepared_xacts")
  assert.equal(helpers.sameEntity(prepared, { ...prepared, values: { datname: "other", prepared_count: 1 } }, "pg_prepared_xacts"), false)
})



test("activity keeps a compact operator table and uses relative detail durations", async () => {
  assert.deepEqual(
    helpers.ACTIVITY_COLUMNS[0],
    { field: "pid", kind: "id", label: "pg.field.pid.label", sticky: true, width: 78 },
  )
  assert.deepEqual(
    helpers.ACTIVITY_COLUMNS.map(({ field }) => field),
    ["pid", "datname", "usename", "query", "query_duration_ms", "transaction_duration_ms", "application_name", "client_addr", "state", "wait_event_type", "wait_event"],
  )
  for (const field of ["backend_type", "leader_pid", "query_id", "backend_xid_age", "backend_xmin_age"]) {
    assert.equal(helpers.ACTIVITY_COLUMNS.some((column) => column.field === field), false)
    assert.equal(helpers.ACTIVITY_DETAIL_COLUMNS.some((column) => column.field === field), true)
  }
  for (const field of ["backend_start", "xact_start", "query_start", "state_change"]) {
    assert.equal(helpers.ACTIVITY_DETAIL_COLUMNS.some((column) => column.field === field), false)
  }
  for (const field of ["backend_age_ms", "query_duration_ms", "transaction_duration_ms", "state_duration_ms"]) {
    assert.equal(helpers.ACTIVITY_DETAIL_COLUMNS.some((column) => column.field === field), true)
  }
  assert.deepEqual(helpers.activityColumns(false), helpers.ACTIVITY_COLUMNS)
  assert.deepEqual(helpers.activityColumns(true).slice(0, 4).map(({ field }) => field), ["pid", "backend_type", "datname", "usename"])
  assert.deepEqual(helpers.ACTIVITY_DEFAULT_ORDER, { column: "query_duration_ms", descending: true })
  const viewSource = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"))
  assert.match(viewSource, /column\.field !== "backend_age_ms"/)
})

test("activity hides only ordinary idle and derives elapsed time from the selected observation", () => {
  const active = activityRow("1", { backend_start: "500000", backend_type: "client backend", state: "active", state_change: "7000000", query_start: "4000000", xact_start: "1000000" })
  const idle = activityRow("2", { backend_type: "client backend", state: "idle", query_start: "1000000", xact_start: null })
  const idleTransaction = activityRow("3", { backend_type: "client backend", state: "idle in transaction", query_start: "2000000", xact_start: "2000000" })
  const aborted = activityRow("6", { backend_type: "client backend", state: "idle in transaction (aborted)", query_start: "3000000", xact_start: "3000000" })
  const system = activityRow("4", { backend_type: "checkpointer", state: null, query_start: null })
  const walsender = activityRow("7", { backend_type: "walsender", state: null, query_start: null })
  const legacy = activityRow("5", { backend_type: null, state: "active", query_start: "9000000", xact_start: "8000000" })

  assert.equal(helpers.isSystemActivity(active), false)
  assert.equal(helpers.isSystemActivity(system), true)
  assert.equal(helpers.isSystemActivity(walsender), true)
  assert.equal(helpers.isSystemActivity(legacy), false)
  assert.equal(helpers.isIdleActivity(idle), true)
  assert.equal(helpers.isIdleActivity(idleTransaction), false)
  assert.equal(helpers.isIdleActivity(aborted), false)
  assert.equal(helpers.activityDurationMs(active), 6_000)
  assert.equal(helpers.activityDurationMs(idle), null)
  assert.equal(helpers.activityDurationMs(idleTransaction), null)
  assert.equal(helpers.transactionDurationMs(active), 9_000)
  assert.equal(helpers.transactionDurationMs(idleTransaction), 8_000)
  assert.equal(helpers.transactionDurationMs(aborted), 7_000)
  assert.equal(helpers.transactionDurationMs(idle), null)
  assert.equal(helpers.activityDurationMs(activityRow("6", { state: "active", query_start: "11000000" })), null)
  assert.equal(helpers.transactionDurationMs(activityRow("7", { xact_start: "11000000" })), null)
  assert.equal(helpers.transactionDurationMs(activityRow("8", { xact_start: "0" })), null)

  const rows = [active, idle, idleTransaction, aborted, system, walsender, legacy]
  const defaults = helpers.visibleActivityRows(rows, { showIdle: false, showSystem: false })
  assert.deepEqual(defaults.map(({ ordinal }) => ordinal), ["1", "5", "3", "6"])
  assert.equal(defaults[0].values.query_duration_ms, 6_000)
  assert.equal(defaults[0].values.transaction_duration_ms, 9_000)
  assert.equal(defaults[0].values.backend_age_ms, 9_500)
  assert.equal(defaults[0].values.state_duration_ms, 3_000)
  assert.equal(defaults[1].values.query_duration_ms, 1_000)
  assert.equal(defaults[2].values.query_duration_ms, null)
  assert.equal(defaults[2].values.transaction_duration_ms, 8_000)
  assert.deepEqual(
    helpers.visibleActivityRows(rows, { showIdle: true, showSystem: false }).map(({ ordinal }) => ordinal),
    ["1", "5", "3", "6", "2"],
  )
  assert.deepEqual(
    helpers.visibleActivityRows(rows, { showIdle: false, showSystem: true }).map(({ ordinal }) => ordinal),
    ["1", "5", "3", "6", "4", "7"],
  )
  assert.deepEqual(
    helpers.visibleActivityRows(rows, { showIdle: false, showSystem: false }, system).map(({ ordinal }) => ordinal),
    ["1", "5", "3", "6", "4"],
  )
})

test("Activity observation timestamps enable duration histories without synthetic stored fields", () => {
  const rows = [activityRow("1", { query_start: "4000000", xact_start: "1000000" })]
  assert.equal(helpers.chartColumnAvailable("pg_stat_activity", rows, { field: "query_duration_ms", kind: "duration" }), true)
  assert.equal(helpers.chartColumnAvailable("pg_stat_activity", rows, { field: "transaction_duration_ms", kind: "duration" }), true)
  assert.equal(helpers.chartColumnAvailable("pg_stat_activity", rows, { field: "missing", kind: "duration" }), false)
  assert.equal(helpers.chartColumnAvailable("pg_locks", rows, { field: "query_duration_ms", kind: "duration" }), false)
})

test("Activity history is PID-only and loads the complete selected hour", async () => {
  const selected = activityRow("1", { pid: 42, backend_start: "9000000", state: "active", query_start: "9500000" })
  const differentStart = activityRow("2", { pid: 42, backend_start: "10500000", state: "active", query_start: "9500000" }, 11_000_000)
  const projected = activityRow("3", { pid: 42, state: "active", query_start: "9500000" }, 12_000_000)
  assert.equal(helpers.sameEntity(selected, differentStart, "pg_stat_activity"), true)
  assert.equal(helpers.sameEntity(selected, projected, "pg_stat_activity"), true)
  assert.equal(helpers.sameEntity(selected, { ...projected, typeId: "1001004" }, "pg_stat_activity"), true)
  assert.equal(helpers.sameEntity(selected, activityRow("4", { pid: 43 }), "pg_stat_activity"), false)
  assert.equal(helpers.sameEntity(activityRow("5", {}), activityRow("6", {}), "pg_stat_activity"), false)

  const column = helpers.ACTIVITY_COLUMNS.find(({ field }) => field === "query_duration_ms")
  assert.ok(column)
  const request = helpers.postgresMetricHistoryRequest(selected, "pg_stat_activity", column)
  assert.deepEqual(request.fields, ["pid", "state", "query_start"])
  assert.deepEqual(request.filters, { pid: "42" })
  assert.equal(request.fields.includes("backend_start"), false)
  assert.deepEqual(
    helpers.postgresMetricHistorySamples([
      differentStart,
      { ...projected, typeId: "1001004" },
      activityRow("4", { pid: 43, state: "active", query_start: "11000000" }, 13_000_000),
    ], selected, "pg_stat_activity", column, request).map(({ value }) => value),
    [1_500, 2_500],
  )

  const source = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /section === "pg_stat_activity"[^\n]*return \["pid"\]/)
  assert.match(source, /section === "pg_stat_activity"\s*\n\s*\? await loadSeries\(hour, section, filters, request\.fields, signal\)/)
  assert.doesNotMatch(source, /activity \? \["backend_start"|\["pid", "backend_start"\]/)
})

test("Activity relative-duration histories keep the selected value and later samples", () => {
  const rows = [
    activityRow("1", { backend_start: "1000000", pid: 274126, state: "active", state_change: "9000000", query_start: "9500000", xact_start: "8000000" }, 10_000_000),
    activityRow("2", { backend_start: "1000000", pid: 274126, state: "active", state_change: "9000000", query_start: "9500000", xact_start: "8000000" }, 12_000_000),
  ]
  assert.deepEqual(helpers.activityDurationHistory(rows, "backend_age_ms").map(({ timestamp, value }) => [timestamp, value]), [
    [10_000_000, 9_000], [12_000_000, 11_000],
  ])
  assert.deepEqual(helpers.activityDurationHistory(rows, "query_duration_ms").map(({ timestamp, value }) => [timestamp, value]), [
    [10_000_000, 500], [12_000_000, 2_500],
  ])
  assert.deepEqual(helpers.activityDurationHistory(rows, "state_duration_ms").map(({ timestamp, value }) => [timestamp, value]), [
    [10_000_000, 1_000], [12_000_000, 3_000],
  ])
  assert.deepEqual(helpers.activityDurationHistory(rows, "transaction_duration_ms").map(({ timestamp, value }) => [timestamp, value]), [
    [10_000_000, 2_000], [12_000_000, 4_000],
  ])
})

test("Activity state duration breaks across pure idle but keeps transactional idle", () => {
  const rows = [
    activityRow("1", { pid: 274126, state: "active", state_change: "9000000" }, 10_000_000),
    activityRow("2", { pid: 274126, state: "idle", state_change: "10000000" }, 12_000_000),
    activityRow("3", { pid: 274126, state: "idle in transaction", state_change: "12000000" }, 15_000_000),
    activityRow("4", { pid: 274126, state: "idle in transaction (aborted)", state_change: "15000000" }, 19_000_000),
  ]
  assert.deepEqual(helpers.activityDurationHistory(rows, "state_duration_ms").map(({ value }) => value), [1_000, null, 3_000, 4_000])
})

test("elapsed Activity values use compact wall-time formatting", () => {
  assert.equal(helpers.humanDuration(850, "en"), "850 ms")
  assert.equal(helpers.humanDuration(5_200, "en"), "5.2 s")
  assert.equal(helpers.humanDuration(59_999, "en"), "60 s")
  assert.equal(helpers.humanDuration(60_000, "en"), "1 min")
  assert.equal(helpers.humanDuration(194_000, "en"), "3.23 min")
  assert.equal(helpers.humanDuration(7_560_000, "en"), "2.1 h")
  assert.equal(helpers.humanDuration(163_800_000, "en"), "45.5 h")
  assert.equal(helpers.humanDuration(-3_600_000, "ru"), "-1 ч")
  assert.equal(helpers.humanDuration(5_200, "ru"), "5,2 с")
  assert.equal(helpers.humanDuration(null, "ru"), "—")
})

test("all statement layouts use their registered identity and timing names", () => {
  const cases = [
    ["1002001", ["queryid", "userid", "dbid"], "total_time", "blk_read_time", null, null],
    ["1002002", ["queryid", "userid", "dbid"], "total_exec_time", "blk_read_time", null, null],
    ["1002003", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "blk_read_time", null, null],
    ["1002004", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "blk_read_time", null, "temp_blk_read_time"],
    ["1002005", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time"],
    ["1002006", ["queryid", "userid", "dbid", "toplevel"], "total_exec_time", "shared_blk_read_time", "local_blk_read_time", "temp_blk_read_time"],
  ]
  for (const [typeId, identity, execution, sharedRead, localRead, tempRead] of cases) {
    assert.deepEqual(helpers.postgresIdentity(typeId), identity)
    assert.equal(helpers.physicalField(typeId, "execution_ms_per_second"), execution)
    assert.equal(helpers.physicalField(typeId, "shared_blk_read_ms_per_second"), sharedRead)
    assert.equal(helpers.physicalField(typeId, "local_blk_read_ms_per_second"), localRead)
    assert.equal(helpers.physicalField(typeId, "temp_blk_read_ms_per_second"), tempRead)
    const projection = helpers.postgresProjection(typeId)
    assert.deepEqual(projection.slice(0, identity.length), identity)
    assert.equal(projection.includes(execution), true)
    const decorated = helpers.decoratePostgresIntervalRow(row(typeId, { calls: 4, rows: 12, [execution]: 20, [sharedRead]: 8 }))
    assert.equal(decorated.values.calls_per_second, 4)
    assert.equal(decorated.values.execution_ms_per_second, 20)
    assert.equal(decorated.values.mean_exec_ms_per_call, 5)
    assert.equal(decorated.values.rows_per_second, 12)
    assert.equal(decorated.values.shared_blk_read_ms_per_second, 8)
  }

  for (const typeId of ["1002001", "1002002"]) {
    assert.equal(helpers.postgresProjection(typeId).includes("toplevel"), false)
    const base = row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: true })
    assert.equal(helpers.sameEntity(base, row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: false }), "pg_stat_statements"), true)
  }
  for (const typeId of ["1002003", "1002004", "1002005", "1002006"]) {
    const base = row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: true })
    assert.equal(helpers.sameEntity(base, row(typeId, { queryid: 7, userid: 8, dbid: 9, toplevel: false }), "pg_stat_statements"), false)
  }
  assert.equal(helpers.sameEntity(row("1002001", { queryid: 7, userid: 8, dbid: 9 }), row("1002002", { queryid: 7, userid: 8, dbid: 9 }), "pg_stat_statements"), false)
})

test("plan layouts expose plan identity and only their available semantics", () => {
  const identity = ["userid", "dbid", "queryid", "planid"]
  const cases = [
    ["1003001", "shared_blk_read_time", null, false],
    ["1004001", "blk_read_time", "total_plan_time", true],
    ["1018001", "shared_blk_read_time", null, false],
  ]
  for (const [typeId, sharedRead, planning, lastAttribution] of cases) {
    assert.deepEqual(helpers.postgresIdentity(typeId), identity)
    assert.equal(helpers.physicalField(typeId, "execution_ms_per_second"), "total_time")
    assert.equal(helpers.physicalField(typeId, "shared_blk_read_ms_per_second"), sharedRead)
    assert.equal(helpers.physicalField(typeId, "planning_ms_per_second"), planning)
    assert.equal(helpers.postgresProjection(typeId).includes("queryid_stat_statements"), lastAttribution)
    const decorated = helpers.decoratePostgresIntervalRow(row(typeId, { calls: "9007199254740993", calls_per_second: 2, total_time: 18 }, "pg_store_plans"))
    assert.equal(decorated.values.calls, "9007199254740993")
    assert.equal(decorated.values.calls_per_second, 2)
    assert.equal(decorated.values.execution_ms_per_second, 18)
    assert.equal(decorated.values.mean_exec_ms_per_call, 9)
  }
  assert.deepEqual(helpers.postgresProjection("1016001"), ["dealloc", "stats_reset"])
  assert.equal(helpers.PLAN_COLUMNS.some(({ field }) => field === "queryid_stat_statements"), true)
  const queryId = helpers.PLAN_COLUMNS.find(({ field }) => field === "queryid")
  const lastQueryId = helpers.PLAN_COLUMNS.find(({ field }) => field === "queryid_stat_statements")
  const exact = row("1003001", { queryid: "17" }, "pg_store_plans")
  const vadv = row("1004001", { queryid: 0, queryid_stat_statements: "29" }, "pg_store_plans")
  assert.equal(queryId.available(exact), true)
  assert.equal(queryId.available(vadv), false)
  assert.equal(lastQueryId.available(exact), false)
  assert.equal(lastQueryId.available(vadv), true)
  assert.equal(lastQueryId.render(vadv), "29")
})

test("dense PostgreSQL columns and the Plans tab stay available by section", async () => {
  const statementFields = helpers.STATEMENT_COLUMNS.map((column) => column.field)
  const planFields = helpers.PLAN_COLUMNS.map((column) => column.field)
  for (const field of ["calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second", "shared_blks_read", "wal_bytes", "planning_ms_per_second"]) {
    assert.equal(statementFields.includes(field), true)
  }
  for (const field of ["planid", "queryid", "plan_summary", "calls", "calls_per_second", "execution_ms_per_second", "rows_per_second", "queryid_stat_statements"]) {
    assert.equal(planFields.includes(field), true)
  }
  const source = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /id: "plans"[\s\S]*sections: \["pg_store_plans", "pg_store_plans_info"\]/)
  assert.match(source, /tab\.id === "plans"/)
  assert.match(source, /pg-plans-empty/)
  assert.match(source, /columns\.some\(\(\{ field \}\) => field === order\.column\)/)
  assert.match(source, /detailColumns=\{ACTIVITY_DETAIL_COLUMNS\}/)
  assert.match(source, /allRows\.map\(decoratePostgresIntervalRow\)/)
  assert.match(source, /tableState\([^)]*displayedRows\.length/)
  assert.match(source, /serverSorted=\{dense\}/)
  assert.match(source, /onNearEnd=\{densePageState === "idle" && canLoadMore \? onLoadMore : undefined\}/)
  assert.match(source, /densePageState === "error" \? onRetry : onLoadMore/)
  assert.match(source, /await loadSeries\(hour, section, filters, request\.fields, signal, row\.typeId\)/)
  assert.match(source, /\{ section, fields: \[field\], typeId: row\.typeId \}[\s\S]*fullText: true/)
})

test("every PostgreSQL dense table and lens has an exact meaning-first order", () => {
  const fields = (columns) => columns.map(({ field }) => field)
  assert.deepEqual(fields(helpers.statementColumns("load")), ["query", "datname", "usename", "queryid", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"])
  assert.deepEqual(fields(helpers.statementColumns("per_call")), ["query", "datname", "usename", "queryid", "mean_exec_ms_per_call", "rows_per_call", "blocks_per_call", "calls_per_second"])
  assert.deepEqual(fields(helpers.statementColumns("io")), ["query", "datname", "usename", "queryid", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "shared_blks_written", "local_blks_read", "temp_blks_read", "temp_blks_written"])
  assert.deepEqual(fields(helpers.statementColumns("resources")), ["query", "datname", "usename", "queryid", "wal_bytes", "wal_per_call", "temp_blks_written", "planning_ms_per_second", "plan_time_pct", "calls_per_second", "execution_ms_per_second"])
  assert.deepEqual(fields(helpers.statementColumns("stability")), ["query", "datname", "usename", "queryid", "cv", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second"])
  assert.deepEqual(fields(helpers.planColumns("load")), ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"])
  assert.deepEqual(fields(helpers.planColumns("timing")), ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second", "first_call", "last_call"])
  assert.deepEqual(fields(helpers.planColumns("io")), ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "local_blks_read", "temp_blks_read"])
  assert.deepEqual(fields(helpers.planColumns("identity")), ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "cmd_type", "calls", "calls_per_second"])
  assert.deepEqual(fields(helpers.LOCK_COLUMNS), ["pid", "datname", "usename", "query", "application_name", "lock_target", "lock_relname", "lock_locktype", "lock_mode", "state", "wait_event_type", "wait_event", "waitstart"])
  assert.equal(fields(helpers.lockDetailColumns((key) => key)).at(-1), "blocked_by")
  assert.deepEqual(fields(helpers.DATABASE_COLUMNS), ["datname", "numbackends", "xact_commit", "xact_rollback", "sessions", "tup_returned", "tup_fetched", "tup_inserted", "tup_updated", "tup_deleted", "blks_read", "blks_hit", "blk_read_time", "blk_write_time", "temp_files", "temp_bytes", "conflicts", "deadlocks", "frozen_xid_age"])
  for (const lens of ["load", "per_call", "io", "resources", "stability"]) assert.deepEqual(fields(helpers.statementColumns(lens)).slice(0, 2), ["query", "datname"])
  assert.equal(fields(helpers.DATABASE_COLUMNS).includes("datid"), false)
  for (const columns of [helpers.STATEMENT_COLUMNS, helpers.PLAN_COLUMNS]) {
    for (const internal of ["dbid", "userid", "relids"]) assert.equal(fields(columns).includes(internal), false, internal)
  }
  assert.equal(helpers.statementColumns("load").some(({ field }) => field === "exec_load"), false)
  assert.equal(helpers.planColumns("load").some(({ field }) => field === "exec_load"), false)
  assert.equal(helpers.statementDefaultOrder("load"), "execution_ms_per_second")
  assert.equal(helpers.statementDefaultOrder("per_call"), "calls_per_second")
  assert.equal(helpers.statementDefaultOrder("io"), "shared_blks_read")
  assert.equal(helpers.statementDefaultOrder("resources"), "wal_bytes")
  assert.equal(helpers.statementDefaultOrder("stability"), "calls_per_second")
  assert.equal(helpers.planDefaultOrder("timing"), "calls_per_second")
})

test("verbatim PostgreSQL tokens use the machine value role in Detail", () => {
  const role = (columns, field) => columns.find((column) => column.field === field)?.detailValueRole
  for (const field of ["lock_target", "lock_relname", "lock_locktype", "lock_mode", "state", "wait_event_type", "wait_event"]) {
    assert.equal(role(helpers.LOCK_COLUMNS, field), "machine", field)
  }
  for (const field of ["backend_type", "state", "wait_event_type", "wait_event"]) {
    assert.equal(role(helpers.ACTIVITY_DETAIL_COLUMNS, field), "machine", field)
  }
  assert.notEqual(role(helpers.ACTIVITY_DETAIL_COLUMNS, "query_duration_ms"), "machine")
  assert.equal(role(helpers.planColumns("identity"), "cmd_type"), "machine")
  const vacuum = { logicalName: "pg_stat_progress_vacuum", ordinal: "0", segmentId: "a", timestamp: 1, typeId: "1012006", values: {} }
  const vacuumColumns = helpers.vacuumDetailColumns(vacuum, null)
  assert.equal(role(vacuumColumns, "phase"), "machine")
  for (const field of ["is_autovacuum", "heap_blks_scanned", "delay_time"]) {
    assert.notEqual(role(vacuumColumns, field), "machine", field)
  }
})

test("lock accessibility exposes exact parents, extra blocker PIDs, and prepared waits", () => {
  const t = (key, slots = {}) => key === "pg.locks.prepared_transaction" ? "prepared transaction" : `${key}:${Object.values(slots).join(",")}`
  const row = { logicalName: "pg_locks", ordinal: "1", segmentId: "a", timestamp: 1, typeId: "1011002", values: {
    blocked_by: [10, 20, 25, 0], pid: 30, lock_tree_depth: 3, lock_tree_parent_pid: 10, lock_tree_extra_blockers: [20, 25], lock_tree_waits_on_prepared: true,
  } }
  const label = helpers.lockRowLabel(row, t)
  assert.match(label, /pg\.locks\.row\.waiter:3,10,30/)
  assert.match(label, /pg\.locks\.row\.extra:20, 25/)
  assert.match(label, /pg\.locks\.row\.prepared:/)
  assert.equal(helpers.lockDetailColumns(t).at(-1).render(row), "PID 10, PID 20, PID 25, prepared transaction")
})

test("the Vacuum ledger names risk by phase and never shows a bare OID", () => {
  const stubTime = { timestamp: (ts) => String(ts) }
  const stubT = (key, slots) => slots === undefined ? key : `${key}:${JSON.stringify(slots)}`
  const row = (typeId, values) => ({ logicalName: "pg_stat_progress_vacuum", ordinal: "0", segmentId: "a", timestamp: 5, typeId, values })
  const pg18 = row("1012006", { datid: 42, datname: "app", relid: 73, schemaname: "public", relname: "orders", phase: "truncating heap", pid: 9, is_autovacuum: true, heap_blks_scanned: 10, heap_blks_total: 20, heap_blks_vacuumed: 4, index_vacuum_count: 2, indexes_processed: 1, indexes_total: 3, delay_time: 100 })
  const pg16 = row("1012004", { datid: 42, datname: "app", relid: 73, schemaname: null, relname: null, phase: "scanning heap", pid: 9, is_autovacuum: false, heap_blks_scanned: 1, heap_blks_total: 2, heap_blks_vacuumed: 0, index_vacuum_count: 0, num_dead_tuples: 5, max_dead_tuples: 9 })
  const episodes = new Map()
  const all = helpers.vacuumColumns([pg18], episodes, 5, null, stubTime, "en", stubT)
  assert.equal(all.some(({ field }) => field === "datid" || field === "relid"), false)
  const relationColumn = all.find(({ field }) => field === "datname")
  assert.equal(relationColumn?.render?.(pg18), "app.public.orders")
  const relationColumn16 = helpers.vacuumColumns([pg16], episodes, 5, null, stubTime, "en", stubT).find(({ field }) => field === "datname")
  assert.equal(relationColumn16?.render?.(pg16), "app · relid=73")
  assert.ok(all.some(({ field }) => field === "vacuum_progress"))
  assert.ok(all.some(({ field }) => field === "heap_blks_scanned"))
  const sizeColumn = all.find(({ field }) => field === "heap_blks_scanned")
  assert.doesNotMatch(String(sizeColumn?.render?.(pg18)), /%/)
  assert.ok(all.some(({ field }) => field === "indexes_processed"))
  assert.ok(all.some(({ field }) => field === "delay_time"))
  const old = helpers.vacuumColumns([pg16], episodes, 5, null, stubTime, "en", stubT)
  assert.equal(old.some(({ field }) => field === "indexes_processed" || field === "delay_time"), false)
  assert.equal(all.filter(({ field }) => field !== "pid").every(({ help }) => typeof help === "string"), true)
  const detail18 = helpers.vacuumDetailColumns(pg18, null).map(({ field }) => field)
  assert.equal(detail18.some((field) => field === "datid" || field === "relid"), false)
  for (const field of ["is_autovacuum", "dead_tuple_bytes", "max_dead_tuple_bytes", "num_dead_item_ids", "indexes_total", "delay_time"]) assert.ok(detail18.includes(field), field)
  const detail16 = helpers.vacuumDetailColumns(pg16, null).map(({ field }) => field)
  assert.equal(detail16.some((field) => field === "datid" || field === "relid"), false)
  for (const field of ["num_dead_tuples", "max_dead_tuples"]) assert.ok(detail16.includes(field), field)
  assert.equal(detail16.includes("delay_time") || detail16.includes("dead_tuple_bytes"), false)
})

test("every non-obvious PostgreSQL dense header has exact EN/RU help", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  const stubTime = { timestamp: (ts) => String(ts) }
  const stubT = (key) => key
  const progressRow = { logicalName: "pg_stat_progress_vacuum", ordinal: "0", segmentId: "a", timestamp: 1, typeId: "1012006", values: { datid: 42, datname: "app", relid: 73, schemaname: null, relname: null, phase: "scanning heap", pid: 9 } }
  const progress = [
    ...helpers.vacuumColumns([progressRow], new Map(), 1, null, stubTime, "en", stubT),
    ...helpers.vacuumDetailColumns(progressRow, null),
    ...helpers.vacuumDetailColumns({ ...progressRow, typeId: "1012004" }, null),
    { field: "vacuum_workers", help: "pg.vacuum.workers.help", label: "pg.vacuum.workers.label" },
  ]
  const groups = [
    helpers.ACTIVITY_COLUMNS,
    ...["load", "per_call", "io", "resources", "stability"].map((lens) => helpers.statementColumns(lens)),
    ...["load", "timing", "io", "identity"].map((lens) => helpers.planColumns(lens)),
    helpers.LOCK_COLUMNS,
    helpers.DATABASE_COLUMNS,
    progress,
  ]
  const usedVacuumHelp = new Set()
  for (const columns of groups) for (const column of columns) {
    const intentionallyObvious = column.field === "pid" && [helpers.LOCK_COLUMNS, helpers.ACTIVITY_COLUMNS, progress].includes(columns)
      || column.field === "datname" && [helpers.DATABASE_COLUMNS, helpers.LOCK_COLUMNS, helpers.ACTIVITY_COLUMNS].includes(columns)
      || ["usename", "application_name"].includes(column.field) && [helpers.LOCK_COLUMNS, helpers.ACTIVITY_COLUMNS].includes(columns)
    assert.equal(column.help === undefined, intentionallyObvious, column.field)
    if (column.help === undefined) continue
    assert.equal(Object.hasOwn(english, column.help), true, column.help)
    assert.equal(Object.hasOwn(russian, column.help), true, column.help)
    if (column.help.startsWith("pg.vacuum.")) usedVacuumHelp.add(column.help)
  }
  const postgresViewSource = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  for (const match of postgresViewSource.matchAll(/pg\.vacuum\.load(\.[a-z_]+)?\.help/g)) {
    assert.equal(Object.hasOwn(english, match[0]), true, match[0])
    assert.equal(Object.hasOwn(russian, match[0]), true, match[0])
    usedVacuumHelp.add(match[0])
  }
  const dictionaryVacuumHelp = Object.keys(english).filter((key) => /^pg\.vacuum\.[^.]+(\.[a-z_]+)?\.help$/.test(key)).sort()
  assert.deepEqual([...usedVacuumHelp].sort(), dictionaryVacuumHelp)
})

test("dense numeric columns advertise server sorting and text identities do not", () => {
  const lenses = [
    ...["load", "per_call", "io", "resources", "stability"].map((lens) => [helpers.statementColumns(lens), helpers.statementRequest(lens)]),
    ...["load", "timing", "io", "identity"].map((lens) => [helpers.planColumns(lens), helpers.planRequest(lens)]),
  ]
  for (const [columns, request] of lenses) {
    for (const column of columns) {
      const quantitative = column.field !== "calls" && ["number", "id", "bytes", "milliseconds", "percent", "timestamp"].includes(column.kind)
      assert.equal(column.sortable === true, quantitative, column.field)
      if (quantitative) assert.ok(request.order[column.field]?.length > 0, column.field)
    }
  }
})

test("dense rows retain the physical server order across layouts", () => {
  const oldest = { ...row("1002001", { total_time: 5, query: "old" }), ordinal: "1", timestamp: 100 }
  const middle = { ...row("1002001", { total_time: 7, query: "middle" }), ordinal: "2", timestamp: 100 }
  const newest = { ...row("1002002", { total_exec_time: 9, query: "new" }), ordinal: "3", timestamp: 200 }
  const decorated = [newest, oldest, middle].map(helpers.decoratePostgresIntervalRow)
  assert.deepEqual(decorated.map(({ ordinal }) => ordinal), ["3", "1", "2"])
  assert.deepEqual(decorated.map(({ timestamp }) => timestamp), [200, 100, 100])
})

test("dense table state names interval, filter, physical server order, and total count", () => {
  const dictionary = {
    "pg.table.interval": "Interval {from} to {to}",
    "pg.table.filter": "Filter {pattern}", "pg.table.no_filter": "No filter",
    "pg.table.order": "Order {semantic}; {physical}; {direction}", "pg.table.order_default": "Default order",
    "pg.table.desc": "descending", "pg.table.shown": "Loaded {returned} of {eligible}",
    "pg.table.interval_unavailable": "No interval",
    "pg.field.execution_ms_per_second.label": "Execution ms/s",
  }
  const t = (key, slots = {}) => Object.entries(slots).reduce((text, [name, value]) => text.replace(`{${name}}`, value), dictionary[key] ?? key)
  const markup = renderToStaticMarkup(helpers.tableState({
    logicalName: "pg_stat_statements", eligible: 4873, returned: 200,
    hasMore: true, truncated: true, nextCursor: "opaque", pageSize: 200,
    orderBy: ["total_exec_time"], orderDirection: "desc",
    from: 1_800_000_000_000_000, to: 1_800_000_010_000_000,
  }, 200, "vacuum*", { column: "execution_ms_per_second", descending: true }, "en", t))
  assert.match(markup, /Interval .* to /)
  assert.doesNotMatch(markup, /GMT|UTC/)
  assert.match(markup, /Filter vacuum\*/)
  assert.match(markup, /Execution ms\/s; total_exec_time; descending/)
  assert.match(markup, /Loaded 200 of 4,873/)
})

test("later pages report the accumulated loaded count without inflating the eligible total", () => {
  const dictionary = {
    "pg.table.interval": "Interval {from} to {to}",
    "pg.table.interval_unavailable": "No interval",
    "pg.table.filter": "Filter {pattern}", "pg.table.no_filter": "No filter",
    "pg.table.order": "Order {semantic}; {physical}; {direction}", "pg.table.order_default": "Default order",
    "pg.table.desc": "descending", "pg.table.shown": "Loaded {returned} of {eligible}",
    "pg.field.execution_ms_per_second.label": "Execution ms/s",
  }
  const t = (key, slots = {}) => Object.entries(slots).reduce((text, [name, value]) => text.replace(`{${name}}`, value), dictionary[key] ?? key)
  const metadata = {
    logicalName: "pg_stat_statements", eligible: 4873, returned: 200,
    hasMore: true, truncated: true, nextCursor: "opaque", pageSize: 200,
    orderBy: ["total_time", "total_exec_time"], orderDirection: "desc", from: 1_000_000, to: 4_000_000,
  }
  const markup = renderToStaticMarkup(helpers.tableState(metadata, 400, "", { column: "execution_ms_per_second", descending: true }, "en", t))
  assert.match(markup, /Loaded 400 of 4,873/)
  assert.match(markup, /total_time, total_exec_time/)
  assert.doesNotMatch(markup, /Loaded 200/)
})

test("dense paging resets, ignores stale work, and preserves retry state", async () => {
  const source = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(source, /const generation = \+\+snapshotGeneration\.current/)
  assert.match(source, /const stale = \(\) => controller\.signal\.aborted \|\| generation !== snapshotGeneration\.current/)
  assert.match(source, /const requestOrder = visibleSource === "processes"[\s\S]*const snapshotTarget = [\s\S]*snapshotTargetKey\(snapshotGroups, cursor, cgroupTargetGroups, requestOrder, denseOptions\)/)
  assert.match(source, /const retainsDenseRows = denseRequest !== undefined[\s\S]*currentSnapshot\.cursor === cursor[\s\S]*currentSnapshot\.denseSection === denseRequest\.section/)
  assert.match(source, /const retainsCurrentView = snapshotTarget !== null && currentSnapshot\.owner === foregroundKey/)
  assert.match(source, /snapshotRowsVisible\(currentSnapshot\.target, snapshotTarget, currentSnapshot\.owner, foregroundKey, retainsDenseRows\)/)
  assert.match(source, /some\(\(row\) => denseRequest\.group === undefined \|\| row\.relation\?\.group === denseRequest\.group\)/)
  assert.doesNotMatch(source, /clearCgroupSnapshotRows/)
  const cgroupLoad = source.slice(source.indexOf("const loadOrdinarySnapshot"), source.indexOf("const timer = setTimeout"))
  assert.match(cgroupLoad, /const exact = await Promise\.all/)
  assert.match(cgroupLoad, /\.catch\(\(reason: unknown\) => \{[\s\S]*!stale\(\) && cgroupSnapshotKey\.current === planKey/)
  assert.match(cgroupLoad, /filtered \$\{request\.section\} snapshot failed/)
  assert.match(cgroupLoad, /return EMPTY_DATA/)
  assert.match(cgroupLoad, /exact\.reduce\(\(current, incoming\) => mergeSnapshotData\(current, incoming\), primary\)/)
  assert.match(source, /let inFlight = false/)
  assert.match(source, /if \(inFlight \|\| stale\(\)\)/)
  assert.match(source, /pageCursor === undefined[\s\S]*mergeSnapshotData\(companion[\s\S]*mergeSnapshotData\(current/)
  assert.match(source, /current\.target === snapshotTarget \? current\.data : EMPTY_DATA/)
  assert.match(source, /loaded\(incoming, null, ordinaryGroups\.length !== 0\)/)
  assert.match(source, /companionPending: false,[\s\S]*companion === EMPTY_DATA \? current\.data : mergeSnapshotData/)
  assert.match(source, /currentSnapshot\.target === snapshotTarget \|\| cursorState === "loading"/)
  assert.match(source, /else if \(!companionPending\) \{\s*ticksPerSecondCache\.current = null/)
  assert.match(source, /\?\? \(companionPending && hour !== null/)
  assert.match(source, /action\.failed = pageCursor[\s\S]*setDensePageState\("error"\)/)
  assert.match(source, /return \(\) => \{ clearTimeout\(timer\); controller\.abort\(\) \}/)
  assert.match(source, /\}, \[finishRefresh, foregroundKey, hour, snapshotReloadVersion, snapshotTarget\]\)/)
  assert.match(source, /const refreshReady = !loading && cursorState === "ready" && densePageState !== "loading"/)
  assert.match(source, /disabled=\{refreshing \|\| !refreshReady\}/)
  assert.match(source, /typeIds: \[context\.typeId\]/)
  assert.match(source, /Object\.fromEntries\(context\.identity\)/)
  assert.match(source, /denseMetadata\?\.hasMore === true \? denseMetadata\.nextCursor/)
  assert.match(source, /action\.load\(action\.failed\)/)
})

test("an exact finding never opens PostgreSQL detail without an explicit row selection", () => {
  const first = { ...row("1001001", { pid: 7, backend_start: "100" }), ordinal: "0" }
  const focus = { ...row("1001001", { pid: 8, backend_start: "200" }), ordinal: "1" }
  assert.equal(helpers.selectedEntity([first, focus], null, "pg_stat_activity"), null)
  assert.equal(helpers.selectedEntity([first, focus], first, "pg_stat_activity"), first)
})

test("an off-page finding is contextualized without appending to the ranked page", async () => {
  const source = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /contextualRows\(ranked, activeContext, exactFocus\)/)
  assert.doesNotMatch(source, /\[\.\.\.ranked,[^\]]*focus/)
  assert.match(source, /contextLabel=\{activeContext\?\.label\}/)
})

test("PostgreSQL detail never opens a row that was not selected", () => {
  const first = { ...row("1002006", { queryid: 7, userid: 8, dbid: 9, toplevel: true }), ordinal: "0" }
  assert.equal(helpers.selectedEntity([first], null, "pg_stat_statements"), null)
  assert.equal(helpers.selectedEntity([], first, "pg_stat_statements"), null)
})

test("an exact locator preview never inherits settled zero page counts", () => {
  const dictionary = {
    "pg.table.focus_loading": "Exact result; page loading",
    "pg.table.focus_outside": "Exact result outside page", "pg.table.focus_exact": "Exact focused result",
    "pg.table.interval": "Interval {from} to {to}", "pg.table.interval_unavailable": "No interval",
    "pg.table.filter": "Filter {pattern}", "pg.table.no_filter": "No filter", "pg.table.order_default": "Default",
    "pg.table.shown": "Loaded {returned} of {eligible}",
  }
  const t = (key, slots = {}) => Object.entries(slots).reduce((text, [name, value]) => text.replace(`{${name}}`, value), dictionary[key] ?? key)
  const loading = renderToStaticMarkup(helpers.tableState(undefined, 0, "", undefined, "en", t, undefined, "loading"))
  assert.match(loading, /Exact result; page loading/)
  assert.match(loading, /Exact focused result/)
  assert.doesNotMatch(loading, /Loaded 0 of 0/)

  const settled = renderToStaticMarkup(helpers.tableState({
    logicalName: "pg_stat_stat_statements", eligible: 1, returned: 1, hasMore: false, truncated: false,
    nextCursor: null, pageSize: 200, orderBy: ["total_exec_time"], orderDirection: "desc", from: 1, to: 2,
  }, 1, "", undefined, "en", t))
  assert.match(settled, /Loaded 1 of 1/)
})

test("database totals omit PostgreSQL's shared-object statistics row", () => {
  const shared = { ...row("1003001", { datid: 0 }), logicalName: "pg_stat_database" }
  const database = { ...row("1003001", { datid: 16_384 }), logicalName: "pg_stat_database" }
  assert.equal(helpers.postgresDatabaseCount([shared, database]), 1)
})
