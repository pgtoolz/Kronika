import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { gunzipSync } from "node:zlib"
import test from "node:test"

import { bundledFixtureHeatmapRecords, bundledFixtureHour, bundledFixtureRange } from "../src/fixture.ts"

const encoded = readFileSync(new URL("../fixtures/real-hour.json.gz", import.meta.url))
const fixture = JSON.parse(gunzipSync(encoded).toString("utf8"))

test("the approved real hour converts without losing entity relationships", () => {
  Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: fixture })
  const range = bundledFixtureRange()
  assert.notEqual(range, null)
  const hour = bundledFixtureHour(Number(range?.from))
  assert.notEqual(hour, null)
  assert.equal(hour?.processes.length, 111_673)
  assert.equal(hour?.activities.length, 2_888)
  assert.equal(hour?.findings.length, 2_884)
  assert.deepEqual(hour?.availableSections, ["os_process", "pg_stat_activity", "health"])
  assert.equal(hour?.sections.os_process, hour?.processes)
  assert.equal(hour?.sections.pg_stat_activity, hour?.activities)
  assert.equal(hour?.memory[0]?.values.mem_available_percent !== undefined, true)
  assert.equal(hour?.memory[0]?.values.mem_available, undefined)
  assert.notEqual(hour?.health[0]?.segmentId, "fixture")
  assert.ok(hour?.health.every((row) => hour.processes.some((process) => process.segmentId === row.segmentId)))
  assert.ok(hour?.points.some((point) => point.series === "os_oom_kills"))
  assert.ok(hour?.points.some((point) => point.series === "os_psi_some_avg10" && point.identity.resource === 0))
  assert.deepEqual(
    [...new Set(hour?.lanePoints.map((point) => point.lane))].sort(),
    ["cpu_busy", "memory", "pg_oldest_xact", "pg_running", "pg_waiting"],
  )
  assert.ok(hour?.lanePoints.every((point) => point.segmentId !== "fixture"))
  const processByTime = new Map<number, Set<unknown>>()
  for (const row of hour?.processes ?? []) {
    const pids = processByTime.get(row.timestamp) ?? new Set()
    pids.add(row.values.pid)
    processByTime.set(row.timestamp, pids)
  }
  const processTimes = [...processByTime.keys()]
  const nearestProcesses = new Map<number, Set<unknown>>()
  for (const activity of hour?.activities ?? []) {
    if (!nearestProcesses.has(activity.timestamp)) {
      const timestamp = processTimes.reduce((nearest, candidate) =>
        Math.abs(candidate - activity.timestamp) < Math.abs(nearest - activity.timestamp) ? candidate : nearest,
      )
      nearestProcesses.set(activity.timestamp, processByTime.get(timestamp) ?? new Set())
    }
  }
  const joined = hour?.activities.filter((row) => nearestProcesses.get(row.timestamp)?.has(row.values.pid)).length
  assert.equal(joined, 2_755)
  const resolvedFindings = (hour?.findings ?? []).filter((finding) =>
    Object.values(hour?.sections ?? {}).some((rows) => rows.some((row) =>
      row.segmentId === finding.segmentId
        && row.typeId === finding.typeId
        && row.ordinal === finding.rowOrdinal
        && row.timestamp === finding.timestamp,
    )),
  )
  assert.equal(resolvedFindings.length, 294)
  assert.equal((hour?.findings.length ?? 0) - resolvedFindings.length, 2_590)
  assert.ok(hour?.findings.some((finding) => finding.logicalName === "pg_stat_statements"))
  assert.ok(hour?.findings.some((finding) => finding.logicalName.startsWith("pg_log_")))
})

test("the production path has no fixture when no approved bundle is injected", () => {
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  assert.equal(bundledFixtureRange(), null)
  assert.equal(bundledFixtureHour(0), null)
})

test("the fixture exposes only exact Rust-generated heatmap records", () => {
  const records = [{ record: "heatmap", labels: ["comm"] }]
  try {
    Object.assign(globalThis, {
      __KRONIKA_REAL_HOUR__: {
        ...fixture,
        heatmaps: [{
          from: 10,
          section: "os_process",
          fields: ["utime", "stime"],
          columns: 60,
          top: 25,
          group: ["comm"],
          records,
        }],
      },
    })
    assert.equal(
      bundledFixtureHeatmapRecords(10, "os_process", ["utime", "stime"], 60, 25, ["comm"]),
      records,
    )
    assert.equal(
      bundledFixtureHeatmapRecords(10, "os_process", ["stime", "utime"], 60, 25, ["comm"]),
      null,
    )
  } finally {
    Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  }
})
