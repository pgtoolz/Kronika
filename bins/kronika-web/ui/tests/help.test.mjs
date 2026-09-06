import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const help = await importModule('export { placeTooltip } from "../src/help.tsx"')

test("tooltip placement stays in the viewport and flips above a low anchor", () => {
  const size = { height: 80, width: 200 }
  assert.deepEqual(
    help.placeTooltip({ bottom: 114, height: 14, left: 300, top: 100, width: 14 }, size, { height: 600, width: 800 }),
    { left: 207, placement: "below", top: 120 },
  )
  assert.deepEqual(
    help.placeTooltip({ bottom: 584, height: 14, left: 786, top: 570, width: 14 }, size, { height: 600, width: 800 }),
    { left: 592, placement: "above", top: 484 },
  )
  assert.deepEqual(
    help.placeTooltip({ bottom: 18, height: 10, left: 2, top: 8, width: 10 }, { height: 180, width: 240 }, { height: 120, width: 160 }),
    { left: 8, placement: "below", top: 8 },
  )
})

test("field help uses a fixed top-level portal above every workspace layer", async () => {
  const [source, styles] = await Promise.all([
    readFile(new URL("../src/help.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  assert.match(source, /createPortal\(/)
  assert.match(source, /document\.body/)
  assert.match(source, /position.*placeTooltip|placeTooltip\(/s)
  assert.match(source, /document\.addEventListener\("pointerdown", outside, true\)/)
  assert.match(source, /window\.addEventListener\("scroll", update, true\)/)
  assert.match(source, /event\.key !== "Escape"/)
  // The tooltip is portaled and placed by script: it stays fixed, above every
  // in-page layer, and narrower than the viewport it is measured against.
  const tooltip = source.match(/<span\s+className="[^"]*"\s+data-placement/s)?.[0] ?? ""
  assert.match(tooltip, /\bfixed\b/)
  assert.match(tooltip, /z-\[1000\]/)
  assert.match(tooltip, /max-w-\[min\(310px,70vw,calc\(100vw_-_16px\)\)\]/)
  assert.match(source, /\[\.entity-header-cell>&\]:opacity-0/)
  assert.match(source, /\[\.entity-header-cell:focus-within>&\]:opacity-100/)
})

test("a USE cell is one action while methodology help stays in the column header", async () => {
  const [source, en, ru] = await Promise.all([
    readFile(new URL("../src/use-table.tsx", import.meta.url), "utf8"),
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  // The complete populated cell is the only cell action. Lane text is plain
  // content, so a help button cannot overlap or nest inside that action.
  assert.doesNotMatch(source, /iconOnly/)
  assert.match(source, /data-testid=\{`use-cell-\$\{resource\.key\}-\$\{column\}`\}/)
  assert.match(source, /<span>\{laneLabels\.join\(" · "\)\}<\/span>/)
  assert.doesNotMatch(source, /useLaneHelp/)
  // What the three columns mean is said once, in their names.
  for (const column of ["utilisation", "saturation", "errors"]) {
    assert.match(source, new RegExp(`use\\.\\$\\{column\\}\\.help`))
    for (const dictionary of [en, ru]) {
      assert.match(dictionary, new RegExp(`^use\\.${column}\\.help: ".+"$`, "m"))
    }
  }
})
