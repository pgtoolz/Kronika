import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

const directory = dirname(fileURLToPath(import.meta.url))
const compiled = await build({
  bundle: true,
  external: ["react", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  stdin: {
    contents: 'export { numericChartPoints, pointsInHour, readingAt, sampleAtOrBefore, SeriesChart, uncollectedStart } from "../src/series-chart.tsx"; export { buildMetricSamples } from "../src/chart.ts"',
    loader: "tsx",
    resolveDir: directory,
  },
  write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const helpers = loaded.exports

test("a recorded null is the reading at its timestamp", () => {
  const points = [10, null, 12].map((value, index) => ({ segmentId: "a", timestamp: index + 1, value }))
  assert.equal(helpers.readingAt(points, 2), null)
  assert.equal(helpers.readingAt(points, 3), 12)
})

test("the shared sample builder omits absent fields but keeps explicit null and joins segments", () => {
  const rows = [
    { segmentId: "a", timestamp: 1, values: { metric: 4 } },
    { segmentId: "a", timestamp: 2, values: { other: 9 } },
    { segmentId: "a", timestamp: 3, values: { metric: null } },
    { segmentId: "a", timestamp: 4, values: { metric: 0 } },
    { segmentId: "b", timestamp: 5, values: { metric: 7 } },
  ]
  const points = helpers.buildMetricSamples(rows, (row) => Object.hasOwn(row.values, "metric") ? row.values.metric : undefined)
  assert.deepEqual(points.map(({ segmentId, timestamp, value }) => [segmentId, timestamp, value]), [
    ["a", 1, 4], ["a", 3, null], ["a", 4, 0], ["b", 5, 7],
  ])
})

test("the selected point is the stored sample at or before the cursor", () => {
  const points = [
    { segmentId: "a", timestamp: 10, value: 2 },
    { segmentId: "a", timestamp: 20, value: null },
    { segmentId: "b", timestamp: 30, value: 8 },
  ]
  assert.equal(helpers.sampleAtOrBefore(points, 19).timestamp, 10)
  assert.equal(helpers.sampleAtOrBefore(points, 20).value, null)
  assert.equal(helpers.sampleAtOrBefore(points, 9), null)
})

test("only a current hour marks its uncollected remainder", () => {
  const hour = 3_600_000_000
  const points = [{ segmentId: "a", timestamp: hour + 42, value: 1 }]
  assert.equal(helpers.uncollectedStart(points, hour, hour + 1_000), hour + 42)
  assert.equal(helpers.uncollectedStart(points, hour, hour + 3_600_000_000), null)
})

test("an all-null chart has no drawable samples while zero remains data", () => {
  assert.equal(helpers.numericChartPoints([
    { segmentId: "a", timestamp: 1, value: null },
  ]).length, 0)
  assert.equal(helpers.numericChartPoints([
    { segmentId: "a", timestamp: 1, value: 0 },
  ]).length, 1)
})

test("the frame stays while a metric resolves and collapses only for an hour without samples", async () => {
  const render = (points, status) => renderToStaticMarkup(createElement(helpers.SeriesChart, {
    helpKey: "m.help",
    hour: 0,
    labelKey: "m.label",
    locale: "en",
    points,
    status,
    t: (key) => key,
  }))
  for (const status of ["loading", "error"]) {
    const markup = render([], status)
    assert.match(markup, /class="uplot-figure /)
    assert.match(markup, /class="uplot-host /)
    assert.match(markup, /class="uplot-status /)
    assert.match(markup, new RegExp(`role="(?:alert|status)">history\\.${status === "ready" ? "empty" : status}`))
  }
  const settledEmpty = render([], "ready")
  assert.doesNotMatch(settledEmpty, /class="uplot-figure /)
  assert.match(settledEmpty, /class="series-reading /)
  assert.match(settledEmpty, /text-fg4[^"]*" role="status">history\.empty/)
  const ready = render([{ segmentId: "a", timestamp: 1, value: 2 }], "ready")
  assert.match(ready, /class="uplot-figure /)
  assert.doesNotMatch(ready, /uplot-status/)
  const loadingWithData = render([{ segmentId: "a", timestamp: 1, value: 2 }], "loading")
  assert.match(loadingWithData, /class="uplot-status /)
  assert.match(loadingWithData, /history\.loading/)

  const [source, chart, styles] = await Promise.all([
    readFile(new URL("../src/series-chart.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.equal((source.match(/<UPlotChart/g) ?? []).length, 1)
  assert.doesNotMatch(source, /hasData\s*\?/)
  assert.match(chart, /status !== undefined && <div className="uplot-status /)
  assert.match(chart, /uplot-figure[^`]*h-\[200px\]/)
  assert.match(chart, /uplot-status pointer-events-none absolute/)
  assert.match(chart, /chart-tooltip pointer-events-none absolute/)
  assert.match(chart, /data-testid="chart-hover-readout"/)
})

test("a series uses only selected-hour points for emptiness, readout, and plotting", async () => {
  const hour = 3_600_000_000
  const prior = { segmentId: "a", timestamp: hour - 1, value: 91 }
  const priorSecond = { segmentId: "a", timestamp: hour - 2, value: 55 }
  const emptyPrimary = helpers.pointsInHour([prior], hour)
  const emptySecond = helpers.pointsInHour([priorSecond], hour)
  assert.deepEqual(emptyPrimary, [])
  assert.deepEqual(emptySecond, [])
  assert.equal(helpers.numericChartPoints(emptyPrimary, emptySecond).length, 0)
  assert.equal(helpers.readingAt(emptyPrimary, hour + 10), null)

  const plottedPrimary = helpers.pointsInHour([
    prior,
    { segmentId: "b", timestamp: hour + 1, value: 7 },
  ], hour)
  const plottedSecond = helpers.pointsInHour([priorSecond], hour)
  assert.deepEqual(plottedPrimary.map(({ value }) => value), [7])
  assert.deepEqual(plottedSecond, [])
  assert.equal(helpers.numericChartPoints(plottedPrimary, plottedSecond).length, 1)
  assert.equal(helpers.readingAt(plottedPrimary, hour + 10), 7)
  assert.deepEqual(helpers.pointsInHour([
    { ...prior, timestamp: hour - 1 },
    { ...prior, timestamp: hour },
    { ...prior, timestamp: hour + 3_600_000_000 - 1 },
    { ...prior, timestamp: hour + 3_600_000_000 },
  ], hour).map(({ timestamp }) => timestamp), [hour, hour + 3_600_000_000 - 1])

  const source = await readFile(new URL("../src/series-chart.tsx", import.meta.url), "utf8")
  assert.match(source, /points: pointsInHour\(points, hour\)[\s\S]*second: second === undefined \? undefined : pointsInHour\(second, hour\)/)
  assert.match(source, /numericChartPoints\(visible\.points, visible\.second\)/)
  assert.match(source, /readingAt\(visible\.points, cursor\)/)
  assert.match(source, /points: visible\.points[\s\S]*points: visible\.second/)
})
