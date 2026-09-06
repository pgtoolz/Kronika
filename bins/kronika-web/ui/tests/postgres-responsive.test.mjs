import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

const directory = dirname(fileURLToPath(import.meta.url))
const stylesheet = await readFile(join(directory, "../src/styles.css"), "utf8")
const app = await readFile(join(directory, "../src/app.tsx"), "utf8")
const entityTable = await readFile(join(directory, "../src/entity-table.tsx"), "utf8")
const postgres = await readFile(join(directory, "../src/postgres-view.tsx"), "utf8")
const chart = await readFile(join(directory, "../src/uplot-chart.tsx"), "utf8")

test("PostgreSQL uses the one shared Inspector beside the table above 1000 pixels", () => {
  assert.match(postgres, /className={`pg-entity-layout[^`]*grid-cols-\[minmax\(0,1fr\)\][^`]*pg-entity-fill/)
  assert.match(postgres, /className={`pg-entity-main min-w-0[^`]*pg-stretch/)
  assert.match(postgres, /<InspectorPortal[\s\S]{0,360}<PgDetail/)
  assert.match(stylesheet, /\.inspector \{[\s\S]*?flex: 0 0 var\(--inspector-width\)/)
  assert.match(stylesheet, /@media \(max-width: 1000px\) \{[\s\S]*?\.inspector \{[\s\S]*?position: fixed;/)
  assert.doesNotMatch(stylesheet, /@utility pg-detail \{[^}]*position: fixed;/)
  assert.doesNotMatch(app, /max-\[1179px\]:fixed/)
})

test("the operator bar wraps before its controls can widen a 1024 pixel page", () => {
  assert.match(app, /className="topbar/)
  assert.match(stylesheet, /@utility topbar \{[\s\S]*?@media \(max-width: 1000px\) \{[\s\S]*?display: grid;/)
  assert.match(stylesheet, /overflow: hidden;/)
})

test("only long PostgreSQL table views own the viewport flex chain", () => {
  assert.match(app, /visibleSource === "postgresql" && pgSection !== "overview"/)
  assert.match(app, /flex h-dvh min-h-0 flex-col overflow-hidden/)
  assert.match(app, /pg-table-shell/)
  assert.match(app, /pg-table-workspace flex flex-col overflow-hidden/)
  assert.match(app, /\[\.pg-table-shell>&\]:flex-none/)
  // Sparse siblings stack by content. A long result alone receives the
  // remaining row and hands that height through to its scroll port.
  assert.match(stylesheet, /@utility pg-stretch \{[\s\S]*?:is\(\.pg-table-shell\) & \{[^}]*flex: 1 1 0;[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(stylesheet, /\.pg-table-shell \.pg-entity-fill \{[^}]*flex: 1 1 0;[^}]*grid-template-rows: minmax\(0, 1fr\);[^}]*min-height: 0;[^}]*overflow: hidden;/s)
  assert.match(postgres, /filterTableRows\(rows, visibleColumns, pattern \?\? "", dense, section\)/)
  assert.match(postgres, /const contentSized = displayedRows\.length < 10 && !canLoadMore/)
  assert.match(postgres, /contentSized \? "" : " pg-entity-fill"/)
  assert.match(postgres, /data-content-sized=\{contentSized \|\| undefined\}/)
  assert.doesNotMatch(postgres, /className="pg-entity-layout[^\n]*pg-table-shell_&/)
  assert.match(entityTable, /contentSized \? "" : " pg-stretch"/)
  assert.match(entityTable, /contentSized \? " !min-h-0 box-content overflow-x-auto overflow-y-hidden" : " overflow-auto"/)
  assert.match(entityTable, /data-scroll-axis=\{contentSized \? "horizontal" : "both"\}/)
  assert.match(entityTable, /if \(contentSized && parent\.current !== null\) parent\.current\.scrollTop = 0/)
  assert.match(entityTable, /if \(!contentSized && locatedIndex >= 0\) virtual\.scrollToIndex/)
  assert.match(entityTable, /const contentWidth = width \+ TABLE_END_GUTTER/)
  assert.match(entityTable, /const virtualHeight = virtual\.getTotalSize\(\)/)
  assert.match(entityTable, /translateY\(\$\{item\.start\}px\)/)
  assert.match(entityTable, /paddingRight: TABLE_END_GUTTER/)
  assert.match(entityTable, /const TABLE_END_GUTTER = 8/)
  assert.match(stylesheet, /\.inspector-body \{[^}]*overflow-x: hidden;[^}]*overflow-y: auto;[^}]*scrollbar-gutter: stable;/s)
  assert.match(stylesheet, /\.inspector-detail-slot > \.pg-detail \{[^}]*overflow: visible;/s)
})

test("short PostgreSQL workspaces keep the honest compact preview", () => {
  assert.match(app, /\[&>\.timeline-shell\]:flex-none/)
  assert.match(stylesheet, /\.timeline-preview \{[^}]*height: 124px;/s)
  assert.match(chart, /variant === "preview" \? "h-\[94px\]/)
  assert.doesNotMatch(stylesheet, /\.timeline-preview[\s\S]{0,240}\.uplot-host[^}]*display: none/)
  assert.match(postgres, /const contentSized = displayedRows\.length < 10 && !canLoadMore/)
  assert.match(entityTable, /const \[headHeight, setHeadHeight\] = useState\(28\)/)
  assert.match(entityTable, /head\.current\?\.getBoundingClientRect\(\)\.height/)
  assert.match(entityTable, /const \[horizontalRailHeight, setHorizontalRailHeight\] = useState\(0\)/)
  assert.match(entityTable, /root\.offsetHeight - root\.clientHeight/)
  assert.match(entityTable, /new ResizeObserver\(measureRail\)/)
  assert.match(entityTable, /contentSized \? \(rendered\.length === 0 \? 72 : Math\.min\(310, headHeight \+ rendered\.length \* rowHeight\)\) \+ horizontalRailHeight/)
})

test("the shared Chart Inspector uses one compact metric selector and one body scroll axis", async () => {
  const timeline = await readFile(join(directory, "../src/timeline.tsx"), "utf8")
  assert.match(app, /presentation="inspector"/)
  assert.match(timeline, /data-testid="timeline-metric-select"/)
  assert.match(timeline, /presentation === "inspector"[\s\S]{0,420}<select/)
  assert.match(stylesheet, /\.timeline-inspector \.timeline-rail \{ height: 34px; min-height: 34px; \}/)
  assert.match(stylesheet, /\.timeline-metric-picker > select \{[^}]*min-width: 0;[^}]*width: 100%;/s)
  assert.match(stylesheet, /\.inspector-head \{[^}]*flex: 0 0 auto;[^}]*min-height: 34px;/s)
  assert.match(stylesheet, /\.inspector-head > strong \{[^}]*overflow-wrap: anywhere;[^}]*white-space: normal;/s)
})
