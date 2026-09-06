import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { USE_RESOURCES, integrateRate, lanePointsByLane, ledgerVerdicts, reading, resolveCell } = await importModule('export { USE_RESOURCES, integrateRate, lanePointsByLane, ledgerVerdicts, reading, resolveCell } from "../src/use-table.tsx"')

test("a resource reading carries the unit of what it measures", () => {
  assert.equal(reading(61.06, "en", "share", "/s"), "61.1%")
  assert.equal(reading(3_355_443, "en", "bytes", "/s"), "3.2 MiB/s")
  assert.equal(reading(0.02, "en", "count", "/s"), "0.02")
  assert.equal(reading(1_400, "en", "rate", "/s"), "1.4K/s")
  assert.equal(reading(21_471, "ru", "rate", "/с"), "21,5 тыс./с")
})

test("the resource table selects a resource per row and keeps the network pair in one cell", async () => {
  const source = await readFile(new URL("../src/use-table.tsx", import.meta.url), "utf8")
  assert.equal(source.includes("<SeriesChart"), false)
  assert.match(source, /data-testid=\{`use-row-\$\{resource\.key\}`\}/)
  assert.match(source, /aria-expanded=\{open\}/)
  assert.match(source, /<SparkCell /)
  assert.match(source, /second === undefined \? null : seriesReading/)
  assert.match(source, /lanePointsByLane\(lanePoints\)/)
  assert.match(source, /visibleResources === undefined \|\| visibleResources\.has\(resource\.key\)/)
  // Container rows are ordinary resource rows under their own scope heading;
  // the header cells are computed verdicts, not slogans.
  assert.doesNotMatch(source, /cgroupRow|afterCgroups|use\.cgroups_hint/)
  assert.match(source, /resolveCell\(resource, column, byLane\)/)
  assert.match(source, /data-testid=\{`use-verdict-\$\{column\}`\}/)
  assert.match(source, /onOpenRow\(verdict\.key as UseResourceKey\)/)
  assert.match(source, /aria-pressed=\{metric === cell\.metric\}/)
  assert.match(source, /onCellSelect\(resource\.key, cell\.metric\)/)
  assert.match(source, /coarse:min-h-11/)
  assert.doesNotMatch(source, /data-testid=\{`use-row-\$\{resource\.key\}`\} onClick=/)
  assert.match(source, /data-testid=\{`use-empty-\$\{resource\.key\}-\$\{column\}`\}/)
})

test("the header verdicts are read from the rows: peak share at the cursor, non-zero pressure, summed events", () => {
  const t = (key, params) => params === undefined ? key : `${key}:${Object.values(params).join(",")}`
  const point = (lane, timestamp, value) => ({ segmentId: "s", lane, timestamp, value })
  const lanePoints = [
    point("cpu_busy", 10_000_000, 30), point("cpu_stall", 5_000_000, 0), point("cpu_stall", 10_000_000, 0),
    point("memory", 10_000_000, 62.5), point("mem_swap", 10_000_000, 0),
    point("mem_oom", 5_000_000, null), point("mem_oom", 10_000_000, 2), point("mem_oom", 15_000_000, 0),
    point("cg_memory_bytes", 10_000_000, 4096), point("cg_mem_psi", 5_000_000, 0), point("cg_mem_psi", 10_000_000, 3.5), point("cg_oom", 10_000_000, 0),
  ]
  const byLane = lanePointsByLane(lanePoints)
  const rows = USE_RESOURCES.filter(({ key }) => ["cgroup_memory", "cpu", "memory", "disk"].includes(key))
  // The memory row of the container has no limit, so its share lane is empty and the byte lane stands in.
  assert.equal(resolveCell(rows.find(({ key }) => key === "cgroup_memory"), "utilisation", byLane).cell.lane, "cg_memory_bytes")
  assert.equal(resolveCell(rows.find(({ key }) => key === "disk"), "utilisation", byLane), null)
  const verdicts = ledgerVerdicts(rows, byLane, 10_000_000, "en", t, (key) => `label(${key})`)
  // Host memory at 62.5% beats CPU at 30%; the container's bytes do not compete with shares.
  assert.deepEqual(verdicts.utilisation, { key: "memory", text: "62.5% · label(memory)" })
  // Only the container memory pressure was non-zero in the hour; its peak is named, zero pressures are not listed.
  assert.deepEqual(verdicts.saturation, { key: "cgroup_memory", text: "use.scope.container · use.lane.cg_mem_psi 3.5%" })
  // Two OOM kills per second for five seconds are ten kills over the hour.
  assert.equal(integrateRate([point("mem_oom", 5_000_000, null), point("mem_oom", 10_000_000, 2), point("mem_oom", 15_000_000, 0)]), 10)
  assert.deepEqual(verdicts.errors, { key: "memory", text: "use.verdict.events:10" })
  const quiet = ledgerVerdicts(rows, lanePointsByLane([point("cpu_busy", 10_000_000, 30), point("cpu_stall", 10_000_000, 0), point("mem_oom", 10_000_000, 0)]), 10_000_000, "en", t, (key) => key)
  assert.deepEqual(quiet.saturation, { key: null, text: "use.verdict.quiet" })
  assert.deepEqual(quiet.errors, { key: null, text: "use.verdict.quiet" })
  const nothing = ledgerVerdicts(rows, lanePointsByLane([]), 10_000_000, "en", t, (key) => key)
  assert.deepEqual(nothing, { utilisation: { key: null, text: "—" }, saturation: { key: null, text: "—" }, errors: { key: null, text: "—" } })
})


test("saturation names host and container pressure separately", () => {
  const points = lanePointsByLane([
    { lane: "cpu_stall", segmentId: "s", timestamp: 10, value: 8 },
    { lane: "cg_cpu_psi", segmentId: "s", timestamp: 10, value: 3 },
    { lane: "cg_cpu_throttle", segmentId: "s", timestamp: 10, value: 0 },
  ])
  const t = (key) => ({ "use.scope.host": "Host", "use.scope.container": "Container", "use.lane.cpu_stall": "CPU PSI", "use.lane.cg_cpu_psi": "CPU PSI" })[key] ?? key
  const verdict = ledgerVerdicts(USE_RESOURCES, points, 10, "en", t, (key) => key, true).saturation
  assert.equal(verdict.key, "cpu")
  assert.equal(verdict.text, "Container · CPU PSI 3% · Host · CPU PSI 8%")
})
