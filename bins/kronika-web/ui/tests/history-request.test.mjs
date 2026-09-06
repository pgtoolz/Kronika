import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const history = await importModule('export { beginHistory, failHistory, finishHistory, visibleHistory } from "../src/history-request.ts"')

test("history requests keep a same-target result and never show another target", () => {
  const ready = { key: "hour:pid:42", status: "ready", value: [1, 2] }
  assert.deepEqual(history.beginHistory(ready, "hour:pid:42"), {
    key: "hour:pid:42", status: "loading", value: [1, 2],
  })
  assert.deepEqual(history.beginHistory(ready, "hour:pid:84"), {
    key: "hour:pid:84", status: "loading", value: null,
  })
  assert.deepEqual(history.visibleHistory(ready, "hour:pid:84"), { status: "loading", value: null })
})

test("history failures remain distinct and late responses cannot replace the target", () => {
  const loading = { key: "cpu", status: "loading", value: [3] }
  assert.deepEqual(history.failHistory(loading, "cpu"), { key: "cpu", status: "error", value: [3] })
  assert.deepEqual(history.finishHistory(loading, "memory", [9]), loading)
  assert.deepEqual(history.failHistory(loading, "memory"), loading)
  assert.deepEqual(history.finishHistory(loading, "cpu", []), { key: "cpu", status: "ready", value: [] })
})

test("full-hour histories refresh on the shared data generation, not cursor movement", async () => {
  const [app, postgres, relations, system] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-relations-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
  ])

  assert.match(app, /useHistoryRequest\(processHistoryKey, refreshVersion,/)
  assert.match(app, /<SystemView[^>]*historyRevision=\{refreshVersion\}/)
  assert.match(app, /<PostgresView[^>]*historyRevision=\{refreshVersion\}/)
  assert.match(postgres, /useHistoryRequest\(String\(hour\), historyRevision,/)
  for (const source of [postgres, relations, system]) {
    assert.doesNotMatch(source, /useHistoryRequest\([^\n]*, (?:row|selectedRow)\?*\.?timestamp/)
  }
})
