import assert from "node:assert/strict"
import test from "node:test"

import {
  activePreset,
  aroundCursor,
  exportFilename,
  hourRange,
  presetRange,
  rangeOnHour,
  rangeSeconds,
  readLastExportSeconds,
  shiftRange,
  validRange,
  writeLastExportSeconds,
} from "../src/export-range.ts"

const HOUR = Date.UTC(2026, 8, 5, 8) * 1_000
const CURSOR = HOUR + 17 * 60 * 1_000_000 + 59 * 1_000_000

test("presets come from the hour and the cursor as inclusive whole seconds", () => {
  assert.deepEqual(hourRange(HOUR), { from: HOUR / 1_000_000, to: HOUR / 1_000_000 + 3_599 })
  assert.deepEqual(aroundCursor(CURSOR, 5), { from: CURSOR / 1_000_000 - 300, to: CURSOR / 1_000_000 + 299 })
  assert.equal(rangeSeconds(aroundCursor(CURSOR, 15)), 30 * 60)
  assert.equal(activePreset(presetRange("around30", HOUR, CURSOR), HOUR, CURSOR), "around30")
  assert.equal(activePreset(hourRange(HOUR), HOUR, CURSOR), "hour")
  assert.equal(activePreset(shiftRange(hourRange(HOUR), 3_600), HOUR, CURSOR), null)
  assert.deepEqual(shiftRange({ from: 10, to: 20 }, -3_600), { from: -3_590, to: -3_580 })
  assert.equal(validRange({ from: -3_590, to: -3_580 }), false, "positive seconds only")
  assert.equal(validRange({ from: 20, to: 10 }), false, "start after end")
  assert.equal(validRange({ from: 10, to: 10 }), true, "one second is a range")
})

test("the file is named the way the server names it, from both UTC seconds", () => {
  assert.equal(exportFilename({ from: Date.UTC(2024, 1, 29) / 1_000, to: Date.UTC(2024, 1, 29) / 1_000 + 3_599 }), "kronika-2024-02-29-000000-2024-02-29-005959-utc.html")
})

test("the timeline shows only the part of the range that lies in its hour", () => {
  const start = HOUR / 1_000_000
  assert.deepEqual(rangeOnHour({ from: start - 600, to: start + 599 }, HOUR), { from: HOUR, to: HOUR + 600 * 1_000_000 })
  assert.deepEqual(rangeOnHour({ from: start + 3_000, to: start + 9_000 }, HOUR), { from: HOUR + 3_000 * 1_000_000, to: HOUR + 3_600 * 1_000_000 })
  assert.equal(rangeOnHour({ from: start - 7_200, to: start - 3_601 }, HOUR), null)
})

test("the last preparation time is stored to one decimal and read back only when usable", () => {
  const store = new Map<string, string>()
  const storage = { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => { store.set(key, value) } }
  assert.equal(readLastExportSeconds(storage), null)
  writeLastExportSeconds(storage, 8.26)
  assert.equal(readLastExportSeconds(storage), 8.3)
  store.set("kronika.export-seconds", "garbage")
  assert.equal(readLastExportSeconds(storage), null)
})
