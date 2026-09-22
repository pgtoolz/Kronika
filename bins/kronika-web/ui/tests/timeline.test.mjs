import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { build } from "esbuild"

import { importModule, registryPlugin } from "./import-module.mjs"

const helpers = await importModule(
  'export { CursorRow } from "../src/cursor-row.tsx"; export { FindingMarker, LockGraphMarker, MARKER_CLUSTER_PX, exactValue, findingShape, findingTrack, groupFindings, groupTimedMarkers, healthEvaluationAtOrBefore, healthThreshold, healthTimelineSeries, laneReading, sampleWindow, timelineDecorations, timelineNavigationTimes, timelineRecordedTimes, timelineSeriesHelpKey } from "../src/timeline.tsx"',
  { plugins: [registryPlugin([{ typeId: "1104001", logicalName: "os_meminfo", columns: ["ts", "mem_total", "mem_free", "mem_available"] }])] },
)

const rendered = await build({
  bundle: true, format: "cjs", platform: "node", write: false,
  external: ["react", "react-dom", "react/jsx-runtime"],
  plugins: [registryPlugin([])],
  stdin: { loader: "tsx", resolveDir: dirname(fileURLToPath(import.meta.url)), contents: `
  import { createElement } from "react";
  import { renderToStaticMarkup } from "react-dom/server";
  import { Timeline, TimelineRequestContext } from "../src/timeline.tsx";
  export function render(phase, health = [], presentation = "preview", options = {}) {
    return renderToStaticMarkup(createElement(TimelineRequestContext, { value: phase },
      createElement(Timeline, { cursor: 200, hour: 0, environment: null, findings: [], health,
        lanePoints: [], locale: "en", presentation, onCursor() {}, onFinding() {}, t: (key) => key, ...options })));
  }
` },
})
const loaded = { exports: {} }
new Function("module", "exports", "require", rendered.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const requestTimeline = loaded.exports

test("deferred timeline data stays pending or failed until a successful empty response", () => {
  for (const presentation of ["preview", "inspector"]) {
    const pending = requestTimeline.render("pending", [], presentation)
    assert.match(pending, /aria-busy="true"/)
    assert.match(pending, /role="status"[^>]*>status.loading/)
    assert.doesNotMatch(pending, /status.no_data/)
    const failed = requestTimeline.render("error", [], presentation)
    assert.match(failed, /role="alert"[^>]*>status.error/)
    assert.doesNotMatch(failed, /status.no_data/)
    const empty = requestTimeline.render("ready", [], presentation)
    assert.match(empty, /status.no_data_completed/)
    for (const markup of [pending, failed, empty]) assert.match(markup, /h-\[124px\]/)
    const retained = requestTimeline.render("pending", [{ segmentId: "a", timestamp: 100,
      logicalName: "health", typeId: "0", ordinal: "0", values: { overall_health: 80 } }], presentation)
    assert.match(retained, /timeline-shell/)
    assert.doesNotMatch(retained, /status.loading|status.no_data/)
  }
})

function finding(kind, timestamp, ordinal) {
  return {
    category: null,
    fieldOrdinal: 0,
    kind,
    logicalName: "os_process",
    rowOrdinal: ordinal,
    segmentId: "segment-a",
    timestamp,
    typeId: "1100001",
  }
}

test("the shared empty timeline uses the hour-aware status", async () => {
  const source = await readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8")
  assert.match(source, /emptyHourStatusKey\(hour\)/)
})

test("the selected lane draws while shared step controls use source navigation", async () => {
  const [app, keyboard, timeline] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/keyboard.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(app, /moveCursor/)
  assert.doesNotMatch(keyboard, /60_000_000|MINUTE/)
  assert.match(timeline, /timelineNavigationTimes\(lanes\)/)
  assert.match(timeline, /navigationTimestamps=\{cursorTimes\}/)
  assert.match(timeline, /moveCursor\(cursor, cursorTimes, event\.key\)/)
  assert.match(timeline, /navigation\.step\(event\.key === "ArrowRight" \? "next" : "previous"\)/)
  assert.match(timeline, /onStep=\{navigation\?\.step\}/)
  assert.doesNotMatch(timeline, /mergeObservationTimestamps/)
  assert.match(timeline, /<UPlotChart/)
  assert.match(timeline, /window\.addEventListener\("keydown", move\)/)
  assert.match(timeline, /previousPrimary\.current = primaryLane\s+if \(controlledLane === undefined\) setLocalLane\(primaryLane\)/)
})

test("the mobile cursor row keeps navigation and the live reading without a second clock", () => {
  const markup = renderToStaticMarkup(createElement(helpers.CursorRow, {
    cursor: 200,
    cursorTimes: [100, 250, 300],
    onCursor() {},
    reading: "48%",
    t: (key) => ({ "hour.cursor_next": "Next", "hour.cursor_previous": "Previous" })[key] ?? key,
  }))
  assert.match(markup, /48%/)
  assert.doesNotMatch(markup, /cursor-row-time|T100|T200|T250|T300|Cursor|Recorded|Data at/)
})

test("the desktop header previews the same cursor without announcing pointer travel", async () => {
  const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(app, /data-testid="cursor-time" ref=\{cursorClock\}>\{cursor === 0 \? "—" : time\.clock\(cursor\)\}<\/span>/)
  assert.match(app, /const previewClock = useCallback/)
  assert.doesNotMatch(app, /aria-live="polite" className="cursor-time|shownAt|hour\.cursor_label|hour\.recorded_label|UpdatedAge|lastUpdated|updated-help/)
})

test("shared chart movement previews while pointerup alone commits", async () => {
  const chart = await readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8")
  assert.match(chart, /addEventListener\("pointermove", follow\)/)
  assert.match(chart, /onPreviewRef\.current\?\.\(timestamp\)/)
  assert.match(chart, /addEventListener\("pointerleave", clearPreview\)/)
  assert.match(chart, /addEventListener\("pointercancel", clearPreview\)/)
  assert.match(chart, /addEventListener\("pointerup", select\)/)
  const follow = chart.slice(chart.indexOf("const follow ="), chart.indexOf("const select ="))
  assert.match(follow, /onPreviewRef\.current/)
  assert.doesNotMatch(follow, /onCursorRef\.current/)
})

test("one timeline mark stays an unlabeled shape", () => {
  const [marker] = helpers.groupFindings([finding("event", 100, "1")], 0, 1_000, 100, 10)
  assert.equal(marker.findings.length, 1)
  assert.deepEqual(marker.composition, [{ count: 1, kind: "event" }])
  assert.deepEqual(marker.findings, [finding("event", 100, "1")])
  const markup = renderToStaticMarkup(createElement(helpers.FindingMarker, { marker, onActivate() {}, share: 0.1, t: (key) => key }))
  assert.match(markup, /data-marker-shape="circle"[^>]*aria-hidden="true"|aria-hidden="true"[^>]*data-marker-shape="circle"/)
  assert.equal(markup.replace(/<[^>]+>/g, ""), "")
})

test("dense coincident marks become one compact count", () => {
  const input = [finding("event", 100, "3"), finding("event", 100, "1"), finding("event", 100, "2")]
  const [marker] = helpers.groupFindings(input, 0, 1_000, 100, 10)
  assert.equal(marker.findings.length, 3)
  assert.deepEqual(marker.composition, [{ count: 3, kind: "event" }])
  assert.deepEqual(marker.findings.map(({ rowOrdinal }) => rowOrdinal), ["1", "2", "3"])
})

test("timeline markers cluster at rendered density with exact ordered locators", () => {
  const input = [
    finding("spike", 150, "3"),
    finding("event", 100, "1"),
    finding("known_bad", 100, "2"),
    finding("event", 205, "4"),
    finding("event", 900, "5"),
  ]
  const grouped = helpers.groupFindings(input, 0, 1_000, 100, 10)

  assert.deepEqual(grouped.map(({ composition, findings }) => ({
    composition,
    count: findings.length,
    endTimestamp: findings.at(-1)?.timestamp,
    kinds: composition.map(({ kind }) => kind),
    startTimestamp: findings[0]?.timestamp,
  })), [
    { composition: [{ count: 1, kind: "event" }, { count: 1, kind: "known_bad" }, { count: 1, kind: "spike" }], count: 3, kinds: ["event", "known_bad", "spike"], startTimestamp: 100, endTimestamp: 150 },
    { composition: [{ count: 1, kind: "event" }], count: 1, kinds: ["event"], startTimestamp: 205, endTimestamp: 205 },
    { composition: [{ count: 1, kind: "event" }], count: 1, kinds: ["event"], startTimestamp: 900, endTimestamp: 900 },
  ])
  const locator = (item) => `${item.segmentId}:${item.typeId}:${item.rowOrdinal}:${item.fieldOrdinal}:${item.timestamp}:${item.kind}`
  assert.deepEqual(
    grouped.flatMap((marker) => marker.findings).map(locator),
    [input[1], input[2], input[0], input[3], input[4]].map(locator),
  )
})

test("duplicate locators keep their exact multiplicity and order", () => {
  const duplicate = finding("event", 100, "1")
  const [marker] = helpers.groupFindings([duplicate, finding("spike", 100, "2"), duplicate], 0, 1_000, 100, 10)
  assert.deepEqual(marker.findings, [duplicate, duplicate, finding("spike", 100, "2")])
})

test("marker clustering is deterministic and separates locators when more pixels are available", () => {
  const input = [
    finding("spike", 150, "3"),
    finding("event", 100, "1"),
    finding("known_bad", 100, "2"),
    finding("event", 205, "4"),
  ]
  const snapshot = (groups) => groups.map((marker) => ({
    count: marker.count,
    locators: marker.findings.map((item) => `${item.timestamp}:${item.kind}:${item.rowOrdinal}`),
    timestamp: marker.timestamp,
  }))
  const compact = helpers.groupFindings(input, 0, 1_000, 100, 10)
  assert.deepEqual(snapshot(compact), snapshot(helpers.groupFindings(input.toReversed(), 0, 1_000, 100, 10)))
  assert.equal(compact.length, 2)
  assert.deepEqual(
    helpers.groupFindings(input, 0, 1_000, 1_000, 10).map((marker) => marker.findings.length),
    [2, 1, 1],
  )
})

test("mixed clusters show each semantic count without a raw locator total", async () => {
  const findings = [
    ...Array.from({ length: 12 }, (_, index) => finding("event", 100, String(index))),
    ...Array.from({ length: 11 }, (_, index) => finding("known_bad", 100, String(index + 20))),
    ...Array.from({ length: 10 }, (_, index) => finding("spike", 100, String(index + 40))),
  ]
  const [marker] = helpers.groupFindings(findings, 0, 1_000, 100, 10)
  const markup = renderToStaticMarkup(createElement(helpers.FindingMarker, {
    marker,
    onActivate() {},
    share: 0.1,
    t: (key, slots = {}) => ({
      "events.scope.event": `Log events: ${slots.count}`,
      "events.scope.known_bad": `Threshold crossings: ${slots.count}`,
      "events.scope.spike": `Sharp rises: ${slots.count}`,
    })[key] ?? key,
  }))
  assert.equal((markup.match(/data-marker-shape=/g) ?? []).length, 3)
  assert.match(markup, /data-marker-composition="event:12 known_bad:11 spike:10"/)
  assert.doesNotMatch(markup, />33</)
  assert.match(markup, /aria-label="Log events: 12 · Threshold crossings: 11 · Sharp rises: 10 · 100"/)
  assert.match(markup, new RegExp(`clamp\\(${helpers.MARKER_CLUSTER_PX / 2}px`))
  assert.doesNotMatch(markup.replace(/aria-label="[^"]*"|title="[^"]*"/g, ""), /Event|Known bad|Spike|Process/)
  const [source, styles] = await Promise.all([
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.doesNotMatch(source, /marker-cluster-summary|className="finding-rail"/)
  assert.doesNotMatch(styles, /\.marker-cluster-summary|\.finding-rail|\.neutral-rail/)
  assert.match(source, /marker-cluster-badge/)
  assert.deepEqual(helpers.groupFindings([], 0, 1_000, 100), [])
})

test("a log severity locator does not count the same physical log row twice", () => {
  const event = { ...finding("event", 100, "1"), logicalName: "pg_log_slow_queries", typeId: "2004001" }
  const severity = { ...event, kind: "known_bad" }
  const [marker] = helpers.groupFindings([event, severity], 0, 1_000, 100, 10)
  assert.equal(marker.findings.length, 2)
  assert.deepEqual(marker.composition, [{ count: 1, kind: "event" }])
  const markup = renderToStaticMarkup(createElement(helpers.FindingMarker, {
    marker,
    onActivate() {},
    share: 0.1,
    t: (key, slots = {}) => key === "events.scope.event" ? `Log events: ${slots.count}` : key,
  }))
  assert.match(markup, /data-marker-count="1"/)
  assert.match(markup, /data-marker-locator-count="2"/)
  assert.match(markup, /data-marker-composition="event:1"/)
  assert.doesNotMatch(markup, /marker-cluster-badge/)
})

test("finding kinds have non-color shape identities", () => {
  assert.equal(helpers.findingShape("event"), "circle")
  assert.equal(helpers.findingShape("known_bad"), "diamond")
  assert.equal(helpers.findingShape("spike"), "triangle")
  const render = (kind) => {
    const [marker] = helpers.groupFindings([finding(kind, 100, "1")], 0, 1_000, 100, 10)
    return renderToStaticMarkup(createElement(helpers.FindingMarker, { marker, onActivate() {}, share: 0.1, t: (key) => key }))
  }
  assert.match(render("event"), /fill="var\(--color-event\)"/)
  assert.match(render("known_bad"), /fill="var\(--color-bad\)"/)
  assert.match(render("spike"), /stroke="var\(--color-warn\)"/)
})

test("health metrics share stored evaluation timestamps and a strict nonfuture cursor", () => {
  const rows = [
    { logicalName: "health", ordinal: "0", segmentId: "a", timestamp: 103, typeId: "0", values: { os_health: 90, overall_health: 62, postgres_health: 72 } },
    { logicalName: "health", ordinal: "1", segmentId: "a", timestamp: 109, typeId: "0", values: { os_health: 90, overall_health: 45, postgres_health: 55 } },
  ]
  const health = helpers.healthTimelineSeries(rows)
  assert.deepEqual(health.series.map(({ field, points }) => [field, points.map(({ timestamp, value }) => [timestamp, value])]), [
    ["overall_health", [[103, 62], [109, 45]]],
    ["os_health", [[103, 90], [109, 90]]],
    ["postgres_health", [[103, 72], [109, 55]]],
  ])
  assert.equal(health.threshold, 50)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 102), null)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 105), 103)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 106), 103)
  assert.equal(helpers.healthEvaluationAtOrBefore(health.series, 109), 109)

  const osOnly = helpers.healthTimelineSeries([
    { logicalName: "health", ordinal: "3", segmentId: "c", timestamp: 300, typeId: "0", values: { os_health: 73, overall_health: 73 } },
  ])
  assert.deepEqual(osOnly.series.map(({ field }) => field), ["overall_health", "os_health"])
  assert.equal(osOnly.series.some(({ field }) => field === "postgres_health"), false)

  const container = helpers.healthTimelineSeries([
    { logicalName: "health", ordinal: "4", segmentId: "d", timestamp: 400, typeId: "0", values: { os_health: null, overall_health: null, postgres_health: 100 } },
    { logicalName: "health", ordinal: "5", segmentId: "d", timestamp: 410, typeId: "0", values: { os_health: null, overall_health: null, postgres_health: 95 } },
  ])
  assert.deepEqual(container.series.map(({ field }) => field), ["postgres_health"])
  assert.equal(container.threshold, undefined)
})

test("all shared lanes own exact heterogeneous navigation timestamps", () => {
  const health = [{ points: [
    { timestamp: 100, value: 90 },
    { timestamp: 105, value: null },
    { timestamp: 111, value: 88 },
  ] }, { points: [
    { timestamp: 105, value: 91 },
    { timestamp: 118, value: 89 },
  ] }]
  const cpu = [{ points: [
    { timestamp: 102, value: 20 },
    { timestamp: 107, value: 21 },
  ] }]
  const lanes = [{ key: "health", series: health }, { key: "cpu_busy", series: cpu }]
  assert.deepEqual(helpers.timelineNavigationTimes(lanes), [100, 102, 105, 107, 111, 118])
})

test("timeline distinguishes unavailable edges from the uncollected current-hour tail", () => {
  const hour = Date.UTC(2026, 7, 13, 10) * 1_000
  const end = hour + 3_600_000_000
  const point = (offset, value) => ({ segmentId: "a", timestamp: hour + offset, value })
  const selected = [{ points: [point(600_000_000, 10), point(1_800_000_000, null), point(2_100_000_000, 12)] }]
  const lanes = [
    { series: selected },
    { series: [{ points: [point(300_000_000, 7), point(2_400_000_000, 9)] }] },
  ]
  assert.deepEqual(helpers.sampleWindow(lanes), { start: hour + 300_000_000, end: hour + 2_400_000_000 })
  assert.deepEqual(helpers.timelineDecorations(lanes, selected, hour, end, hour + 3_000_000_000), [
    { from: hour, to: hour + 300_000_000, tone: "unavailable" },
    { from: hour + 2_400_000_000, to: end, tone: "unavailable" },
    { from: hour + 2_100_000_000, to: end, tone: "future" },
  ])
  assert.deepEqual(helpers.timelineDecorations(lanes, selected, hour, end, end + 1), [
    { from: hour, to: hour + 300_000_000, tone: "unavailable" },
    { from: hour + 2_400_000_000, to: end, tone: "unavailable" },
  ])
})

test("health lane readings use only an exact observation", () => {
  const points = [10, null, 12].map((value, index) => ({ segmentId: "a", timestamp: index + 1, value }))
  assert.equal(helpers.exactValue(points, 2), null)
  assert.equal(helpers.exactValue(points, 3), 12)
  assert.equal(helpers.exactValue(points, 4), null)
})

test("a slow lane stays sample-at-or-before at faster cursor positions", () => {
  const lane = {
    key: "pg_running",
    series: [{ color: "cyan", field: "pg_running", points: [
      { segmentId: "a", timestamp: 100, value: 3 },
      { segmentId: "a", timestamp: 130, value: 7 },
    ] }],
  }
  const t = (key) => key
  assert.equal(helpers.laneReading(lane, 99, "en", t), "—")
  assert.equal(helpers.laneReading(lane, 100, "en", t), "3")
  assert.equal(helpers.laneReading(lane, 115, "en", t), "3")
  assert.equal(helpers.laneReading(lane, 130, "en", t), "7")
})

test("a transaction age lane reading scales durations like table cells do", () => {
  const lane = {
    key: "oldest_xact",
    series: [{ color: "violet", field: "pg_oldest_xact", points: [
      { segmentId: "a", timestamp: 100, value: 0.00509 },
      { segmentId: "a", timestamp: 130, value: 3725 },
    ] }],
  }
  const t = (key) => key
  assert.equal(helpers.laneReading(lane, 115, "ru", t), "5,09 мс")
  assert.equal(helpers.laneReading(lane, 130, "ru", t), "1,03 ч")
})

test("a health lane reading keeps the shared evaluation timestamp", () => {
  const lane = {
    key: "health",
    series: [
      { color: "cyan", field: "overall_health", points: [
        { segmentId: "a", timestamp: 100, value: 62 },
        { segmentId: "a", timestamp: 130, value: 45 },
      ] },
      { color: "amber", field: "os_health", points: [
        { segmentId: "a", timestamp: 100, value: 90 },
        { segmentId: "a", timestamp: 130, value: 80 },
      ] },
    ],
  }
  const t = (key) => key
  assert.equal(helpers.laneReading(lane, 99, "en", t), "lane.health.overall_health — · lane.health.os_health —")
  assert.equal(helpers.laneReading(lane, 115, "en", t), "lane.health.overall_health 62% · lane.health.os_health 90%")
})

test("health series resolve distinct help while other series keep lane help", () => {
  assert.deepEqual(
    ["overall_health", "os_health", "postgres_health"].map((field) => helpers.timelineSeriesHelpKey("health", field)),
    ["lane.health.overall_health.help", "lane.health.os_health.help", "lane.health.postgres_health.help"],
  )
  assert.equal(helpers.timelineSeriesHelpKey("cpu_busy", "cpu_busy"), "lane.cpu_busy.help")
})

test("only overall health owns the below-50 band and exact findings map to tracks", () => {
  assert.equal(helpers.healthThreshold("overall_health"), 50)
  assert.equal(helpers.healthThreshold("os_health"), null)
  assert.equal(helpers.healthThreshold("postgres_health"), null)
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "health", typeId: "0", fieldOrdinal: 1 }), "health")
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "health", typeId: "0", fieldOrdinal: 0 }), null)
  assert.equal(helpers.findingTrack({ ...finding("known_bad", 100, "1"), logicalName: "os_meminfo", typeId: "1104001", fieldOrdinal: 3 }), "memory")
  assert.equal(helpers.groupFindings([finding("event", 100, "1")], 0, 1_000, 100)[0].findings[0]?.kind, "event")
  assert.equal(helpers.groupFindings([finding("spike", 100, "1")], 0, 1_000, 100)[0].findings[0]?.kind, "spike")
})

test("the renderer is exclusively the shared uPlot adapter", async () => {
  const source = await readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8")
  assert.match(source, /<UPlotChart/)
  assert.doesNotMatch(source, /SeriesLine|svgPath|timelineRuns|preserveAspectRatio/)
  assert.ok(source.indexOf("if (selected === undefined)") > source.indexOf("const threshold = useMemo"))
})

test("timeline controls stay above a full-width plot without a redundant time title", async () => {
  const [source, styles, chart] = await Promise.all([
    readFile(new URL("../src/timeline.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../src/uplot-chart.tsx", import.meta.url), "utf8"),
  ])
  const railMarkup = 'className="timeline-rail flex h-7 min-w-0 flex-none overflow-hidden border-b border-line2"'
  assert.match(source, /timeline-shell[^"]*flex-col[^"]*overflow-hidden/)
  assert.ok(source.includes(railMarkup))
  assert.match(source, /className="timeline-lanes[^"]*overflow-hidden/)
  assert.doesNotMatch(source, /className="timeline-lanes[^"]*overflow-x-auto/)
  assert.match(source, /data-testid="timeline-preview-metric-select"/)
  assert.match(source, /aria-label=\{accessible\}/)
  // Lanes size to their content so a short selected reading does not starve a long one.
  assert.match(styles, /\.timeline-lane-label \{ flex: 1 1 auto; \}/)
  assert.doesNotMatch(styles, /data-primary="true"\] \{[^}]*flex:/)
  // Clipped readings hide before the picker replaces the lane names.
  assert.match(styles, /\.timeline-lane-slot\[data-density="names"\] \.timeline-lane-label:not\(\[data-primary="true"\]\) \.timeline-lane-reading \{ display: none; \}/)
  assert.match(source, /setLaneDensity\(laneDensity === "full" \? "names" : "picker"\)/)
  assert.match(source, /data-compact=\{compactPicker \|\| undefined\} data-density=\{laneDensity\}/)
  assert.match(styles, /\.timeline-open-chart \{[^}]*flex: 0 0 64px;[^}]*width: 64px;/s)
  assert.match(styles, /\.timeline-preview \{[^}]*height: 124px;/s)
  assert.match(chart, /variant === "preview" \? "h-\[94px\]/)
  assert.doesNotMatch(styles, /timeline-shell[^}]*uplot-host \{ min-height:/)
  // The lane strip renders above the plot; comparing by a class name that no
  // longer exists made this pass on two -1s.
  assert.ok(source.indexOf(railMarkup) < source.indexOf('className="timeline-chart"'))
  assert.doesNotMatch(chart, /Time, browser local|Время, местное в браузере/)
})


test("automatic lanes remain automatic while explicit unavailable lanes keep their picker", () => {
  const lanePoints = [{ segmentId: "s", lane: "pg_waiting", timestamp: 200, value: 3 }]
  const automatic = requestTimeline.render("ready", [], "preview", { selectedLane: null, primaryLane: "pg_waiting", lanePoints })
  assert.match(automatic, /aria-pressed="true"[^>]*>[^]*?lane.pg_waiting.label/)
  const pending = requestTimeline.render("pending", [], "preview", { selectedLane: null, primaryLane: "pg_waiting" })
  assert.match(pending, /value="pg_waiting"/)
  for (const presentation of ["preview", "inspector"]) {
    const unavailable = requestTimeline.render("ready", [], presentation, { selectedLane: "disk_busy", lanePoints })
    assert.match(unavailable, /value="disk_busy"[^>]*>lane.disk_busy.label/)
    assert.match(unavailable, /value="pg_waiting"[^>]*>lane.pg_waiting.label/)
    assert.match(unavailable, /status.no_data/)
  }
})

test("each lane reserves its widest reading of the hour so pointer travel never reflows the strip", () => {
  const lanePoints = [
    { segmentId: "s", lane: "pg_waiting", timestamp: 100, value: 3 },
    { segmentId: "s", lane: "pg_waiting", timestamp: 300, value: 1234 },
  ]
  const html = requestTimeline.render("ready", [], "preview", { selectedLane: null, primaryLane: "pg_waiting", lanePoints })
  assert.match(html, /data-testid="lane-reading"[^>]*><span>3<\/span><span aria-hidden="true">1\.23K<\/span>/)
})

test("an unavailable automatic lane falls back to the first recorded lane", () => {
  const lanePoints = [{ segmentId: "s", lane: "pg_waiting", timestamp: 200, value: 3 }]
  for (const presentation of ["preview", "inspector"]) {
    const html = requestTimeline.render("ready", [], presentation, { selectedLane: null, primaryLane: "health", lanePoints })
    assert.match(html, /value="pg_waiting" selected=""/)
    assert.doesNotMatch(html, /value="health"|status.no_data/)
  }
  const pending = requestTimeline.render("pending", [], "preview", { selectedLane: null, primaryLane: "health" })
  assert.match(pending, /value="health" selected=""/)
})

test("Disk and Locks choices expose recorded point identity, same-device queue and graph markers", () => {
  const device = { major: 8, minor: 0, name: "sda", scope: 0 }
  const lanePoints = [
    { segmentId: "s", lane: "disk_busy", timestamp: 200, value: 60, device },
    { segmentId: "s", lane: "disk_queue", timestamp: 200, value: 0.8, device },
    { segmentId: "s", lane: "pg_lock_waiting", timestamp: 200, value: 0 },
    { segmentId: "s", lane: "pg_lock_graph", timestamp: 199, value: null, locks: { waiting: 2, blockers: 1, prepared: true } },
  ]
  const disk = requestTimeline.render("ready", [], "preview", { selectedLane: "disk_busy", environment: "machine", lanePoints })
  assert.match(disk, /60% · sda · use.lane.disk_queue 0.8/)
  assert.match(disk, /lane.pg_lock_waiting.label/)
  const locks = requestTimeline.render("ready", [], "preview", { selectedLane: "pg_lock_waiting", environment: "machine", lanePoints })
  assert.match(locks, /data-testid="lock-graph-marker"/)
  assert.match(locks, /lane.pg_lock_waiting.prepared/)
  assert.match(locks, /aria-label="lane.pg_lock_waiting.label, count"/)
  assert.match(locks, /data-testid="timeline-preview-reading" title="0">0<\/span>/)
  const host = requestTimeline.render("ready", [], "preview", { selectedLane: "host_disk", environment: "container", lanePoints })
  assert.match(host, /lane.host_disk.label/)
  assert.doesNotMatch(host, /value="disk_busy"/)
})


test("Host Disk uses the same scope-filtered point for reading and queue", () => {
  const device = { major: 8, minor: 0, name: "host-disk", scope: 0 }
  const lanePoints = [
    { segmentId: "s", lane: "disk_busy", timestamp: 100, value: 60, device },
    { segmentId: "s", lane: "disk_queue", timestamp: 100, value: 0.8, device },
    { segmentId: "s", lane: "disk_busy", timestamp: 200, value: 99, device: { ...device, name: "unknown", scope: null } },
  ]
  const html = requestTimeline.render("ready", [], "preview", { selectedLane: "host_disk", environment: "container", lanePoints })
  assert.match(html, /60% · host-disk · use.lane.disk_queue 0.8/)
  assert.doesNotMatch(html, /99%|unknown/)
})


test("Locks marker clusters retain every exact graph at narrow and wide plot widths", () => {
  const points = [0, 45_000_000, 50_000_000, 1_800_000_000, 3_599_000_000].map((timestamp, index) => ({
    segmentId: "s", lane: "pg_lock_graph", timestamp, value: null,
    locks: { waiting: index + 1, blockers: 1, prepared: index === 2 },
  }))
  for (const width of [280, 720, 1200]) {
    const groups = helpers.groupTimedMarkers(points, 0, 3_600_000_000, width)
    assert.deepEqual(groups.flat(), points)
    assert.deepEqual(groups[0], points.slice(0, 3))
    assert.equal(groups[1].length, 1)
    const anchors = groups.map(group => Math.max(44, Math.min(width - 44, group[0].timestamp / 3_600_000_000 * width)))
    for (let i = 1; i < anchors.length; i++) assert.ok(anchors[i] - anchors[i - 1] > helpers.MARKER_CLUSTER_PX)
  }
  const t = (key, values) => key + (values === undefined ? "" : JSON.stringify(values))
  const grouped = renderToStaticMarkup(createElement(helpers.LockGraphMarker, { points: points.slice(0, 3), share: 0, onActivate() {}, t, time: String }))
  assert.match(grouped, /<select[^>]*data-testid="lock-graph-cluster"/)
  assert.match(grouped, /<option[^>]*disabled=""[^>]*value=""[^>]*>◆ \+3<\/option>/)
  for (const point of points.slice(0, 3)) assert.ok(grouped.includes(`value="${point.timestamp}"`))
  assert.match(grouped, /lane.pg_lock_waiting.prepared/)
  assert.match(grouped, /clamp\(44px, 0%, calc\(100% - 44px\)\)/)
  const single = renderToStaticMarkup(createElement(helpers.LockGraphMarker, { points: [points[3]], share: 0.5, onActivate() {}, t, time: String }))
  assert.match(single, /<button[^>]*data-testid="lock-graph-marker"/)
  assert.doesNotMatch(single, /<select/)
})


test("the Locks lane reserves markers for captured graphs while other lanes keep findings", () => {
  const lanePoints = [
    { segmentId: "s", lane: "pg_lock_waiting", timestamp: 200, value: 2 },
    { segmentId: "s", lane: "pg_lock_graph", timestamp: 199, value: null, locks: { waiting: 2, blockers: 1, prepared: false } },
    { segmentId: "s", lane: "pg_waiting", timestamp: 200, value: 2 },
  ]
  const findings = [finding("known_bad", 199, "1")]
  const locks = requestTimeline.render("ready", [], "preview", { selectedLane: "pg_lock_waiting", lanePoints, findings })
  assert.match(locks, /data-testid="lock-graph-marker"/)
  assert.doesNotMatch(locks, /data-marker-count/)
  const waiting = requestTimeline.render("ready", [], "preview", { selectedLane: "pg_waiting", lanePoints, findings })
  assert.match(waiting, /data-marker-count="1"/)
  assert.doesNotMatch(waiting, /data-testid="lock-graph-marker"/)
})
