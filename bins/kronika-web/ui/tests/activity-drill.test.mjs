import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const activity = await importModule(
  'export { ActivityStrip, cgroupActivityIdentity, cgroupIoSharedPath, cursorColumnOf, intervalInstant, planTextsByPlanId, rowPeakColumn, statementTextsByQueryId } from "../src/activity.tsx"; export { activityPreview } from "../src/activity-cuts.ts"',
  { plugins: [registryPlugin([])] },
)

const HOUR = 1_000_000_000_000
const HOUR_MICROS = 3_600_000_000

function intervals(from, toExclusive, columns) {
  return Array.from({ length: columns }, (_, index) => ({
    start: from + Math.floor(((toExclusive - from) * index) / columns),
    end: from + Math.floor(((toExclusive - from) * (index + 1)) / columns) - 1,
  }))
}

for (const columns of [12, 60]) {
  for (const partial of [false, true]) {
    test(`${columns}-column ${partial ? "partial" : "full"} hour binds rendered cells, cursor and drill to returned times`, () => {
      const from = partial ? HOUR + 2_122_238_000 : HOUR
      const to = partial ? from + 600_343_350 : HOUR + HOUR_MICROS
      const recorded = intervals(from, to, columns)
      const cells = recorded.map((_, index) => index + 1)
      const picked = []
      const strip = activity.ActivityStrip({ cells, cursor: from, hour: HOUR, intervals: recorded, max: columns, onCursor: (at) => picked.push(at) })
      const rects = strip.props.children[0]
      assert.equal(strip.props.viewBox, "0 0 100 8")
      for (const index of [0, Math.floor(columns / 2), columns - 1]) {
        const interval = recorded[index]
        const width = ((interval.end - interval.start + 1) / HOUR_MICROS) * 100
        assert.equal(rects[index].props.x, ((interval.start - HOUR) / HOUR_MICROS) * 100 + width * 0.05)
        assert.equal(rects[index].props.width, width * 0.9)
        assert.equal(activity.cursorColumnOf(interval.start, recorded), index)
        assert.equal(activity.cursorColumnOf(interval.end, recorded), index)
        const time = Math.floor((interval.start + interval.end) / 2)
        strip.props.onClick({ stopPropagation() {}, clientX: 10 + ((time - HOUR) / HOUR_MICROS) * 1000, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 1000 }) } })
        assert.equal(picked.at(-1), interval.end)
        assert.equal(cells[activity.cursorColumnOf(picked.at(-1), recorded)], cells[index])
      }
      assert.equal(activity.intervalInstant(recorded, activity.rowPeakColumn(cells)), to - 1)
      assert.equal(activity.cursorColumnOf(from - 1, recorded), null)
      assert.equal(activity.cursorColumnOf(to, recorded), null)
      if (partial) {
        strip.props.onClick({ stopPropagation() {}, clientX: 10, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 1000 }) } })
        assert.equal(picked.length, 3)
        assert.ok(rects[0].props.x > 58)
        assert.ok(rects.at(-1).props.x + rects.at(-1).props.width < 76)
      }
    })
  }
}

test("returned gaps, null cells and intervals outside the hour remain empty", () => {
  const recorded = [{ start: HOUR - 10, end: HOUR - 1 }, { start: HOUR + 10, end: HOUR + 19 }, { start: HOUR + 30, end: HOUR + 39 }, { start: HOUR + HOUR_MICROS, end: HOUR + HOUR_MICROS + 9 }]
  const strip = activity.ActivityStrip({ cells: [1, null, 0, 2], cursor: HOUR + 25, hour: HOUR, intervals: recorded, max: 2, onCursor: () => assert.fail("gap has no target") })
  assert.deepEqual(strip.props.children[0].map((cell) => cell !== null), [false, false, true, false])
  assert.equal(activity.cursorColumnOf(HOUR + 25, recorded), null)
  assert.equal(activity.intervalInstant(recorded, 4), null)
  strip.props.onClick({ stopPropagation() {}, clientX: 25, currentTarget: { getBoundingClientRect: () => ({ left: 0, width: HOUR_MICROS }) } })
})

test("a row's peak is its first strictly positive maximum, and a silent row has none", () => {
  assert.equal(activity.rowPeakColumn([null, 2, 7, null, 7, 1]), 2)
  assert.equal(activity.rowPeakColumn([0, 0.5, 0.5]), 1)
  assert.equal(activity.rowPeakColumn([null, null]), null)
  assert.equal(activity.rowPeakColumn([0, 0, 0]), null)
  assert.equal(activity.rowPeakColumn([]), null)
})

test("cgroup I/O hoists one shared path and keeps differing paths on their device rows", () => {
  const row = (path, major, minor) => ({ typeId: "1203002", identity: [path, major, minor], labels: {}, members: null, total: 1, cells: [1] })
  const rows = [row("/", "259", "0"), row("/", "252", "0")]
  const view = { cumulative: true, summary: "sum", intervals: [], rows, totals: { cells: [2], total: 2 }, others: { cells: [0], total: 0 }, othersCount: 0, entityCount: 2 }
  assert.equal(activity.cgroupIoSharedPath(view), "/")
  assert.deepEqual(activity.cgroupActivityIdentity(rows[0], true), { text: "259:0", prefix: "/" })
  assert.deepEqual(activity.cgroupActivityIdentity(rows[1], true), { text: "252:0", prefix: "/" })
  assert.deepEqual(activity.cgroupActivityIdentity(rows[0], false), { text: "/", prefix: null })

  const split = { ...view, rows: [rows[0], row("/batch", "252", "0")] }
  assert.equal(activity.cgroupIoSharedPath(split), null)
  assert.deepEqual(activity.cgroupActivityIdentity(split.rows[1], true), { text: "252:0", prefix: "/batch" })
  assert.equal(activity.cgroupIoSharedPath({ ...view, othersCount: 1, entityCount: 3 }), null)

  const mapped = new Map([
    [JSON.stringify(["/", "252:0"]), {
      associations: [], chain: [{ id: "252:0", name: "dm-0" }, { id: "259:4", name: null }, { id: "259:0", name: "nvme0n1" }], device: "dm-0", foldedInto: null, id: "252:0", preferredMounts: ["/var/lib/kronika/data"], source: "/dev/mapper/data-docker",
    }],
    [JSON.stringify(["/", "259:0"]), { associations: [], chain: [{ id: "259:0", name: null }], device: null, foldedInto: "252:0", id: "259:0", preferredMounts: [], source: null }],
  ])
  assert.deepEqual(activity.cgroupActivityIdentity(rows[1], true, mapped), {
    detail: "252:0", text: "/var/lib/kronika/data", title: "/var/lib/kronika/data · data-docker · dm-0 → nvme0n1 · 252:0", prefix: "/",
  })
  assert.deepEqual(activity.cgroupActivityIdentity(split.rows[1], true, mapped), { text: "252:0", prefix: "/batch" })
  // An unnamed device is its bare identity, never prose about the recording.
  assert.deepEqual(activity.cgroupActivityIdentity(rows[0], true, mapped), {
    detail: "259:0", text: "259:0", title: "259:0", prefix: "/",
  })
})

test("a drill moves the cursor only when the drilled row is silent at it", async () => {
  const source = await readFile(new URL("../src/activity.tsx", import.meta.url), "utf8")
  const choose = /const choose = drill === undefined \? undefined : \(row[\s\S]*?\n  \}/.exec(source)?.[0] ?? ""
  // Silent at the cursor (or the cursor outside the hour) -> jump to the
  // row's own peak, by the shared instant. Alive at the cursor -> stay.
  assert.match(choose, /cursorColumn === null \|\| \(row\.cells\[cursorColumn\] \?\? null\) === null/)
  assert.match(choose, /intervalInstant\(view\?\.intervals \?\? \[\], peak\)/)
  assert.match(choose, /rowPeakColumn\(row\.cells\)/)
  // The strip's own click uses the same instant, so the two gestures agree.
  assert.match(source, /intervalInstant\(intervals, column\)/)
})

test("ranked statement and plan previews use the first nonempty loaded table text", async () => {
  const row = (logicalName, ordinal, values) => ({ logicalName, ordinal, segmentId: "s", timestamp: HOUR, typeId: "t", values })
  const statements = activity.statementTextsByQueryId([
    row("pg_stat_statements", "1", { queryid: "101", query: " \n\t" }),
    row("pg_stat_statements", "2", { queryid: "101", query: " select  \n  one " }),
    row("pg_stat_statements", "3", { queryid: "101", query: "ignored later text" }),
    row("pg_stat_statements", "4", { queryid: "102", query: null }),
  ])
  const plans = activity.planTextsByPlanId([
    row("pg_store_plans", "1", { planid: 201, plan: "  Seq Scan\t on orders  " }),
    row("pg_store_plans", "2", { planid: 201, plan: "ignored later plan" }),
  ])
  assert.deepEqual([...statements], [["101", " select  \n  one "]])
  assert.deepEqual([...plans], [["201", "  Seq Scan\t on orders  "]])
  assert.equal(activity.activityPreview(statements.get("101")), "select one")
  assert.equal(activity.activityPreview(plans.get("201")), "Seq Scan on orders")
  assert.equal(activity.activityPreview(`select ${"x".repeat(300)}`).length, 240)

  const [source, view] = await Promise.all([
    readFile(new URL("../src/activity.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
  ])
  assert.match(source, /t\("pg\.detail\.query", \{ id: queryId \?\? "—" \}\)/)
  assert.match(source, /t\("pg\.detail\.plan", \{ id: planId \?\? "—" \}\)/)
  assert.doesNotMatch(source, /labelText\(row, "(?:query|plan)"\)|loadRelatedStatementTextRow|first_match/)
  // The heatmap and the summary always render; the scope travels to the server.
  assert.match(view, /<StatementsActivity[^>]+rows=\{statementRows\} scope=\{statementScope\.scope\}/)
  assert.doesNotMatch(view, /showMonitorQueries && <StatementsActivity|monitorQueriesVisible && <StatementsActivity/)
  assert.match(view, /summary=\{summary\("statements", statementLens\)\}/)
  assert.match(view, /usePostgresSummary\(hour, historyRevision, statementScope\.scope\)/)
  assert.match(view, /<PlansActivity[^>]+rows=\{data\.sections\.pg_store_plans \?\? NO_ROWS\}/)
})

test("Statements scope widens to every statement only for explicit navigation", async () => {
  const [view, app] = await Promise.all([
    readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
  ])
  assert.match(view, /const forced = pattern\.trim\(\) !== ""\s*\|\| context\?\.logicalName === "pg_stat_statements"\s*\|\| exactMonitorQuery\s*\|\| selectedMonitorQuery/)
  assert.match(view, /return \{ scope: show \|\| forced \? "all" : "workload", forced \}/)
  assert.match(view, /forced=\{statementScope\.forced\}/)
  assert.match(view, /count=\{excludedMonitorQueries\}/)
  // The table no longer filters rows on the client: the exact count comes from the page trailer.
  assert.doesNotMatch(view, /transformRows=\{statementTransform\}|statusRowCount=\{monitorQueriesVisible/)
  assert.match(app, /denseRequest\.section === "pg_stat_statements" \? statementScope\.scope : undefined/)
})
