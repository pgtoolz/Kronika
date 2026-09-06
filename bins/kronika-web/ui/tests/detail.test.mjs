import assert from "node:assert/strict"
import test from "node:test"

import { importFile, registryPlugin } from "./import-module.mjs"

const detail = await importFile("../src/detail.tsx", { plugins: [registryPlugin([])] })

function row(timestamp, values, segmentId = "segment-a", ordinal = "0") {
  return { segmentId, typeId: "1100001", ordinal, timestamp, values }
}

test("a counter is drawn as the rate between two readings", () => {
  const series = detail.processLensHistory([
    row(3_000_000, { pid: 77, starttime: 10, stime: 30 }, "segment-b", "2"),
    row(1_000_000, { pid: 77, starttime: 10, stime: 10 }, "segment-a", "0"),
    row(2_000_000, { pid: 77, starttime: 10, stime: 14 }, "segment-a", "1"),
  ], "cpu")

  assert.deepEqual(series.map((item) => item.field), [
    "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "minflt", "majflt",
  ])
  assert.deepEqual(series[1].points.map((point) => point.value), [null, 4, 16])
})

test("an explicit null counter resets the next rate", () => {
  const series = detail.processLensHistory([
    row(1_000_000, { pid: 77, starttime: 10, stime: 10 }, "segment-a", "0"),
    row(2_000_000, { pid: 77, starttime: 10, stime: null }, "segment-a", "1"),
    row(3_000_000, { pid: 77, starttime: 10, stime: 30 }, "segment-b", "2"),
    row(4_000_000, { pid: 77, starttime: 10, stime: 34 }, "segment-b", "3"),
  ], "cpu")

  assert.deepEqual(series[1].points.map((point) => point.value), [null, null, null, 4])
})

test("a row that does not own the counter neither creates a null nor resets the rate", () => {
  const series = detail.processLensHistory([
    row(1_000_000, { pid: 77, starttime: 10, stime: 10 }, "segment-a", "0"),
    row(2_000_000, { pid: 77, starttime: 10, utime: 99 }, "segment-a", "1"),
    row(3_000_000, { pid: 77, starttime: 10, stime: 30 }, "segment-b", "2"),
  ], "cpu")

  assert.deepEqual(series[1].points.map((point) => [point.timestamp, point.value]), [[1_000_000, null], [3_000_000, 10]])
})

test("null values split rendered history runs while later zero stays numeric", () => {
  const [series] = detail.processLensHistory([
    row(10, { pid: 77, starttime: 10, rmem_kb: 1 }),
    row(20, { pid: 77, starttime: 10, rmem_kb: null }),
    row(30, { pid: 77, starttime: 10, rmem_kb: 0 }),
  ], "memory")

  assert.equal(series.points[0].segmentId, series.points[1].segmentId)
  assert.equal(series.points[1].segmentId, series.points[2].segmentId)
  assert.equal(series.points[2].value, 0)
})

test("chart presentation normalizes process counters to the unit shown on its axis", () => {
  const source = { field: "utime", key: "col.utime", kind: "cores", counter: true, points: [
    { segmentId: "segment-a", timestamp: 10, value: null },
    { segmentId: "segment-a", timestamp: 20, value: 250 },
    { segmentId: "segment-a", timestamp: 30, value: 0 },
  ] }

  assert.deepEqual(detail.processChartPoints(source, 100), [
    { segmentId: "segment-a", timestamp: 10, value: null },
    { segmentId: "segment-a", timestamp: 20, value: 2.5 },
    { segmentId: "segment-a", timestamp: 30, value: 0 },
  ])
  assert.equal(detail.processChartPoints(source, null), source.points)
  assert.equal(detail.processChartUnit("cores", () => " cores", 100), "cores")
  assert.equal(detail.processChartUnit("cores", (key) => key === "unit.per_second" ? "/s" : " cores", null), "ticks/s")
  assert.equal(detail.processChartUnit("bytes", () => "/s", 100), "B/s")
  assert.equal(detail.processChartUnit("rate", () => "/s", 100), "#/s")
})

test("process detail mounts one selected chart and exposes metric actions", async () => {
  const source = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"))

  assert.match(source, /className="[^"]*overflow-x-auto[^"]*" role="group"/)
  assert.match(source, /const selectableHistory = availableHistory\.length === 0 \? history : availableHistory/)
  assert.match(source, /aria-pressed=\{series\.field === selectedHistory\?\.field\}/)
  assert.match(source, /data-testid=\{`process-history-metric-\$\{series\.field\}`\}/)
  assert.doesNotMatch(source, /<TimeTicks/)
  assert.equal(source.match(/<SeriesChart/g)?.length, 1)
})

test("process details expose each user identity exactly once in every lens", () => {
  const process = row(1, { pid: 77, uid: 26, euid: 27, user: "postgres", effective_user: "worker", utime: 10, rmem_kb: 20, read_bytes: 30 })
  for (const lens of ["generic", "cpu", "memory", "disk"]) {
    const ids = detail.processDetailFields(lens, process).map(({ id }) => id)
    assert.equal(ids.filter((id) => id === "user").length, 1, lens)
    assert.equal(ids.filter((id) => id === "effective_user").length, 1, lens)
    assert.equal(new Set(ids).size, ids.length, lens)
  }
})

test("verbatim process state uses the machine value role", () => {
  assert.equal(detail.processDetailValueRole({ kind: "state" }), "machine")
  assert.equal(detail.processDetailValueRole({ kind: "percent" }), "semantic")
})

test("scheduler references stay in CPU detail while temporal fault metrics stay chartable", async () => {
  for (const field of ["nice", "prio", "rtprio"]) assert.equal(detail.PROCESS_HISTORY_FIELDS.includes(field), false, field)
  assert.equal(detail.PROCESS_HISTORY_FIELDS.includes("majflt"), true)
  const processSource = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8"))
  const cpuFields = /cpu:\s*\[([\s\S]*?)\],\s*memory:/.exec(processSource)?.[1] ?? ""
  for (const field of ["nice", "prio", "rtprio"]) assert.match(cpuFields, new RegExp(`"${field}"`), field)
})

test("process history requests project PID without process start time", async () => {
  assert.equal(detail.PROCESS_HISTORY_FIELDS[0], "pid")
  assert.equal(detail.PROCESS_HISTORY_FIELDS.includes("starttime"), false)
  const source = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/app.tsx", import.meta.url), "utf8"))
  assert.match(source, /loadSeries\(hour, "os_process", \{ pid: selectedPid \}, PROCESS_HISTORY_FIELDS/)
  assert.doesNotMatch(source, /selectedStart|starttime: selectedStart/)
})

test("linked Activity detail shows elapsed values instead of repeatable absolute starts", async () => {
  const source = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/detail-activity.tsx", import.meta.url), "utf8"))
  const fields = /const ACTIVITY_FIELDS = \[([\s\S]*?)\] as const/.exec(source)?.[1] ?? ""
  for (const field of ["backend_start", "xact_start", "query_start", "state_change"]) assert.doesNotMatch(fields, new RegExp(field))
  for (const field of ["backend_age_ms", "transaction_duration_ms", "query_duration_ms", "state_duration_ms"]) assert.match(source, new RegExp(`\\["${field}"`))
  assert.match(source, /value=\{humanDuration\(elapsed, locale\)\}/)
})

test("Process detail delegates Escape and focus return to the shared Inspector", async () => {
  const fs = await import("node:fs/promises")
  const [source, inspector] = await Promise.all([
    fs.readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/inspector.tsx", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(source, /useDetailDismiss|addEventListener\("keydown"/)
  assert.match(inspector, /event\.key === "Escape"/)
  assert.match(inspector, /opener\.current\.focus\(\{ preventScroll: true \}\)/)
})

test("a recorded PostgreSQL backend is its own Inspector panel, not a tail under the process facts", async () => {
  const fs = await import("node:fs/promises")
  const [dock, panel, app, inspector, address] = await Promise.all([
    fs.readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/detail-activity.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/inspector.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/address.ts", import.meta.url), "utf8"),
  ])
  // The facts moved whole: the dock keeps none of them.
  assert.doesNotMatch(dock, /pg-exact-query|ACTIVITY_FIELDS|ACTIVITY_DURATIONS|detail\.pg_pid/)
  assert.match(panel, /data-testid="process-activity-panel"/)
  assert.match(panel, /pg-exact-query/)
  // The tab exists only when a backend was recorded under the selected PID,
  // and it arrives through the relation portal, not through a shell prop.
  assert.match(app, /joinedActivity\.row !== null && <InspectorRelatedPortal id="pg_stat_activity"/)
  assert.doesNotMatch(app, /related=\{/)
  assert.match(inspector, /registerRelated/)
  assert.match(inspector, /data-testid=\{`inspector-tab-\$\{tab\.id\}`\}/)
  // Back must return to the panel it left, so the panel is addressed.
  const panels = /INSPECTOR_PANELS = \[([^\]]*)\]/.exec(address)?.[1] ?? ""
  for (const token of ["chart", "detail", "pg_stat_activity", "os_process", "pg_stat_statements", "pg_store_plans"]) {
    assert.match(panels, new RegExp(`"${token}"`))
  }
  assert.match(address, /address\.panel !== null && address\.panel !== "detail"/)
})

test("a relation panel keeps the Inspector open rather than closing it on its own tab", async () => {
  const app = await import("node:fs/promises").then((fs) => fs.readFile(new URL("../src/app.tsx", import.meta.url), "utf8"))
  const open = /const inspectorOpen = ([^\n]*)/.exec(app)?.[1] ?? ""
  assert.doesNotMatch(open, /inspectorPanel === "detail"/)
  assert.match(open, /inspectorPanel !== null && detailAvailable/)
})

test("a backend's OS process is its own Inspector panel, fetched by PID at the cursor", async () => {
  const fs = await import("node:fs/promises")
  const [view, panel, inspector] = await Promise.all([
    fs.readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/detail-process.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/inspector.tsx", import.meta.url), "utf8"),
  ])
  // The tab is offered whenever the backend carries a PID; the body answers
  // with the recorded row, a loading line, or an honest missing line.
  assert.match(view, /backendPid !== null && <InspectorRelatedPortal id="os_process"/)
  assert.match(view, /filters: \{ pid: String\(backendPid\) \}/)
  assert.match(panel, /data-testid="backend-process-panel"/)
  assert.match(panel, /pg\.related\.process_missing/)
  // A tab's owner going away takes only its own tab with it.
  assert.match(inspector, /relatedOwners\.current\.get\(id\) !== identity/)
})

test("statements and plans read each other as peer panels under one identity expression", async () => {
  const fs = await import("node:fs/promises")
  const [view, panels, api] = await Promise.all([
    fs.readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/detail-plans.tsx", import.meta.url), "utf8"),
    fs.readFile(new URL("../src/api.ts", import.meta.url), "utf8"),
  ])
  // Each side is offered exactly when the existing header jump is: the same
  // navigation target names the identity, so the tab and the jump agree.
  assert.match(view, /statementTarget !== null && onRelated !== undefined && <InspectorRelatedPortal id="pg_store_plans"/)
  assert.match(view, /planTarget !== null && onRelated !== undefined && <InspectorRelatedPortal id="pg_stat_statements"/)
  // The rows come from the newest snapshot at or before the cursor, through
  // the same search expressions the drills use.
  assert.match(panels, /loadRelatedPlanRows\(segments, cursor, expression/)
  assert.match(panels, /loadRelatedStatementRow\(segments, cursor, target\.expression/)
  assert.match(panels, /pg\.related\.plans_missing/)
  assert.match(panels, /pg\.related\.statement_missing/)
  // first_match is pinned server-side to a text-only projection; the panel
  // wants counters, so it searches an ordinary one-row page instead.
  assert.match(api, /loadSnapshot\(group\.anchor\.id, at, \[request\], signal, undefined, \{ fullText: true, search \}\)/)
})
