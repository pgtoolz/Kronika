import assert from "node:assert/strict"
import test from "node:test"

import { detailEscapeDismisses } from "../src/detail-dismiss.ts"
import { moveCursor, nearestRecordedTime, orderedRecordedTimes, ownsArrowKeys } from "../src/keyboard.ts"

test("arrows follow exact irregular recorded timestamps", () => {
  const times = [205, 100, 130, 130]
  assert.deepEqual(orderedRecordedTimes(times), [100, 130, 205])
  assert.equal(moveCursor(155, times, "ArrowLeft"), 130)
  assert.equal(moveCursor(155, times, "ArrowRight"), 205)
  assert.equal(moveCursor(130, times, "ArrowLeft"), 100)
  assert.equal(moveCursor(130, times, "ArrowRight"), 205)
  assert.equal(moveCursor(155, times, "Enter"), 155)
})

test("recorded navigation stays at the exact first and last samples", () => {
  const times = [100, 130, 205]
  assert.equal(moveCursor(100, times, "ArrowLeft"), 100)
  assert.equal(moveCursor(205, times, "ArrowRight"), 205)
  assert.equal(moveCursor(80, times, "ArrowLeft"), 80)
  assert.equal(moveCursor(80, times, "ArrowRight"), 100)
  assert.equal(moveCursor(220, times, "ArrowLeft"), 205)
  assert.equal(moveCursor(220, times, "ArrowRight"), 220)
  assert.equal(moveCursor(140, [], "ArrowRight"), 140)
})

test("five-second operating-system samples remain five-second arrow steps", () => {
  const start = 1_800_000_000_000_000
  const times = [start, start + 5_000_000, start + 10_000_000]
  assert.equal(moveCursor(start, times, "ArrowRight"), start + 5_000_000)
  assert.equal(moveCursor(start + 10_000_000, times, "ArrowLeft"), start + 5_000_000)
})

test("pointer targets choose the nearest sample and break ties earlier", () => {
  assert.equal(nearestRecordedTime([205, 100, 130], 181), 205)
  assert.equal(nearestRecordedTime([100, 180], 140), 100)
  assert.equal(nearestRecordedTime([], 140), null)
})

test("editing controls retain their arrow keys", () => {
  for (const tag of ["button", "input", "select", "textarea"]) assert.equal(ownsArrowKeys(tag, false), true)
  assert.equal(ownsArrowKeys("div", true), true)
  assert.equal(ownsArrowKeys("div", false), false)
})

test("detail Escape yields to higher-priority overlays", () => {
  assert.equal(detailEscapeDismisses("Escape", false, false), true)
  assert.equal(detailEscapeDismisses("Escape", true, false), false)
  assert.equal(detailEscapeDismisses("Escape", false, true), false)
  assert.equal(detailEscapeDismisses("Enter", false, false), false)
})
