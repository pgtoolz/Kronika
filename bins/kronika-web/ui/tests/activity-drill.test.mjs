import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const activity = await importModule(
  'export { cgroupActivityIdentity, cgroupIoSharedPath, cursorColumnOf, intervalInstant, planTextsByPlanId, rowPeakColumn, statementTextsByQueryId } from "../src/activity.tsx"; export { activityPreview } from "../src/activity-cuts.ts"',
  { plugins: [registryPlugin([])] },
)

const HOUR = 1_000_000_000_000
const HOUR_MICROS = 3_600_000_000

test("a cell click and the drill land on the identical microsecond of a column", () => {
  // The last moment of the column, exactly as ActivityStrip.pick computes it.
  assert.equal(activity.intervalInstant(HOUR, 0, 12), HOUR + 300_000_000 - 1)
  assert.equal(activity.intervalInstant(HOUR, 11, 12), HOUR + HOUR_MICROS - 1)
  assert.equal(activity.intervalInstant(HOUR, 59, 60), HOUR + HOUR_MICROS - 1)
})

test("the cursor's column mirrors the strip and is null outside the hour", () => {
  assert.equal(activity.cursorColumnOf(HOUR, HOUR, 12), 0)
  assert.equal(activity.cursorColumnOf(HOUR + 300_000_000, HOUR, 12), 1)
  assert.equal(activity.cursorColumnOf(HOUR + HOUR_MICROS - 1, HOUR, 12), 11)
  assert.equal(activity.cursorColumnOf(HOUR - 1, HOUR, 12), null)
  assert.equal(activity.cursorColumnOf(HOUR + HOUR_MICROS, HOUR, 12), null)
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
  assert.match(choose, /onCursor\(intervalInstant\(hour, peak, columns\)\)/)
  assert.match(choose, /rowPeakColumn\(row\.cells\)/)
  // The strip's own click uses the same instant, so the two gestures agree.
  assert.match(source, /onCursor\(intervalInstant\(hour, column, columns\)\)/)
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
