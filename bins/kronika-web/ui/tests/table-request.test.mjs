import assert from "node:assert/strict"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import test from "node:test"

import { build } from "esbuild"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  external: ["react", "react-dom", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  stdin: {
    contents: 'export * from "../src/table-request.tsx"; export { EntityTable, entityRowHeight } from "../src/entity-table.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const request = loaded.exports

const copy = {
  "table.loading": "Loading rows…",
  "table.loading_retained": "Loading rows… Previous rows remain visible.",
  "table.load_failed": "Could not load rows.",
  "table.load_failed_retained": "Could not load rows. Previous rows remain visible.",
  "filter.search_failed": "Could not search rows.",
}
const t = (key) => copy[key] ?? key
const row = { logicalName: "os_process", ordinal: "1", segmentId: "a", timestamp: 1, typeId: "1", values: { pid: 42 } }
const table = (requestPhase, rows = [], searchRequest = { phase: "idle" }) => renderToStaticMarkup(createElement(request.EntityTable, {
  columns: [{ field: "pid", kind: "id", label: "PID", width: 80 }],
  contentSized: true,
  empty: "No process data at the selected time",
  label: "Processes",
  locale: "en",
  requestPhase,
  rows,
  searchRequest,
  status: createElement("strong", null, `Loaded ${rows.length} of ${rows.length}`),
  t,
  testId: "process-table",
}))
const placeholder = (phase) => renderToStaticMarkup(createElement(request.TableRequestPlaceholder, {
  empty: "No Host metrics at the selected time",
  phase,
  t,
  testId: "system-request-state",
}))

test("keyed snapshot requests make a new target pending and ignore stale settlements", () => {
  let state = request.READY_SNAPSHOT_REQUEST
  assert.equal(request.visibleSnapshotRequest(state, "target-a"), "loading")
  state = request.beginSnapshotRequest("target-a")
  assert.equal(request.visibleSnapshotRequest(state, "target-a"), "loading")
  assert.deepEqual(request.settleSnapshotRequest(state, "target-b", "ready"), state)
  state = request.settleSnapshotRequest(state, "target-a", "ready")
  assert.equal(request.visibleSnapshotRequest(state, "target-a"), "ready")
  assert.equal(request.visibleSnapshotRequest(state, "target-b"), "loading")

  state = request.beginSnapshotRequest("target-b")
  assert.deepEqual(request.settleSnapshotRequest(state, "target-a", "missing"), state)
  state = request.settleSnapshotRequest(state, "target-b", "missing")
  assert.equal(request.visibleSnapshotRequest(state, "target-b"), "missing")
  assert.equal(request.tableRequestPhase("missing", "idle"), "error")
  assert.equal(request.tableRequestPhase("ready", "loading"), "pending")
  assert.equal(request.tableRequestPhase("ready", "idle"), "ready")
})

test("entity row stride follows the active pointer target", () => {
  assert.equal(request.entityRowHeight(false), 24)
  assert.equal(request.entityRowHeight(true), 44)
})

test("the same table owner keeps its last success until a replacement settles", () => {
  assert.equal(request.snapshotRowsVisible("target-a", "target-b", "hour:processes:cpu", "hour:processes:cpu", false), true)
  assert.equal(request.snapshotRowsVisible("target-a", "target-b", "hour:processes:cpu", "hour:processes:disk", false), false)
  assert.equal(request.snapshotRowsVisible("target-a", null, "hour:processes:cpu", "hour:processes:cpu", false), false)
  assert.equal(request.snapshotRowsVisible("target-a", "target-b", "old", "new", true), true)
  assert.equal(request.snapshotRowsVisible("target-b", "target-b", "old", "new", false), true)
})

test("pending, failure, and successful empty table states stay distinct", () => {
  const pendingState = request.tableRequestState("pending", false)
  const pending = renderToStaticMarkup(createElement(request.TableRequestMessage, { request: pendingState, t }))
  assert.match(pending, /role="status"/)
  assert.match(pending, /aria-live="polite"/)
  assert.match(pending, /<progress/)
  assert.match(pending, /Loading rows/)
  assert.doesNotMatch(pending, /No process data/)

  const failedState = request.tableRequestState("error", false)
  const failed = renderToStaticMarkup(createElement(request.TableRequestMessage, { request: failedState, t }))
  assert.match(failed, /role="alert"/)
  assert.match(failed, /Could not load rows/)
  assert.doesNotMatch(failed, /No process data/)

  const readyState = request.tableRequestState("ready", false)
  const ready = renderToStaticMarkup(createElement(request.TableRequestMessage, { request: readyState, t }))
  assert.deepEqual(readyState, { phase: "ready" })
  assert.equal(ready, "")
})

test("compact request placeholders reserve one frame and show empty copy only when ready", () => {
  const pending = placeholder("pending")
  assert.match(pending, /aria-busy="true"/)
  assert.match(pending, /h-\[72px\]/)
  assert.match(pending, /aria-live="polite"[^>]*role="status"/)
  assert.match(pending, /<progress aria-hidden="true"/)
  assert.doesNotMatch(pending, /No Host metrics/)

  const failed = placeholder("error")
  assert.match(failed, /aria-busy="false"/)
  assert.match(failed, /h-\[72px\]/)
  assert.match(failed, /role="alert"/)
  assert.doesNotMatch(failed, /No Host metrics|<progress/)

  const ready = placeholder("ready")
  assert.match(ready, /aria-busy="false"/)
  assert.match(ready, /h-\[72px\]/)
  assert.match(ready, /No Host metrics at the selected time/)
  assert.doesNotMatch(ready, /role="status"|role="alert"|<progress/)
})

test("a content-sized empty table keeps one compact frame while loading", () => {
  const pending = table("pending")
  const ready = table("ready")
  const height = (markup) => markup.match(/aria-label="Processes"[^>]*style="height:([^"]+)"/)?.[1]

  assert.equal(height(pending), "72px")
  assert.equal(height(ready), "72px")
  assert.match(pending, /data-testid="table-skeleton"/)
  assert.doesNotMatch(pending, /No process data at the selected time/)
  assert.match(ready, /No process data at the selected time/)
})

test("retained rows are labelled during replacement and failure", () => {
  for (const phase of ["pending", "error"]) {
    const state = request.tableRequestState(phase, true)
    const markup = renderToStaticMarkup(createElement(request.TableRequestMessage, { request: state, t }))
    assert.match(markup, /Previous rows remain visible/)
    assert.match(markup, phase === "pending" ? /role="status"/ : /role="alert"/)
  }
})

test("the actual table surface separates pending, empty, failure, and retained rows", () => {
  const pending = table("pending")
  assert.match(pending, /<section aria-busy="false"/)
  assert.match(pending, /aria-busy="true"[^>]*role="table"/)
  assert.match(pending, /aria-live="polite"[^>]*role="status"/)
  assert.match(pending, /<progress aria-hidden="true"/)
  assert.match(pending, /data-testid="table-skeleton"/)
  assert.doesNotMatch(pending, /No process data|Loaded 0 of 0/)
  assert.equal((pending.match(/role="status"/g) ?? []).length, 1)

  const empty = table("ready")
  assert.match(empty, /<section aria-busy="false"/)
  assert.match(empty, /aria-busy="false"[^>]*role="table"/)
  assert.match(empty, /No process data at the selected time/)
  assert.match(empty, /Loaded 0 of 0/)
  assert.doesNotMatch(empty, /table-skeleton|role="status"/)

  const failed = table("error")
  assert.match(failed, /role="alert"/)
  assert.match(failed, /Could not load rows/)
  assert.doesNotMatch(failed, /No process data|Loaded 0 of 0/)
  assert.equal((failed.match(/Could not load rows/g) ?? []).length, 1)

  for (const phase of ["pending", "error"]) {
    const retained = table(phase, [row])
    assert.match(retained, /Previous rows remain visible/)
    assert.doesNotMatch(retained, /Loaded 1 of 1/)
  }
})

test("a newly pending view takes precedence over a stale search failure", () => {
  const markup = table("pending", [], { phase: "error", retained: false, surface: "os_process" })
  assert.match(markup, /Loading rows/)
  assert.match(markup, /data-testid="table-skeleton"/)
  assert.doesNotMatch(markup, /Could not search rows/)
})
