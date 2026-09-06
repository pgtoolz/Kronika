import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { semanticValueTone } from "../src/value-tone.ts"

test("null and workload volume remain neutral while zero rates are inactive", () => {
  assert.equal(semanticValueTone("calls_per_second", null, true), null)
  assert.equal(semanticValueTone("calls_per_second", 0, true), "inactive")
  assert.equal(semanticValueTone("calls_per_second", 1_000_000, true), null)
  assert.equal(semanticValueTone("calls", 0), null)
})

test("statement duration marks only the inclusive five-second boundary", () => {
  assert.equal(semanticValueTone("mean_exec_ms_per_call", 4_999.999), null)
  assert.equal(semanticValueTone("mean_exec_ms_per_call", 5_000), "critical")
  assert.equal(semanticValueTone("mean_exec_time_ms", 4_999.99), null)
  assert.equal(semanticValueTone("mean_exec_time_ms", 5_000), "critical")
  assert.equal(semanticValueTone("query_duration_ms", 999.99), null)
  assert.equal(semanticValueTone("query_duration_ms", 1_000), "warning")
  assert.equal(semanticValueTone("query_duration_ms", 4_999.99), "warning")
  assert.equal(semanticValueTone("query_duration_ms", 5_000), "critical")
})

test("Activity marks long transactions and visible wait states", () => {
  assert.equal(semanticValueTone("transaction_duration_ms", 4_999), null)
  assert.equal(semanticValueTone("transaction_duration_ms", 5_000), "warning")
  assert.equal(semanticValueTone("transaction_duration_ms", 60_000), "critical")
  assert.equal(semanticValueTone("state", "idle"), null)
  assert.equal(semanticValueTone("state", "idle in transaction"), "warning")
  assert.equal(semanticValueTone("state", "idle in transaction (aborted)"), "critical")
  assert.equal(semanticValueTone("wait_event_type", null), null)
  assert.equal(semanticValueTone("wait_event_type", "Lock"), "warning")
})

test("Activity duration tones belong only to active client backends", () => {
  const activity = (backend_type: string, state = "active") => ({
    logicalName: "pg_stat_activity",
    ordinal: "0",
    segmentId: "a",
    timestamp: 10_000_000,
    typeId: "1001004",
    values: { backend_type, state },
  })
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("client backend")), "critical")
  assert.equal(semanticValueTone("transaction_duration_ms", 60_000, false, activity("client backend")), "critical")
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("walsender")), null)
  assert.equal(semanticValueTone("transaction_duration_ms", 60_000, false, activity("walsender")), null)
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("checkpointer")), null)
  assert.equal(semanticValueTone("query_duration_ms", 5_000, false, activity("client backend", "idle in transaction")), null)
})

test("cache hit tones distinguish no accesses from a real zero-percent hit rate", () => {
  assert.equal(semanticValueTone("hit_pct", null), null)
  assert.equal(semanticValueTone("hit_pct", 0), "critical")
  assert.equal(semanticValueTone("hit_pct", 89.999), "critical")
  assert.equal(semanticValueTone("hit_pct", 90), "warning")
  assert.equal(semanticValueTone("hit_pct", 98.999), "warning")
  assert.equal(semanticValueTone("hit_pct", 99), "good")
})

test("statement stability and planning use exact inclusive boundaries", () => {
  assert.equal(semanticValueTone("cv", 0.999), "good")
  assert.equal(semanticValueTone("cv", 1), "warning")
  assert.equal(semanticValueTone("cv", 3), "critical")
  assert.equal(semanticValueTone("plan_time_pct", 49.999), "good")
  assert.equal(semanticValueTone("plan_time_pct", 50), "warning")
  assert.equal(semanticValueTone("plan_time_pct", 80), "critical")
})

test("a Linux process state carries its own top-style tones, distinct from PostgreSQL's idle-in-transaction states", () => {
  assert.equal(semanticValueTone("state", "R"), "good")
  assert.equal(semanticValueTone("state", "D"), "warning")
  assert.equal(semanticValueTone("state", "Z"), "critical")
  assert.equal(semanticValueTone("state", "I"), "inactive")
  assert.equal(semanticValueTone("state", "S"), null)
})

test("the ps STAT string tones by its state letter, whatever flags trail it", () => {
  assert.equal(semanticValueTone("process_stat", "R"), "good")
  assert.equal(semanticValueTone("process_stat", "Rl"), "good")
  assert.equal(semanticValueTone("process_stat", "D<l"), "warning")
  assert.equal(semanticValueTone("process_stat", "ZN"), "critical")
  assert.equal(semanticValueTone("process_stat", "Il"), "inactive")
  assert.equal(semanticValueTone("process_stat", "S<l"), null)
  assert.equal(semanticValueTone("process_stat", null), null)
})

test("the row is shaded for running and blocked states, and left alone for sleeping ones", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
  const rowTone = (tone: string) => new RegExp(
    String.raw`\.process-table \.entity-row:has\(:is\(\[data-field="state"\], \[data-field="process_stat"\]\)\[data-value-tone="${tone}"\]\)`,
  )
  for (const tone of ["good", "warning", "critical"]) assert.match(styles, rowTone(tone), tone)
  // Sleeping has no tone at all, so no rule can reach it.
  assert.equal(semanticValueTone("process_stat", "S"), null)
  assert.equal(semanticValueTone("process_stat", "Ss"), null)
  assert.equal(semanticValueTone("process_stat", "D"), "warning")
  assert.equal(semanticValueTone("process_stat", "R"), "good")
  // Load is decided later in the sheet, so a hot row wins over a calm state.
  const stateWarning = styles.indexOf('[data-field="process_stat"])[data-value-tone="good"]')
  const loadWarning = styles.indexOf('[data-field="cpu_percent"][data-value-tone="warning"]')
  const loadCritical = styles.indexOf('[data-field="cpu_percent"][data-value-tone="critical"]')
  assert.ok(stateWarning < loadWarning && loadWarning < loadCritical, "load rules must follow the state rules")
})

test("a Linux process state still tones correctly when the cell arrives as its raw ASCII code, not the character", () => {
  assert.equal(semanticValueTone("state", "R".charCodeAt(0)), "good")
  assert.equal(semanticValueTone("state", "D".charCodeAt(0)), "warning")
  assert.equal(semanticValueTone("state", "Z".charCodeAt(0)), "critical")
  assert.equal(semanticValueTone("state", "I".charCodeAt(0)), "inactive")
  assert.equal(semanticValueTone("state", "S".charCodeAt(0)), null)
})

test("%CPU crosses into a top-style warning, then critical, at busy but plain round numbers", () => {
  assert.equal(semanticValueTone("cpu_percent", 49.999), null)
  assert.equal(semanticValueTone("cpu_percent", 50), "warning")
  assert.equal(semanticValueTone("cpu_percent", 89.999), "warning")
  assert.equal(semanticValueTone("cpu_percent", 90), "critical")
})

test("workload volume and identifiers stay neutral", () => {
  assert.equal(semanticValueTone("execution_ms_per_second", 10_000), null)
  assert.equal(semanticValueTone("rows_per_second", 86_400_000), null)
  assert.equal(semanticValueTone("planid", 4), null)
})

test("semantic tones coexist with exact locator classes", async () => {
  const table = await readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8")
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
  assert.match(table, /data-value-tone=/)
  assert.match(table, /data-locator-cell=/)
  assert.match(table, /value-tone-\$\{tone\}.*locator-cell/)
  assert.match(table, /aria-label=\{toneText === null \|\| field === undefined \? undefined : `\$\{toneText\}: \$\{cellAriaValue\(stored, field, locale, t\)\}`\}/)
  assert.doesNotMatch(table, /\$\{toneText\}: \$\{rawText\(stored\)/)
  assert.match(table, /critical: "\[&_\.entity-value\]:text-bad \[&_\.numeric-cell\]:text-bad"/)
  assert.match(table, /border-l-2 border-current/)
  assert.doesNotMatch(table, /value-tone-critical[^"]*box-shadow/)
  assert.match(table, /known_bad: "[^"]*\[&_\.entity-value\]:text-fg-hi"/)
})
