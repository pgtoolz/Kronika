import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { activityForProcess, hasPostgresTelemetry, postgresProcessesShared, recordedLinuxEnabled } = await importModule('export { activityForProcess, hasPostgresTelemetry, postgresProcessesShared, recordedLinuxEnabled } from "../src/source-availability.ts"')
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

test("recorded PostgreSQL-only mode hides Linux while legacy and mixed hours retain it", () => {
  assert.equal(recordedLinuxEnabled([]), true)
  assert.equal(recordedLinuxEnabled([{ segmentId: "legacy" }]), true)
  assert.equal(recordedLinuxEnabled([{ segmentId: "remote", osEnabled: false }]), false)
  assert.equal(recordedLinuxEnabled([{ segmentId: "remote", osEnabled: false }, { segmentId: "local", osEnabled: true }]), true)
})

test("process sharing applies only to the selected recorded segment", () => {
  const contexts = [
    { segmentId: "local", postgresqlProcessesShared: true },
    { segmentId: "remote", postgresqlProcessesShared: false },
    { segmentId: "old" },
  ]
  assert.equal(postgresProcessesShared(contexts, "local"), true)
  assert.equal(postgresProcessesShared(contexts, "remote"), false)
  assert.equal(postgresProcessesShared(contexts, "old"), false)
  assert.equal(postgresProcessesShared(contexts, "missing"), false)
})

test("a remote backend with the same numeric PID never becomes a local process Activity", () => {
  const row = (logicalName, segmentId) => ({ logicalName, segmentId, timestamp: 100, typeId: "test", ordinal: "0", values: { pid: 42 } })
  const local = row("os_process", "local")
  const remote = row("pg_stat_activity", "remote")
  const contexts = [{ segmentId: "local", postgresqlProcessesShared: true }, { segmentId: "remote", postgresqlProcessesShared: false }]
  assert.equal(activityForProcess(local, [remote], contexts, 100).row, null)
  const shared = row("pg_stat_activity", "local")
  assert.equal(activityForProcess(local, [remote, shared], contexts, 100).row, shared)
  assert.equal(activityForProcess(local, [shared], [], 100).row, null)
  assert.equal(activityForProcess(row("os_process", "remote"), [remote], contexts, 100).row, null)
})
