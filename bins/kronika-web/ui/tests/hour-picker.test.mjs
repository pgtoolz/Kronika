import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const picker = await importModule(`export {
  catalogueHours,
  hourPopoverPlacement,
  hourHasData,
  hoursForDay,
  pickerFocusIndex,
  visibleHours,
} from "../src/hour-picker.tsx";
export { calendarMonthDays, calendarMonthLabel, createDisplayTimeFormatter } from "../src/display-time.ts"`)

const HOUR = 3_600_000_000

test("the picker enumerates exact catalogue instants and retains an exact current selection", () => {
  const first = Date.UTC(2026, 7, 10, 15) * 1_000
  const current = first + HOUR
  const hours = picker.catalogueHours([first + 2 * HOUR, first, first], current)
  assert.deepEqual(hours, [first, current, first + 2 * HOUR])
  assert.equal(picker.hourHasData(current, [first, first + 2 * HOUR]), false)
  assert.equal(picker.hourHasData(first, [first, first + 2 * HOUR]), true)
})

test("report navigation exposes requested hour buckets without context or foreign URL hours", () => {
  const requested = Date.UTC(2026, 8, 4, 12) * 1_000
  const context = requested - HOUR
  const foreign = requested + 2 * HOUR
  assert.deepEqual(
    picker.catalogueHours([context, requested], foreign, { from: requested, toExclusive: requested + HOUR }),
    [requested],
  )
  assert.deepEqual(picker.visibleHours({
    from: requested + 15 * 60_000_000,
    toExclusive: requested + 2 * HOUR + 20 * 60_000_000,
  }), [requested, requested + HOUR, requested + 2 * HOUR])
})

test("active-zone civil days group exact instants across UTC and half-hour boundaries", () => {
  const first = Date.UTC(2026, 7, 13, 18, 30) * 1_000
  const second = first + HOUR
  const india = picker.createDisplayTimeFormatter("en", "browser", "Asia/Kolkata")
  const utc = picker.createDisplayTimeFormatter("en", "utc", "Asia/Kolkata")
  assert.equal(india.dayKey(first), "2026-08-14")
  assert.equal(utc.dayKey(first), "2026-08-13")
  assert.deepEqual(picker.hoursForDay([first, second], "2026-08-14", india), [first, second])
  assert.deepEqual(picker.hoursForDay([first, second], "2026-08-14", utc), [])
  assert.equal(india.hourLabel(first), "00:00")
})

test("DST-fold catalogue instants keep exact keys when their plain labels repeat", () => {
  const first = Date.UTC(2026, 10, 1, 5) * 1_000
  const second = Date.UTC(2026, 10, 1, 6) * 1_000
  const eastern = picker.createDisplayTimeFormatter("en", "browser", "America/New_York")
  assert.deepEqual(picker.hoursForDay([first, second], "2026-11-01", eastern), [first, second])
  assert.equal(eastern.hourLabel(first), "01:00")
  assert.equal(eastern.hourLabel(second), "01:00")
})

test("the three-column exact-hour grid has bounded keyboard navigation", () => {
  assert.equal(picker.pickerFocusIndex(7, "ArrowLeft", 10), 6)
  assert.equal(picker.pickerFocusIndex(7, "ArrowRight", 10), 8)
  assert.equal(picker.pickerFocusIndex(7, "ArrowUp", 10), 4)
  assert.equal(picker.pickerFocusIndex(7, "ArrowDown", 10), 9)
  assert.equal(picker.pickerFocusIndex(1, "ArrowUp", 10), 0)
  assert.equal(picker.pickerFocusIndex(8, "Home", 10), 0)
  assert.equal(picker.pickerFocusIndex(2, "End", 10), 9)
  assert.equal(picker.pickerFocusIndex(2, "Enter", 10), null)
})

test("the popover stays inside every viewport regardless of the trigger row", () => {
  assert.deepEqual(picker.hourPopoverPlacement({ bottom: 120, left: 10 }, { height: 900, width: 465 }), {
    left: 10, maxHeight: 764, top: 126, width: 304,
  })
  assert.deepEqual(picker.hourPopoverPlacement({ bottom: 120, left: 430 }, { height: 900, width: 585 }), {
    left: 271, maxHeight: 764, top: 126, width: 304,
  })
  assert.deepEqual(picker.hourPopoverPlacement({ bottom: 90, left: 430 }, { height: 900, width: 945 }), {
    left: 375, maxHeight: 794, top: 96, width: 560,
  })
  assert.deepEqual(picker.hourPopoverPlacement({ bottom: 90, left: 430 }, { compact: false, height: 900, width: 746 }), {
    left: 176, maxHeight: 794, top: 96, width: 560,
  })
})

test("calendar month labels and date counts remain human", () => {
  assert.equal(picker.calendarMonthDays("2027-01").length, 31)
  assert.ok(picker.calendarMonthDays("2026-12").includes("2026-12-31"))
  assert.match(picker.calendarMonthLabel("2027-01", "en"), /January.*2027/i)
  assert.match(picker.calendarMonthLabel("2027-01", "ru"), /январ.*2027/i)
})

test("the combined picker has no native date/time controls or invented local hours", async () => {
  const [pickerSource, appSource, timezoneSource] = await Promise.all([
    readFile(new URL("../src/hour-picker.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/timezone-select.tsx", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(pickerSource, /<input\b|<select\b|type=["']date["']|type=["']time["']/)
  assert.match(appSource, /<TimezoneSelect\b/)
  assert.match(timezoneSource, /data-testid="timezone-select"/)
  assert.match(pickerSource, /data-testid="hour-picker-trigger"/)
  assert.match(pickerSource, /data-testid="hour-popover"/)
  assert.match(pickerSource, /dayHours\.map/)
  assert.doesNotMatch(pickerSource, /ALL_HOURS|selectedHour|Date\.UTC/)
  assert.match(pickerSource, /data-instant=\{candidate\}/)
  assert.match(pickerSource, /document\.addEventListener\("pointerdown"/)
  // The popover portals to body: no in-page stacking context can trap it.
  assert.match(pickerSource, /createPortal\(/)
  assert.match(pickerSource, /document\.body\)/)
  // Layout moved from named CSS rules onto the markup: a week per day row,
  // three hours per hour row, and a popover that never exceeds the viewport.
  assert.match(pickerSource.match(/<div[^>]*data-testid="day-grid"[^>]*>/s)?.[0] ?? "", /grid-cols-7/)
  assert.match(pickerSource.match(/<div[^>]*data-testid="hour-grid"[^>]*>/s)?.[0] ?? "", /grid-cols-3/)
  assert.match(pickerSource.match(/<div[^>]*data-testid="hour-popover"[^>]*>/s)?.[0] ?? "", /w-\[min\(560px,calc\(100vw_-_20px\)\)\]/)
})
