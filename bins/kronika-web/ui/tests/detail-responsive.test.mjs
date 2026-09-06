import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { gunzipSync } from "node:zlib"


const stylesheet = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")

function blockAfter(marker, source = stylesheet) {
  const start = source.indexOf(marker)
  assert.notEqual(start, -1, `missing ${marker}`)
  const opening = source.indexOf("{", start)
  let depth = 0
  for (let index = opening; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1
    if (source[index] === "}") depth -= 1
    if (depth === 0) return source.slice(opening + 1, index)
  }
  assert.fail(`unterminated ${marker}`)
}

test("all detail key/value rows share a readable label track and bounded value track", async () => {
  // the shared row pattern lives in @utility detail-row/detail-dt/detail-dd and
  // every dock detail surface uses it. The events console is a full-width
  // surface, where the dock pattern strands the value at the far edge, so it
  // carries facts as chips instead.
  assert.match(stylesheet, /@utility detail-row \{[^}]*minmax\(0, min\(10rem, 40%\)\) minmax\(0, 1fr\)/s)
  assert.match(stylesheet, /@utility detail-dd \{[^}]*overflow-wrap:\s*anywhere;[^}]*text-align:\s*right;/s)
  assert.match(stylesheet, /@utility detail-dd \{[^}]*@media \(max-width: 520px\) \{ text-align: left; \}/s)
  const composition = await readFile(new URL("../src/detail-list.tsx", import.meta.url), "utf8")
  assert.match(composition, /detail-row max-\[520px\]:detail-row-stacked/)
  assert.match(composition, /valueRole === "machine" \? "detail-dd detail-dd-machine" : "detail-dd"/)
  for (const view of ["detail.tsx", "detail-activity.tsx", "detail-plans.tsx", "detail-process.tsx", "postgres-view.tsx", "postgres-relations-view.tsx", "system-view.tsx"]) {
    const source = await readFile(new URL(`../src/${view}`, import.meta.url), "utf8")
    assert.match(source, /DetailList/, view)
    assert.match(source, /DetailRow/, view)
  }
  const console_ = await readFile(new URL("../src/events-console.tsx", import.meta.url), "utf8")
  assert.doesNotMatch(console_, /DetailList|DetailRow/)
  assert.match(console_, /data-testid="event-entry-facts"/)
  const relation = await readFile(new URL("../src/postgres-relations-view.tsx", import.meta.url), "utf8")
  assert.doesNotMatch(relation, /<dl>|<dt>|<dd>/)
})

test("live and report artifacts retain the shared Detail value style", async () => {
  const [live, report] = await Promise.all([
    readFile(new URL("../kronika-ui.html.gz", import.meta.url)),
    readFile(new URL("../../../kronika-report/assets/kronika-report-shell.html.gz", import.meta.url)),
  ])
  for (const [name, artifact] of [["live", live], ["report", report]]) {
    assert.match(gunzipSync(artifact).toString("utf8"), /\.detail-dd\{/s, name)
  }
})

test("narrow detail rows stack labels above values", () => {
  assert.match(stylesheet, /@utility detail-row-stacked \{[^}]*grid-template-columns:\s*minmax\(0, 1fr\)/s)
})

test("detail typography keeps semantic values proportional and opts machine strings into mono", async () => {
  const label = blockAfter("@utility detail-dt")
  const value = blockAfter("@utility detail-dd")
  const machine = blockAfter("@utility detail-dd-machine")
  assert.match(label, /font-family:\s*var\(--font-sans\)/)
  assert.match(label, /font-size:\s*var\(--text-sm\)/)
  assert.match(value, /font-family:\s*var\(--font-sans\)/)
  assert.match(value, /font-size:\s*var\(--text-md\)/)
  assert.match(value, /font-variant-numeric:\s*tabular-nums/)
  assert.match(machine, /font-family:\s*var\(--font-mono\)/)
  assert.match(machine, /font-size:\s*var\(--text-sm\)/)

  const [composition, entity, process, activity, postgres, system] = await Promise.all([
    readFile(new URL("../src/detail-list.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/detail-activity.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
  ])
  assert.match(composition, /DetailValueRole = "semantic" \| "machine"/)
  assert.match(composition, /valueRole = "semantic"/)
  assert.match(composition, /data-value-role={valueRole}/)
  assert.doesNotMatch(composition, /valueClassName/)
  assert.match(entity, /column\.kind === "id" \|\| column\.kind === "timestamp" \? "machine" : "semantic"/)
  assert.match(process, /"tree_command", "state", "timestamp"/)
  for (const field of ["backend_type", "state", "wait_event_type", "wait_event"]) {
    assert.match(activity, new RegExp(`"${field}", "pg\\.[^"]+", "text", "machine"`), field)
  }
  assert.match(postgres, /extra\("datname", "text"\), detailValueRole: "machine"/)
  assert.match(postgres, /extra\("phase", "text"\), detailValueRole: "machine"/)
  assert.match(system, /machineText\("cgroup_path", 240, true\)/)
  assert.match(system, /machineText\("fstype", 120\)/)
})

test("Process table and dock inherit one remaining viewport row", async () => {
  const [app, detail, table, entity] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8"),
  ])
  assert.match(app, /process-main grid min-h-0[^\n]*flex-1[^\n]*grid-rows-\[minmax\(0,1fr\)\]/)
  assert.match(table, /process-table flex min-h-0[^\n]*flex-col/)
  assert.match(entity, /\[\.process-table_&\]:h-auto \[\.process-table_&\]:min-h-0 \[\.process-table_&\]:flex-1/)
  assert.match(detail, /process-detail-dock min-h-0 bg-s2 p-3/)
  assert.doesNotMatch(entity, /\[\.process-table_&\]:h-\[min\(570px,calc\(100vh-/)
  assert.doesNotMatch(detail, /max-h-\[min\(570px,calc\(100vh-/)
})

test("shared shells join the timeline directly to real content", async () => {
  const [app, host, postgres] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/postgres-view.tsx", import.meta.url), "utf8"),
  ])
  assert.match(app, /className="lensbar !mt-0 border-t-0"/)
  assert.match(host, /className="system-main mt-0 min-w-0"/)
  assert.match(postgres, /className="pg-tabs !mt-0/)
})
