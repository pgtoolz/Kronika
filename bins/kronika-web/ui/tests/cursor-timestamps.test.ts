import assert from "node:assert/strict"
import test from "node:test"

import { mergeObservationTimestamps, observationTimestamps } from "../src/cursor-timestamps.ts"

test("current-screen navigation keeps exact mixed-cadence observations without inventing a grid", () => {
  const pgWaiting = [0, 30, 60].map((timestamp) => ({ timestamp: timestamp * 1_000_000 }))
  const os = Array.from({ length: 13 }, (_, index) => ({ timestamp: index * 5_000_000 }))
  const shared = observationTimestamps(pgWaiting, os)

  assert.deepEqual(shared, os.map(({ timestamp }) => timestamp))
  assert.equal(shared.includes(1_000_000), false)
  assert.equal(shared.includes(30_000_000), true)
})

test("per-database Activity capture moments retain exact microseconds", () => {
  const shared = [0, 30_000_000, 60_000_000]
  const activity = [{ timestamp: 30_000_001 }, { timestamp: 30_004_000 }]

  assert.deepEqual(mergeObservationTimestamps(shared, activity), [
    0,
    30_000_000,
    30_000_001,
    30_004_000,
    60_000_000,
  ])
})

test("many Process rows from one snapshot contribute one navigation moment", () => {
  const processRows = Array.from({ length: 10_000 }, (_, index) => ({
    timestamp: index < 7_500 ? 5_000_000 : 10_000_000,
  }))
  const selectedHistory = [{ timestamp: 5_000_000 }, { timestamp: 15_000_000 }]

  assert.deepEqual(mergeObservationTimestamps([], processRows, selectedHistory), [
    5_000_000,
    10_000_000,
    15_000_000,
  ])
})
