import assert from "node:assert/strict"
import test from "node:test"

import { SPARK_HEIGHT, SPARK_WIDTH, sparkCursorX, sparkGeometry, sparkScaleMax } from "../src/spark.ts"

const HOUR = 0
const END = 3_600_000_000

test("spark geometry breaks the line at explicit nulls", () => {
  const geometry = sparkGeometry([
    { segmentId: "1", timestamp: 0, value: 10 },
    { segmentId: "1", timestamp: 900_000_000, value: 20 },
    { segmentId: "1", timestamp: 1_800_000_000, value: null },
    { segmentId: "1", timestamp: 2_700_000_000, value: 30 },
    { segmentId: "1", timestamp: 3_500_000_000, value: 40 },
  ], HOUR, END, 100)
  assert.equal((geometry.path.match(/M/g) ?? []).length, 2)
  assert.equal(geometry.dots.length, 0)
})

test("spark geometry renders an isolated sample as a dot", () => {
  const geometry = sparkGeometry([
    { segmentId: "1", timestamp: 0, value: null },
    { segmentId: "1", timestamp: 1_800_000_000, value: 50 },
    { segmentId: "1", timestamp: 3_500_000_000, value: null },
  ], HOUR, END, 100)
  assert.equal(geometry.path, "")
  assert.equal(geometry.dots.length, 1)
  const [x, y] = geometry.dots[0]!.split(" ").map(Number)
  assert.ok(Math.abs(x! - SPARK_WIDTH / 2) <= 0.5)
  assert.ok(y! > 0 && y! < SPARK_HEIGHT)
})

test("spark scale is 100 for shares and a nice ceiling otherwise", () => {
  assert.equal(sparkScaleMax("share", [[{ segmentId: "1", timestamp: 0, value: 900 }]]), 100)
  assert.equal(sparkScaleMax("rate", [[{ segmentId: "1", timestamp: 0, value: 7 }]]), 10)
  assert.equal(sparkScaleMax("bytes", [
    [{ segmentId: "1", timestamp: 0, value: 3 }],
    [{ segmentId: "1", timestamp: 0, value: 40 }],
  ]), 50)
})

test("spark cursor projects inside the hour and hides outside", () => {
  assert.equal(sparkCursorX(1_800_000_000, HOUR, END), SPARK_WIDTH / 2)
  assert.equal(sparkCursorX(-1, HOUR, END), null)
  assert.equal(sparkCursorX(END, HOUR, END), null)
})
