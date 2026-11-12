import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

const source = (name) => readFile(new URL(`../src/${name}`, import.meta.url), "utf8")

test("column resize uses the installed library's mouse and touch handlers", async () => {
  const entityTable = await source("entity-table.tsx")
  assert.match(entityTable, /onMouseDown=\{header\.getResizeHandler\(\)\}/)
  assert.match(entityTable, /onTouchStart=\{header\.getResizeHandler\(\)\}/)
  assert.match(entityTable, /column-grip[^"]*touch-none/)
  assert.doesNotMatch(entityTable, /onMouseDown=\{\(\) =>/)
  assert.match(entityTable, /columnResizeMode: "onChange"/)
})

test("cancelled or unmounted table gestures release the library's document listeners", async () => {
  const entityTable = await source("entity-table.tsx")
  assert.match(entityTable, /addEventListener\("touchcancel", cancelTableTouchResize\)/)
  assert.match(entityTable, /addEventListener\("pointercancel", cancelTableTouchResize\)/)
  assert.match(entityTable, /removeEventListener\("touchcancel", cancelTableTouchResize\)/)
  assert.match(entityTable, /removeEventListener\("pointercancel", cancelTableTouchResize\)/)
  const cleanup = entityTable.match(/useEffect\(\(\) => \{\s*document\.addEventListener\("touchcancel"[\s\S]*?\}, \[\]\)/)?.[0] ?? ""
  assert.match(cleanup, /cancelTableTouchResize\(\)/)
  assert.match(entityTable, /Object\.defineProperty\(end, "touches", \{ value: \[\] \}\)/)
  assert.match(entityTable, /document\.dispatchEvent\(end\)/)
})

test("Inspector pointer resize owns cancellation, pointer capture and unmount cleanup", async () => {
  const inspector = await source("inspector.tsx")
  assert.match(inspector, /const resizeCleanup = useRef<\(\(\) => void\) \| null>\(null\)/)
  assert.match(inspector, /useEffect\(\(\) => \(\) => \{ resizeCleanup\.current\?\.\(\) \}, \[\]\)/)
  assert.match(inspector, /if \(!event\.isPrimary \|\| event\.button !== 0\) return/)
  assert.match(inspector, /handle\.setPointerCapture\(pointerId\)/)
  assert.match(inspector, /addEventListener\("pointercancel", finish\)/)
  assert.match(inspector, /addEventListener\("lostpointercapture", finish\)/)
  assert.match(inspector, /removeEventListener\("pointercancel", finish\)/)
  assert.match(inspector, /removeEventListener\("lostpointercapture", finish\)/)
  assert.match(inspector, /handle\.releasePointerCapture\(pointerId\)/)
  assert.match(inspector, /resizeCleanup\.current = finish/)
  assert.match(inspector, /event\.key === "Home"/)
  assert.match(inspector, /event\.key === "End"/)
})

test("touch resize blockers stay on the handles and the coarse hit area is not clipped", async () => {
  const styles = await source("styles.css")
  assert.match(styles, /\.inspector-splitter \{[^}]*touch-action: none;/s)
  const marker = "@media (hover: none), (pointer: coarse) {"
  const coarse = styles.slice(styles.lastIndexOf(marker))
  assert.match(coarse, /\.inspector-splitter \{ touch-action: none; width: 16px; \}/)
  // A header cell has overflow-hidden: a negative right offset is clipped and
  // makes the grip unhittable for mouse as well as touch.
  assert.match(coarse, /\.entity-head \.column-grip \{ right: 0; touch-action: none; width: 16px; \}/)
  assert.doesNotMatch(coarse, /right: -\d+px/)
  const hitArea = coarse.match(/\.entity-head \.column-grip::after \{[\s\S]*?\}/)?.[0] ?? ""
  assert.match(hitArea, /width: 44px;/)
  assert.match(hitArea, /right: 0;/)
  // The Inspector handle is full-height; an overlay would shadow its tabs.
  assert.doesNotMatch(coarse, /\.inspector-splitter::after/)
})
