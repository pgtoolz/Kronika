import assert from "node:assert/strict"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import test from "node:test"
import { build } from "esbuild"
import { registryPlugin } from "./import-module.mjs"

const fields = ["pid", "datname", "usename", "query", "query_start", "xact_start", "application_name", "client_addr", "state", "wait_event_type", "wait_event", "backend_type"]
const compiled = await build({
  bundle: true, external: ["react", "react-dom", "react/jsx-runtime"], format: "cjs", platform: "node",
  plugins: [{ name: "fixed-test-viewport", setup(builder) {
    builder.onResolve({ filter: /^@tanstack\/react-virtual$/ }, () => ({ path: "viewport", namespace: "test-viewport" }))
    builder.onLoad({ filter: /.*/, namespace: "test-viewport" }, () => ({ contents: `export function useVirtualizer(options) { return { measure() {}, scrollToIndex() {}, getTotalSize: () => options.count * 24, getVirtualItems: () => Array.from({length: Math.min(10, options.count)}, (_, index) => ({index, start: index * 24, size: 24})) } }` }))
  } }, registryPlugin([{ typeId: "1001001", logicalName: "pg_stat_activity", identity: ["pid"], columns: fields }])],
  stdin: { contents: 'export { PostgresView, ACTIVITY_COLUMNS, activityColumns } from "../src/postgres-view.tsx"; export { EntityTable } from "../src/entity-table.tsx"', loader: "tsx", resolveDir: dirname(fileURLToPath(import.meta.url)) }, write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const { PostgresView, ACTIVITY_COLUMNS, activityColumns, EntityTable } = loaded.exports
const noop = () => {}
const at = 100_000_000
const row = { logicalName: "pg_stat_activity", ordinal: "0", segmentId: "1", timestamp: at, typeId: "1001001", values: {
  pid: 1, datname: "db", usename: "role", query: "select 1", query_start: 1, xact_start: 1, application_name: "app", client_addr: "127.0.0.1", state: "active", wait_event_type: "Lock", wait_event: "transactionid", backend_type: "client backend",
} }
const props = {
  context: null, densePageState: "idle", requestPhase: "ready", pattern: "", cursor: at,
  data: { sections: { pg_stat_activity: [row] }, snapshotRows: [], rateColumns: {}, availableSections: ["pg_stat_activity"], findings: [], health: [], lanePoints: [] },
  environment: "machine", focus: null, focusFinding: null, historyRevision: 0, hour: 0, locale: "en", navigationTimestamps: [],
  section: "activity", selectedKey: null, selectedLane: "health", statementLens: "load", monitorQueries: false, statementScope: { scope: "all", forced: false }, planLens: "load", segments: [], relationFilters: {}, relationLens: "access", relationLevel: "object", relationSelectedKey: null,
  t: (key) => key,
  ...Object.fromEntries(["LoadMore", "Retry", "Order", "Pattern", "Cursor", "ContextClear", "Finding", "OpenChart", "Related", "Section", "RelationLens", "RelationNavigate", "RelationSelectedKey", "SelectedLane", "SelectedKey", "MonitorQueries", "StatementLens", "PlanLens"].map((key) => [`on${key}`, noop])),
}

test("Activity renders the requested order on every ordinary column", () => {
  for (const column of ACTIVITY_COLUMNS) for (const descending of [false, true]) {
    const html = renderToStaticMarkup(createElement(PostgresView, { ...props, order: { column: column.field, descending } }))
    const headers = [...html.matchAll(/<div[^>]*role="columnheader"[^>]*>([\s\S]*?)<\/button>/g)].map((match) => match[1])
    const selected = headers.find((header) => header.includes(`>${column.label}</span>`))
    assert.ok(selected?.includes(descending ? "↓" : "↑"), `${column.field} ${descending ? "DESC" : "ASC"}`)
  }
})

function orderedPids(rows, field, descending, locale = "en") {
  const html = renderToStaticMarkup(createElement(EntityTable, { columns: activityColumns(true), rows, order: { column: field, descending }, locale, t: (key) => key, searchSurface: "pg_stat_activity", label: "Activity", empty: "empty" }))
  return [...html.matchAll(/class="entity-row[^]*?role="cell"[^>]*>([^]*?)<\/div>/g)].map((match) => Number(match[1].replace(/<[^>]*>/g, "")))
}

test("Activity sorts numeric durations and missing values before the viewport with stable PID ties", () => {
  const rows = [[17, 1000], [19, null], [13, 10000], [5, null], [11, 1000], [7, 0]].map(([pid, duration]) => ({ ...row, ordinal: String(pid), values: { ...row.values, pid, query_duration_ms: duration, transaction_duration_ms: duration } }))
  for (const field of ["query_duration_ms", "transaction_duration_ms"]) for (const locale of ["ru", "en"]) {
    assert.deepEqual(orderedPids(rows, field, false, locale), [7, 11, 17, 13, 5, 19])
    assert.deepEqual(orderedPids(rows, field, true, locale), [13, 11, 17, 7, 5, 19])
  }
  const long = Array.from({ length: 250 }, (_, index) => ({ ...row, ordinal: String(index), values: { ...row.values, pid: index + 1 } }))
  assert.deepEqual(orderedPids(long, "pid", true), [250, 249, 248, 247, 246, 245, 244, 243, 242, 241])
})

test("all Activity text headers use raw text consistently in RU and EN", () => {
  for (const field of ["datname", "usename", "query", "application_name", "client_addr", "state", "wait_event_type", "wait_event", "backend_type"]) {
    const rows = [[19, null], [7, "a10"], [13, "a2"], [5, "A"], [11, "a10"]].map(([pid, text]) => ({ ...row, ordinal: String(pid), values: { ...row.values, pid, [field]: text } }))
    for (const locale of ["en", "ru"]) {
      assert.deepEqual(orderedPids(rows, field, false, locale), [5, 7, 11, 13, 19], field)
      assert.deepEqual(orderedPids(rows, field, true, locale), [13, 7, 11, 5, 19], field)
    }
  }
  const addresses = [[17, ""], [11, null], [5, "::1"], [7, "127.0.0.1"]].map(([pid, address]) => ({ ...row, ordinal: String(pid), values: { ...row.values, pid, client_addr: address } }))
  assert.deepEqual(orderedPids(addresses, "client_addr", false), [7, 5, 11, 17])
  assert.deepEqual(orderedPids(addresses, "client_addr", true), [5, 7, 11, 17])
})


test("a filtered captured Locks graph uses ordinary empty results, not missing graph copy", () => {
  const graph = { ...row, logicalName: "pg_locks", typeId: "1011002", values: { pid: 42, blocked_by: [7], datname: "recorded" } }
  const html = renderToStaticMarkup(createElement(PostgresView, {
    ...props, section: "locks", pattern: "no-match-at-all",
    data: { ...props.data, sections: { pg_locks: [graph] }, availableSections: ["pg_locks"], lanePoints: [{ segmentId: "1", lane: "pg_lock_waiting", timestamp: at, value: 1 }] },
  }))
  assert.match(html, /pg.locks.recorded_at/)
  assert.doesNotMatch(html, /pg.locks.not_recorded/)
  assert.match(html, /filter.none/)
})
