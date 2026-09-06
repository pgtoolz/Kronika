import assert from "node:assert/strict"
import test from "node:test"

import {
  exportDurationSeconds,
  exportRangeDefaults,
  formatExportDuration,
  formatExportEndpoint,
  resolveExportEndpoint,
} from "../src/export-time.ts"

const noPreference = { occurrence: null, preferred: null } as const

test("selected-hour defaults retain inclusive whole-second bounds in both representations", () => {
  withZone("Asia/Kolkata", () => {
    const hour = Date.UTC(2026, 7, 14, 5) * 1_000
    const browser = exportRangeDefaults(hour, "browser")
    const utc = exportRangeDefaults(hour, "utc")
    assert.deepEqual(browser, {
      from: { date: "2026-08-14", second: 1_786_683_600, time: "10:30:00" },
      to: { date: "2026-08-14", second: 1_786_687_199, time: "11:29:59" },
    })
    assert.deepEqual(utc, {
      from: { date: "2026-08-14", second: browser.from.second, time: "05:00:00" },
      to: { date: "2026-08-14", second: browser.to.second, time: "05:59:59" },
    })
    for (const endpoint of [browser.from, browser.to]) {
      assert.equal(resolveExportEndpoint(endpoint.date, endpoint.time, "browser", {
        occurrence: null,
        preferred: endpoint.second,
      }).second, endpoint.second)
    }
    assert.equal(exportDurationSeconds(browser.from.second, browser.to.second), 3_600)
  })
})

test("UTC exact fields round-trip across days and before the Unix epoch", () => {
  assert.deepEqual(formatExportEndpoint(-1, "utc"), { date: "1969-12-31", second: -1, time: "23:59:59" })
  assert.equal(resolveExportEndpoint("1969-12-31", "23:59:59", "utc").second, -1)
  assert.equal(resolveExportEndpoint("2026-08-16", "00:00:00", "utc").second, Date.UTC(2026, 7, 16) / 1_000)
})

test("date, time, and nonexistent civil-time errors stay distinct", () => {
  withZone("America/New_York", () => {
    const cases = [
      ["", "00:00:00", "date_required"],
      ["2026-02-29", "00:00:00", "date_invalid"],
      ["2026-01-01", "", "time_required"],
      ["2026-01-01", "12:00", "time_invalid"],
      ["2026-01-01", "24:00:00", "time_invalid"],
      ["2026-01-01", "12:00:00.500", "time_invalid"],
      ["2026-03-08", "02:30:00", "nonexistent"],
    ] as const
    for (const [date, time, error] of cases) {
      assert.equal(resolveExportEndpoint(date, time, "browser", noPreference).error, error)
    }
    assert.equal(resolveExportEndpoint("2026-03-08", "01:59:59", "browser").second, 1_772_953_199)
    assert.equal(resolveExportEndpoint("2026-03-08", "03:00:00", "browser").second, 1_772_953_200)
    assert.equal(exportDurationSeconds(1_772_953_199, 1_772_953_200), 2)
  })
})

test("a New York fold requires and preserves an explicit occurrence", () => {
  withZone("America/New_York", () => {
    const unresolved = resolveExportEndpoint("2026-11-01", "01:30:00", "browser", noPreference)
    assert.equal(unresolved.error, "occurrence_required")
    assert.deepEqual(unresolved.candidates, [1_793_511_000, 1_793_514_600])
    const first = resolveExportEndpoint("2026-11-01", "01:30:00", "browser", { occurrence: 0, preferred: null })
    const second = resolveExportEndpoint("2026-11-01", "01:30:00", "browser", { occurrence: 1, preferred: null })
    assert.equal(first.second, 1_793_511_000)
    assert.equal(second.second, 1_793_514_600)
    assert.deepEqual(formatExportEndpoint(first.second!, "browser"), { date: "2026-11-01", second: first.second, time: "01:30:00" })
    assert.deepEqual(formatExportEndpoint(second.second!, "browser"), { date: "2026-11-01", second: second.second, time: "01:30:00" })
    assert.equal(resolveExportEndpoint("2026-11-01", "01:31:00", "browser", {
      occurrence: null,
      preferred: second.second,
    }).occurrence, 1)
  })
})

test("Lord Howe exposes two occurrences thirty minutes apart", () => {
  withZone("Australia/Lord_Howe", () => {
    const folded = resolveExportEndpoint("2026-04-05", "01:30:00", "browser", noPreference)
    assert.equal(folded.error, "occurrence_required")
    assert.deepEqual(folded.candidates, [1_775_313_000, 1_775_314_800])
    const from = resolveExportEndpoint("2026-04-05", "01:50:00", "browser", { occurrence: 0, preferred: null })
    const to = resolveExportEndpoint("2026-04-05", "01:40:00", "browser", { occurrence: 1, preferred: null })
    assert.equal(from.second, 1_775_314_200)
    assert.equal(to.second, 1_775_315_400)
    assert.equal(exportDurationSeconds(from.second, to.second), 1_201)
    assert.equal(exportDurationSeconds(to.second, from.second), null)
    assert.equal(resolveExportEndpoint("2026-10-04", "02:15:00", "browser").error, "nonexistent")
    assert.equal(resolveExportEndpoint("2026-10-04", "01:59:59", "browser").second, 1_791_041_399)
    assert.equal(resolveExportEndpoint("2026-10-04", "02:30:00", "browser").second, 1_791_041_400)
  })
})

test("duration text is exact, inclusive, and readable across days", () => {
  assert.equal(exportDurationSeconds(50, 50), 1)
  assert.equal(formatExportDuration(1, "en"), "1\u00a0s")
  assert.equal(formatExportDuration(3_600, "ru"), "1\u00a0ч")
  assert.equal(formatExportDuration(2 * 86_400 + 4 * 3_600 + 30 * 60 + 1, "ru"), "2\u00a0д 4\u00a0ч 30\u00a0мин 1\u00a0с")
  assert.equal(formatExportDuration(2 * 86_400 + 4 * 3_600 + 30 * 60 + 1, "en"), "2\u00a0d 4\u00a0h 30\u00a0min 1\u00a0s")
})

function withZone(zone: string, run: () => void): void {
  const previous = process.env.TZ
  process.env.TZ = zone
  try { run() } finally {
    if (previous === undefined) delete process.env.TZ
    else process.env.TZ = previous
  }
}
