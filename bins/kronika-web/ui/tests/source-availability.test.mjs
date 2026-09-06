import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { hasPostgresTelemetry } = await importModule('export { hasPostgresTelemetry } from "../src/source-availability.ts"')
const data = (postgresqlConfigured, postgresqlPresent = false) => ({ postgresqlConfigured, postgresqlPresent })

test("PostgreSQL availability follows configuration or selected-hour telemetry", () => {
  assert.equal(hasPostgresTelemetry(data(false)), false)
  assert.equal(hasPostgresTelemetry(data(true)), true)
  assert.equal(hasPostgresTelemetry(data(false, true)), true)
})

test("unavailable peer routes remain explicit and never redirect into Host", async () => {
  const source = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(source, /const visibleSource = source/)
  assert.doesNotMatch(source, /if \(source === "postgresql" && !pgPresent\) setSource\("host"\)/)
  assert.doesNotMatch(source, /if \(source === "events" && !eventsPresent\) setSource\("host"\)/)
  assert.match(source, /visibleSource === "postgresql" && <PostgresView/)
  assert.match(source, /title=\{pgPresent \? undefined : t\("nav\.no_data"\)\}/)
  assert.match(source, /title=\{eventsPresent \? undefined : t\("nav\.no_data"\)\}/)
  const host = source.indexOf('setSource("host")')
  const processes = source.indexOf('data-testid="process-tab"', host)
  const postgresql = source.indexOf('setSource("postgresql")', processes)
  const events = source.indexOf('setSource("events")', postgresql)
  assert.ok(host < processes && processes < postgresql && postgresql < events)
})
