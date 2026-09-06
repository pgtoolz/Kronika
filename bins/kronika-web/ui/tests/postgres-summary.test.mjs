import assert from "node:assert/strict"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { build } from "esbuild"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

import { registryPlugin } from "./import-module.mjs"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  external: ["react", "react-dom", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  plugins: [registryPlugin([])],
  stdin: {
    contents: 'export { PostgresSummary, postgresSummaryRow } from "../src/postgres-summary.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const summary = loaded.exports

const copy = {
  "pg.summary.active_statements": "Active statements",
  "pg.summary.loading": "Loading PostgreSQL context…",
  "pg.summary.title": "PostgreSQL context",
}
const t = (key) => copy[key] ?? key

function row(timestamp, surface, values) {
  return {
    logicalName: "postgresql_summary",
    ordinal: String(timestamp),
    segmentId: "hour",
    timestamp,
    typeId: "postgresql-summary",
    values: { surface, ...values },
  }
}

function markup({ cursor = 250, lens = "load", rows = [], section = "statements", status = "ready" } = {}) {
  return renderToStaticMarkup(createElement(summary.PostgresSummary, {
    cursor,
    lens,
    locale: "en",
    section,
    state: { status, value: rows },
    t,
  }))
}

test("cursor selection uses the latest matching surface row not later than the cursor", () => {
  const rows = [
    row(100, 1, { active_count: 10 }),
    row(200, 2, { active_count: 20 }),
    row(300, 1, { active_count: 30 }),
    row(200, 1, { active_count: 73 }),
  ]
  assert.equal(summary.postgresSummaryRow(rows, "statements", 250)?.values.active_count, 73)
  assert.equal(summary.postgresSummaryRow(rows, "plans", 250)?.values.active_count, 20)
  assert.equal(summary.postgresSummaryRow(rows, "statements", 99), null)
})

test("same-hour refresh retains the active statement count and share as one fact", () => {
  const output = markup({ rows: [row(200, 1, { active_count: 73, active_pct: 9 })], status: "loading" })
  assert.match(output, /Active statements/)
  assert.match(output, /73[^<]*·[^<]*9%/)
  assert.doesNotMatch(output, /Loading PostgreSQL context/)
  assert.equal((output.match(/data-summary-fact=/g) ?? []).length, 1)
})

test("statement stability keeps an empty rail", () => {
  const output = markup({ lens: "stability", rows: [row(200, 1, { active_count: 73, active_pct: 9 })] })
  assert.doesNotMatch(output, /data-summary-fact=/)
  assert.match(output, /aria-label="PostgreSQL context"/)
})

test("loading without a selected row renders one live status", () => {
  const output = markup({ rows: [], status: "loading" })
  assert.match(output, /Loading PostgreSQL context…/)
  assert.match(output, /aria-live="polite"/)
  assert.doesNotMatch(output, /data-summary-fact=/)
})

test("recorded zero stays zero and null stays unavailable", () => {
  const output = markup({
    rows: [row(200, 3, { buffer_read_pct: null, rollback_pct: 0, temp_bytes_per_transaction: null })],
    section: "databases",
  })
  assert.match(output, />0%</)
  assert.equal((output.match(/>—</g) ?? []).length, 2)
})
