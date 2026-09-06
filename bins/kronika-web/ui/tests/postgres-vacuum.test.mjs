import assert from "node:assert/strict"
import test from "node:test"

import { readFile } from "node:fs/promises"
import { importModule, registryPlugin } from "./import-module.mjs"

const vacuum = await importModule(
  'export * from "../src/postgres-vacuum.ts"',
  { plugins: [registryPlugin([])] },
)

const HOUR = 1_000_000_000_000
const S = 1_000_000

let ordinal = 0
function row(atSeconds, values, typeId = "1012004") {
  ordinal += 1
  return {
    logicalName: "pg_stat_progress_vacuum", ordinal: String(ordinal), segmentId: "a",
    timestamp: HOUR + atSeconds * S, typeId,
    values: { datid: 5, pid: 9, relid: 70, ...values },
  }
}

test("rows group into episodes by identity, and a lower counter starts a new episode", () => {
  const rows = [
    row(0, { heap_blks_scanned: 10, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap" }),
    row(30, { heap_blks_scanned: 50, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap" }),
    // The same key with a smaller scanned counter is a different run.
    row(60, { heap_blks_scanned: 5, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap" }),
    // A different relation is its own stream regardless of timing.
    row(60, { heap_blks_scanned: 100, heap_blks_vacuumed: 0, index_vacuum_count: 0, phase: "scanning heap", relid: 71 }),
  ]
  const episodes = vacuum.buildVacuumEpisodes(rows, 30)
  assert.equal(episodes.length, 3)
})

test("rows at 10:00:00 and 10:05:00 with no recorded sample between them are two episodes", () => {
  const rows = [
    row(0, { heap_blks_scanned: 10, phase: "scanning heap" }),
    row(300, { heap_blks_scanned: 20, phase: "scanning heap" }),
  ]
  // With a 30-second recorded interval the two rows are not adjacent.
  assert.equal(vacuum.buildVacuumEpisodes(rows, 30).length, 2)
  // Without a recorded interval only identity and counters decide.
  assert.equal(vacuum.buildVacuumEpisodes(rows, null).length, 1)
})

test("the in-phase span restarts when the index cycle increments under an unchanged phase name", () => {
  const rows = [
    row(0, { index_vacuum_count: 1, indexes_processed: 1, phase: "vacuuming indexes" }, "1012005"),
    row(30, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, "1012005"),
    row(60, { index_vacuum_count: 2, indexes_processed: 0, phase: "vacuuming indexes" }, "1012005"),
  ]
  const [episode] = vacuum.buildVacuumEpisodes(rows, 30)
  assert.equal(episode.rows.length, 3)
  assert.equal(episode.phaseRows.length, 1)
  assert.equal(vacuum.phaseSpanUs(episode), 0)
})

test("no movement needs three still samples of the phase counter and never fires on old index layouts", () => {
  const still = (typeId) => vacuum.buildVacuumEpisodes([
    row(0, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, typeId),
    row(30, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, typeId),
    row(60, { index_vacuum_count: 1, indexes_processed: 2, phase: "vacuuming indexes" }, typeId),
  ], 30)[0].noMovement
  assert.deepEqual(still("1012005"), { samples: 3, spanUs: 60 * S })
  // PG10-16 records no index progress: the phase shows its span and nothing more.
  assert.equal(still("1012004"), null)
  // Movement clears it.
  const moving = vacuum.buildVacuumEpisodes([
    row(0, { heap_blks_scanned: 1, phase: "scanning heap" }),
    row(30, { heap_blks_scanned: 1, phase: "scanning heap" }),
    row(60, { heap_blks_scanned: 2, phase: "scanning heap" }),
  ], 30)[0].noMovement
  assert.equal(moving, null)
  // Truncation is judged by the phase alone.
  const truncating = vacuum.buildVacuumEpisodes([
    row(0, { phase: "truncating heap" }),
    row(30, { phase: "truncating heap" }),
    row(60, { phase: "truncating heap" }),
  ], 30)[0].noMovement
  assert.deepEqual(truncating, { samples: 3, spanUs: 60 * S })
})

test("risk is fixed by phase name and the sort fronts the cursor's pass, riskiest first", () => {
  assert.equal(vacuum.phaseRisk("truncating heap"), "dangerous")
  assert.equal(vacuum.phaseRisk("vacuuming indexes"), "heavy")
  assert.equal(vacuum.phaseRisk("vacuuming heap"), "heavy")
  assert.equal(vacuum.phaseRisk("cleaning up indexes"), "heavy")
  assert.equal(vacuum.phaseRisk("scanning heap"), "ordinary")
  assert.equal(vacuum.phaseRisk("unheard-of phase"), "ordinary")
  const rows = [
    row(0, { phase: "scanning heap", relid: 70 }),
    row(60, { phase: "scanning heap", relid: 70 }),
    row(60, { phase: "truncating heap", relid: 71 }),
    row(30, { phase: "vacuuming heap", relid: 72 }),
  ]
  const episodes = vacuum.buildVacuumEpisodes(rows, null)
  const atTs = vacuum.vacuumAtTimestamp(rows, HOUR + 90 * S)
  assert.equal(atTs, HOUR + 60 * S)
  const sorted = vacuum.sortVacuumEpisodes(episodes, atTs)
  // Both at-sample episodes lead, the dangerous one first; the one last seen
  // at second 30 follows.
  assert.equal(sorted[0].last.values.relid, 71)
  assert.equal(sorted[1].last.values.relid, 70)
  assert.equal(sorted[2].last.values.relid, 72)
})

test("layout gates and the delay delta read the recorded layouts, not zeroes", () => {
  const old = [row(0, { phase: "scanning heap" })]
  const pg18 = [
    row(0, { delay_time: 100, phase: "scanning heap" }, "1012006"),
    row(30, { delay_time: 150, phase: "scanning heap" }, "1012006"),
  ]
  assert.equal(vacuum.vacuumLayoutHas(old, "indexes_total"), false)
  assert.equal(vacuum.vacuumLayoutHas(old, "delay_time"), false)
  assert.equal(vacuum.vacuumLayoutHas(pg18, "indexes_total"), true)
  assert.equal(vacuum.vacuumLayoutHas(pg18, "delay_time"), true)
  const [episode] = vacuum.buildVacuumEpisodes(pg18, 30)
  assert.equal(vacuum.delayDelta(episode), 50)
  const [single] = vacuum.buildVacuumEpisodes([pg18[0]], 30)
  assert.equal(vacuum.delayDelta(single), null)
})

test("the progress series is the episode's own scan percent, skipping samples with no usable total", () => {
  const rows = [
    row(0, { heap_blks_scanned: 10, heap_blks_total: 100, phase: "scanning heap" }),
    row(30, { heap_blks_scanned: null, heap_blks_total: null, phase: "scanning heap" }),
    row(60, { heap_blks_scanned: 40, heap_blks_total: 100, phase: "scanning heap" }),
  ]
  const [episode] = vacuum.buildVacuumEpisodes(rows, 30)
  assert.deepEqual(vacuum.progressSeries(episode), [10, 40])
})

test("the hour fetch names no fields: an empty list, not a fixed union across PG-version shapes", async () => {
  // The server rejects a field a segment's own layout does not define at
  // all (query.rs output_names), which is the ordinary case for one
  // instance running one PostgreSQL major all hour. Naming any fixed list
  // here reintroduces that failure the moment the hour holds only one of
  // the three layouts — which is most hours. An empty list asks the server
  // for exactly what each segment's own layout defines.
  const source = await readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8")
  assert.match(source, /useHistoryRequest\(String\(hour\), historyRevision,/)
  assert.match(source, /\(signal\) => loadSeries\(hour, "pg_stat_progress_vacuum", \{\}, \[\], signal\)/)
  assert.match(source, /const vacuumRequestPhase = hourRows\.status === "loading" \? "pending" : hourRows\.status/)
  assert.match(source, /requestPhase=\{vacuumRequestPhase\}/)
})

function processRow(atSeconds, pid, values) {
  return {
    logicalName: "os_process", ordinal: String(atSeconds), segmentId: "a",
    timestamp: HOUR + atSeconds * S, typeId: "1100001",
    values: { pid, ...values },
  }
}

test("process load is the delta between the process samples nearest the episode's own first and last recorded moment", () => {
  const [episode] = vacuum.buildVacuumEpisodes([
    row(0, { heap_blks_scanned: 10, phase: "scanning heap" }, "1012004"),
    row(300, { heap_blks_scanned: 40, phase: "scanning heap" }, "1012004"),
  ], 300)
  const processRows = [
    processRow(-30, 9, { utime: 100, stime: 50, read_bytes: 1_000, write_bytes: 0, majflt: 4, blkdelay_ticks: 2 }),
    processRow(0, 9, { utime: 110, stime: 55, read_bytes: 2_000, write_bytes: 100, majflt: 5, blkdelay_ticks: 3 }),
    processRow(150, 9, { utime: 400, stime: 200, read_bytes: 9_000, write_bytes: 500, majflt: 40, blkdelay_ticks: 20 }),
    // After the episode's own window: not counted.
    processRow(600, 9, { utime: 900, stime: 500, read_bytes: 50_000, write_bytes: 900, majflt: 90, blkdelay_ticks: 50 }),
  ]
  const load = vacuum.vacuumProcessLoad(processRows, episode)
  // The sample exactly at the episode's own first moment is a closer,
  // still-valid baseline than the one before it.
  assert.equal(load.before.ordinal, "0")
  assert.equal(load.after.ordinal, "150")
  assert.equal(load.cpuTicks, 435n) // (400-110) + (200-55)
  assert.equal(load.readBytes, 7_000n)
  assert.equal(load.writeBytes, 400n)
  assert.equal(load.majorFaults, 35n)
  assert.equal(load.blockWaitTicks, 17n)
})

test("process load is null without a baseline before the episode, and a counter reset drops only its own field", () => {
  const [episode] = vacuum.buildVacuumEpisodes([
    row(0, { heap_blks_scanned: 10, phase: "scanning heap" }, "1012004"),
    row(300, { heap_blks_scanned: 40, phase: "scanning heap" }, "1012004"),
  ], 300)
  // No sample at or before the episode's first moment: no honest baseline.
  assert.equal(vacuum.vacuumProcessLoad([processRow(150, 9, { utime: 100, stime: 0 })], episode), null)

  const reset = vacuum.vacuumProcessLoad([
    processRow(-10, 9, { utime: 100, stime: 50, read_bytes: 9_000 }),
    processRow(200, 9, { utime: 150, stime: 5, read_bytes: 12_000 }), // stime went backwards
  ], episode)
  assert.equal(reset.cpuTicks, null)
  assert.equal(reset.readBytes, 3_000n)
})

test("load shares convert ticks to time, and the read share compares against PG's own scanned bytes, never a guess", () => {
  const load = {
    before: { timestamp: HOUR },
    after: { timestamp: HOUR + 10 * S },
    cpuTicks: 500n, blockWaitTicks: 200n, runDelayNs: 0n,
    readBytes: 40_000n, writeBytes: 5_000n, majorFaults: 12n,
  }
  // 500 ticks at 100 Hz is 5 s of CPU over a 10 s span: 50%.
  const withClock = vacuum.vacuumLoadShares(load, 100, 50_000)
  assert.equal(withClock.cpuMs, 5_000)
  assert.equal(withClock.cpuShare, 50)
  assert.equal(withClock.blockWaitMs, 2_000)
  assert.equal(withClock.readBytes, 40_000)
  assert.equal(withClock.readShare, 80) // 40,000 of 50,000 PG scanned bytes
  assert.equal(withClock.majorFaults, 12)

  // No recorded clock rate: tick-scaled facts withhold, byte facts do not.
  const noClock = vacuum.vacuumLoadShares(load, null, 50_000)
  assert.equal(noClock.cpuMs, null)
  assert.equal(noClock.blockWaitMs, null)
  assert.equal(noClock.readBytes, 40_000)

  // No recorded block size: the comparison is withheld, not guessed at 8192.
  const noBlockSize = vacuum.vacuumLoadShares(load, 100, null)
  assert.equal(noBlockSize.readBytes, 40_000)
  assert.equal(noBlockSize.readShare, null)

  // A share is clamped at 100%, never claiming more than the whole span or
  // more than PG itself reports scanning.
  const saturated = vacuum.vacuumLoadShares({ ...load, cpuTicks: 5_000n, readBytes: 90_000n }, 100, 50_000)
  assert.equal(saturated.cpuShare, 100)
  assert.equal(saturated.readShare, 100)
})
