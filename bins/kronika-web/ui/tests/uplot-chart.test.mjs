import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const chart = await importModule('export { alignRecordedSeries, axisTimeLabel, chartNavigationTimestamps, chartSecondsUseful, chartStatsRows, chartSummary, chartTimeRange, compactChartTime, effectiveIsolation, exactReadings, isolatedSampleIndices, navigationSampleText, nearestRecordedTimestamp, sampleText, scalePartitions, scaleRange, seriesStats } from "../src/uplot-chart.tsx"; export { createDisplayTimeFormatter } from "../src/display-time.ts"; export { compact, humanPercent } from "../src/model.ts"')

const format = (value) => String(value)
const line = (id, unit, scale, points) => ({ color: "cyan", helpKey: `${id}.help`, id, label: id, labelKey: `${id}.label`, points, scale, unit, value: format })

test("series stats are the nearest-rank percentiles of exactly the drawn samples", () => {
  assert.equal(chart.seriesStats([]), null)
  assert.deepEqual(chart.seriesStats([7]), { last: 7, max: 7, min: 7, p50: 7, p90: 7, p99: 7 })
  const hundred = Array.from({ length: 100 }, (_, index) => index + 1)
  assert.deepEqual(chart.seriesStats(hundred), { last: 100, max: 100, min: 1, p50: 50, p90: 90, p99: 99 })
  // Time order, not size order, picks the last sample; non-finite values drop out.
  assert.deepEqual(chart.seriesStats([9, 1, 5, Number.NaN, 3]), { last: 3, max: 9, min: 1, p50: 3, p90: 9, p99: 9 })
})

test("statistics stay associated with every visible series", () => {
  const series = [
    line("cpu", "%", "percent", [{ segmentId: "a", timestamp: 1, value: 10 }, { segmentId: "a", timestamp: 2, value: 20 }]),
    line("wait", "%", "percent", [{ segmentId: "a", timestamp: 1, value: null }, { segmentId: "a", timestamp: 2, value: 3 }]),
  ]
  const rows = chart.chartStatsRows(series, chart.alignRecordedSeries(series))
  assert.deepEqual(rows.map(({ line: shown, stats }) => [shown.id, stats]), [
    ["cpu", { last: 20, max: 20, min: 10, p50: 10, p90: 20, p99: 20 }],
    ["wait", { last: 3, max: 3, min: 3, p50: 3, p90: 3, p99: 3 }],
  ])
})

test("a crowded chart isolates onto its anchor until the operator chooses", () => {
  const many = ["used", "capacity", "user", "system", "irq"]
  assert.equal(chart.effectiveIsolation(many, undefined, "used"), "used")
  assert.equal(chart.effectiveIsolation(many, undefined, "missing"), "used")
  assert.equal(chart.effectiveIsolation(many, undefined, undefined), "used")
  assert.equal(chart.effectiveIsolation(many, null, "used"), null)
  assert.equal(chart.effectiveIsolation(many, "system", "used"), "system")
  assert.equal(chart.effectiveIsolation(many, "gone", "used"), "used")
  const few = ["rx", "tx"]
  assert.equal(chart.effectiveIsolation(few, undefined, "rx"), null)
  assert.equal(chart.effectiveIsolation(few, "tx", "rx"), "tx")
  assert.equal(chart.effectiveIsolation(["solo"], undefined, undefined), null)
})

test("aligned data distinguishes missing rows, explicit nulls, zero and storage boundaries", () => {
  const frame = chart.alignRecordedSeries([
    line("one", "%", "percent", [
      { segmentId: "a", timestamp: 1, value: 4 },
      { segmentId: "a", timestamp: 3, value: null },
      { segmentId: "a", timestamp: 4, value: 0 },
      { segmentId: "b", timestamp: 5, value: 7 },
    ]),
    line("two", "%", "percent", [{ segmentId: "b", timestamp: 2, value: 9 }]),
  ])
  assert.deepEqual(frame.timestamps, [1, 2, 3, 4, 5])
  assert.deepEqual(frame.data[1], [4, undefined, null, 0, 7])
  assert.deepEqual(frame.data[2], [undefined, 9, undefined, undefined, undefined])
  assert.deepEqual(frame.isolated.get(1), [0])
  assert.deepEqual(frame.isolated.get(2), [1])
})

test("an explicit cursor domain does not add samples to the drawn frame", () => {
  const series = [line("pg_waiting", "count", "nonnegative", [
    { segmentId: "a", timestamp: 0, value: 1 },
    { segmentId: "a", timestamp: 30_000_000, value: 2 },
    { segmentId: "a", timestamp: 60_000_000, value: 3 },
  ])]
  const frame = chart.alignRecordedSeries(series)
  const navigation = chart.chartNavigationTimestamps(frame, [
    0, 5_000_000, 10_000_000, 15_000_000, 30_000_000, 60_000_000,
  ], 0, 61_000_000)

  assert.deepEqual(frame.timestamps, [0, 30_000_000, 60_000_000])
  assert.deepEqual(frame.data[1], [1, 2, 3])
  assert.deepEqual(navigation, [0, 5_000_000, 10_000_000, 15_000_000, 30_000_000, 60_000_000])
  const time = chart.createDisplayTimeFormatter("en", "utc", "UTC")
  assert.match(chart.navigationSampleText(series, frame, navigation, 5_000_000, "en", time), /^00:00:05$/)
})

test("aligned mode joins identical boundaries, preserves resets and rejects conflicting values", () => {
  const joined = chart.alignRecordedSeries([
    line("one", "%", "percent", [
      { segmentId: "a", timestamp: 1, value: 4 },
      { segmentId: "b", timestamp: 1, value: 4 },
    ]),
  ])
  assert.deepEqual(joined.data[1], [4])
  const reset = chart.alignRecordedSeries([
    line("one", "%", "percent", [
      { segmentId: "a", timestamp: 1, value: 4 },
      { segmentId: "b", timestamp: 1, value: null },
      { segmentId: "b", timestamp: 2, value: 5 },
    ]),
  ])
  assert.deepEqual(reset.data[1], [null, 5])
  assert.throws(() => chart.alignRecordedSeries([
    line("one", "%", "percent", [
      { segmentId: "a", timestamp: 1, value: 4 },
      { segmentId: "b", timestamp: 1, value: 5 },
    ]),
  ]), /conflicting chart sample one@1/)
  assert.throws(() => chart.alignRecordedSeries([
    line("one", "%", "percent", [{ segmentId: "a", timestamp: Number.MAX_SAFE_INTEGER + 1, value: 4 }]),
  ]), /invalid chart timestamp/)
})

test("isolated samples are points and not fake line stubs", () => {
  assert.deepEqual(chart.isolatedSampleIndices([10, null, 12]), [0, 2])
  assert.deepEqual(chart.isolatedSampleIndices([null, 0, undefined, 3, null]), [])
})

test("semantic scales remain explicit", () => {
  assert.deepEqual(chart.scaleRange("percent", [-2, 42, 101]), [0, 100])
  assert.deepEqual(chart.scaleRange("nonnegative", [0, 12]), [0, 20])
  assert.deepEqual(chart.scaleRange("signed", [-4, 12]), [-5, 20])
  assert.deepEqual(chart.scaleRange("signed", [-12, -4]), [-20, 0])
})

test("incompatible units and semantics receive distinct labelled scales", () => {
  const partitions = chart.scalePartitions([
    line("health", "%", "percent", []),
    line("cpu", "%", "percent", []),
    line("bytes", "B/s", "nonnegative", []),
    line("signed", "%", "signed", []),
  ])
  assert.deepEqual(partitions.map(({ label, scale, seriesIds, unit }) => [label, unit, scale, seriesIds]), [
    ["health / cpu", "%", "percent", ["health", "cpu"]],
    ["bytes", "B/s", "nonnegative", ["bytes"]],
    ["signed", "%", "signed", ["signed"]],
  ])
})

test("tooltip readings use only the exact timestamp without carrying a neighbor", () => {
  const time = chart.createDisplayTimeFormatter("en", "utc", "UTC")
  const series = [
    line("one", "%", "percent", [{ segmentId: "a", timestamp: 1, value: 4 }]),
    line("two", "B/s", "nonnegative", [{ segmentId: "a", timestamp: 2, value: 0 }]),
  ]
  const frame = chart.alignRecordedSeries(series)
  const reading = chart.exactReadings(frame, series, 2, "en", time)
  assert.deepEqual(reading.values.map(({ output }) => output), ["—", "0"])
  assert.equal(chart.nearestRecordedTimestamp([100, 180], 140), 100)
  assert.equal(chart.nearestRecordedTimestamp([100, 180], 141), 180)
  assert.equal(chart.nearestRecordedTimestamp([100, 180, 500, 900], 340), 180)
  assert.equal(chart.nearestRecordedTimestamp([100, 180, 500, 900], 340.1), 500)
  assert.match(chart.sampleText(series, frame, 2, "en", time), /two \(B\/s\): 0/)
})

test("compact chart time follows Browser or UTC mode without repeating the zone", () => {
  const timestamp = Date.UTC(2026, 10, 1, 6, 30) * 1_000
  const eastern = chart.createDisplayTimeFormatter("en", "browser", "America/New_York")
  const utc = chart.createDisplayTimeFormatter("en", "utc", "America/New_York")
  assert.equal(eastern.clock(timestamp), "01:30:00")
  assert.equal(utc.clock(timestamp), "06:30:00")
  assert.equal(chart.axisTimeLabel(timestamp, eastern), "01:30")
  assert.equal(chart.axisTimeLabel(timestamp, utc), "06:30")
  assert.doesNotMatch(eastern.clock(timestamp), /\.000|UTC|GMT/)
  assert.doesNotMatch(chart.axisTimeLabel(timestamp, eastern), /GMT|UTC/)
  const series = [line("memory", "%", "percent", [{ segmentId: "a", timestamp, value: 42 }])]
  const frame = chart.alignRecordedSeries(series)
  assert.equal(chart.exactReadings(frame, series, timestamp, "en", eastern).time, "01:30")
  assert.match(chart.sampleText(series, frame, timestamp, "en", eastern), /^01:30;/)
})

test("tooltip uses seconds only when two samples share a displayed minute", () => {
  const timestamp = Date.UTC(2026, 7, 14, 5, 30, 45) * 1_000
  const time = chart.createDisplayTimeFormatter("en", "browser", "Europe/Moscow")
  const series = [{
    ...line("memory", "%", "percent", [
      { segmentId: "a", timestamp, value: 41.729068244136855 },
      { segmentId: "a", timestamp: timestamp + 10_000_000, value: 41.729068244136855 },
    ]),
    value: chart.humanPercent,
  }]
  const frame = chart.alignRecordedSeries(series)
  assert.equal(chart.chartSecondsUseful([timestamp], time), false)
  assert.equal(chart.compactChartTime(timestamp, time, false), "08:30")
  assert.equal(chart.chartSecondsUseful(frame.timestamps, time), true)
  assert.equal(chart.compactChartTime(timestamp, time, true), "08:30:45")
  assert.equal(chart.exactReadings(frame, series, timestamp, "en", time).values[0].output, "41.7%")
  assert.match(chart.chartSummary(series, frame, timestamp, timestamp + 3_600_000_000, "en", time), /41\.7%…41\.7%/)
  assert.match(chart.sampleText(series, frame, timestamp, "en", time), /08:30:45; memory \(%\): 41\.7%/)
  assert.doesNotMatch(chart.sampleText(series, frame, timestamp, "en", time), /GMT|UTC/)
  assert.doesNotMatch(chart.sampleText(series, frame, timestamp, "en", time), /41\.729068|\.000/)
  const ru = chart.createDisplayTimeFormatter("ru", "browser", "Europe/Moscow")
  assert.match(chart.sampleText(series, frame, timestamp, "ru", ru), /41,7 %/)
})

test("y-axis labels carry only the unit, series names live in the caption", async () => {
  const source = await readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8")
  assert.match(source, /!compact && initialLine\.unit !== "" && initialLine\.tickAxis !== "duration" \? \{ label: \(\) => runtime\.current\.series/)
  assert.doesNotMatch(source, /label: `\$\{labels\}/)
  assert.match(source, /chart-series-labels/)
})

test("same-topology chart data updates the existing plot in place", async () => {
  const source = await readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8")
  assert.match(source, /chart\.setData\(latest\.frame\.data, false\)/)
  assert.match(source, /chart\.setScale\(partition\.key, \{ min, max \}\)/)
  assert.match(source, /runtime\.current\.frame\.isolated/)
})

test("the shared time scale reserves a stable right-end label gutter", () => {
  const hour = 1_700_000_000_000
  const end = hour + 3_600_000
  const [, wideEnd] = chart.chartTimeRange(hour, end, 1_200)
  const [, narrowEnd] = chart.chartTimeRange(hour, end, 360)
  assert.ok(wideEnd > end)
  assert.ok(narrowEnd > wideEnd)
  assert.deepEqual(chart.chartTimeRange(hour, end, 1_200, 0), [hour, end])
})

test("the built-in legend stays hidden and chart titles use portal help metadata", async () => {
  const source = await readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8")
  const help = await readFile(new URL("../src/help.tsx", import.meta.url), "utf8")
  assert.match(source, /legend: \{ show: false \}/)
  assert.match(source, /chart-series-labels/)
  assert.match(source, /<LabelHelp helpKey=\{line\.helpKey\}/)
  assert.match(source, /chart-series-labels[\s\S]{0,300}?overflow-x-auto/)
  assert.match(help, /\[\.chart-series-labels_&\]:flex-none/)
})

test("full charts live only in the bounded shared Inspector", async () => {
  const [source, markerSource, stylesheet, inspector, relations, system] = await Promise.all([
    readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../src/inspector.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-relations-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(source, /uplot-expanded|chart-expand|setExpanded/)
  assert.match(inspector, /className=\{`inspector\$\{maximized \? " inspector-maximized" : ""\}`\}/)
  assert.match(inspector, /data-testid="inspector-maximize"/)
  assert.match(inspector, /role="separator"/)
  assert.match(stylesheet, /@media \(max-width: 1000px\)[\s\S]*?\.inspector \{[\s\S]*?position: fixed;/)
  assert.match(stylesheet, /@media \(max-width: 520px\)[\s\S]*?max-height: min\(480px/)
  assert.match(source, /--chart-plot-top/)
  assert.match(source, /getPropertyValue\("--chart-marker-end-reserve"\)/)
  assert.match(source, /Math\.max\(1, width - endReserve\)/)
  assert.doesNotMatch(markerSource, /--chart-marker-end-reserve/)
  assert.match(stylesheet, /html \{[^}]*overflow-anchor: none;/)
  assert.match(source, /w-\[max\(1px,calc\(var\(--chart-plot-width,calc\(100%_-_70px\)\)_-_var\(--chart-marker-end-reserve,0px\)\)\)\]/)
  assert.match(relations, /<InspectorChartPortal identity=\{`pg:\$\{row\.logicalName\}/)
  assert.match(system, /<InspectorChartPortal identity=\{`system:\$\{section\}:\$\{entityRowKey\(selectedRow\)\}:history`\}/)
})
