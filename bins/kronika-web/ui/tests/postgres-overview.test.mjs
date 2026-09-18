import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const overview = await importModule('export { buildBands } from "../src/postgres-overview.tsx"', {
  plugins: [
    {
      name: "overview-reducer",
      setup(context) {
        context.onLoad({ filter: /\/postgres-overview\.tsx$/ }, async ({ path }) => ({
          contents: `${await readFile(path, "utf8")}\nexport { buildBands }`,
          loader: "tsx",
        }))
      },
    },
    registryPlugin([]),
  ],
})

const START = 1_780_000_000_000_000
const SECOND = 1_000_000

function row(section, seconds, values) {
  const typeId = {
    pg_stat_activity: "1001004",
    pg_stat_database: "1005004",
    pg_stat_progress_vacuum: "1012004",
  }[section]
  return {
    segmentId: "s", logicalName: section, typeId, ordinal: `${seconds}:${values.pid ?? 0}`,
    timestamp: START + seconds * SECOND, values,
  }
}

function vacuumWorkers(activity, database, vacuum) {
  const streams = {
    activity, database, vacuum,
    wal: [], checkpointer: [], bgwriter: [], archiver: [], io: [], walStorage: [],
    prepared: [], settings: [], lifecycle: [],
  }
  const bands = overview.buildBands(streams, [], "en", (key) => key)
  return bands.find((band) => band.key === "horizons").rows.find((metric) => metric.key === "vacuum_workers")
}

test("Overview counts one vacuum worker per adaptive activity snapshot, not per database interval", () => {
  const activity = [0, 10, 15, 25, 35].flatMap((seconds) => [
    row("pg_stat_activity", seconds, { pid: 9, backend_type: "autovacuum worker" }),
    row("pg_stat_activity", seconds, { pid: 10, backend_type: "client backend" }),
  ])
  const database = [0, 30, 60].map((seconds) => row("pg_stat_database", seconds, { datid: 5 }))
  const vacuum = [0.1, 10.1, 15.1, 25.1].map((seconds) => row("pg_stat_progress_vacuum", seconds, {
    pid: 9, is_autovacuum: true,
  }))
  vacuum.push(row("pg_stat_progress_vacuum", 15.1, { pid: 11, is_autovacuum: false }))

  const workers = vacuumWorkers(activity, database, vacuum)
  assert.deepEqual(workers.points.map(({ timestamp, value }) => [(timestamp - START) / SECOND, value]), [
    [0, 1], [10, 1], [15, 1], [25, 1], [35, 0],
  ])
  assert.equal(workers.headline, "1")
})

test("Overview retains vacuum worker counts when all PostgreSQL sources share the old cadence", () => {
  const database = [0, 30, 60].map((seconds) => row("pg_stat_database", seconds, { datid: 5 }))
  const activity = [0.1, 30.1, 60.1].map((seconds) => row("pg_stat_activity", seconds, { pid: 9 }))
  const vacuum = [0.2, 30.2].map((seconds) => row("pg_stat_progress_vacuum", seconds, {
    pid: 9, is_autovacuum: true,
  }))
  assert.deepEqual(vacuumWorkers(activity, database, vacuum).points.map(({ value }) => value), [1, 1, 0])
})
