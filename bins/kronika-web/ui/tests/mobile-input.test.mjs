import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { gunzipSync } from "node:zlib"

const source = (name) => readFile(new URL(`../src/${name}`, import.meta.url), "utf8")

test("HH:MM:SS uses an ordinary text keyboard and keeps the strict parser", async () => {
  const [dialog, parser, shell, en, ru] = await Promise.all([
    source("export-dialog.tsx"),
    source("export-time.ts"),
    source("index.html"),
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  assert.match(dialog, /inputMode="text"/)
  assert.doesNotMatch(dialog, /inputMode="(?:numeric|tel|decimal)"/)
  assert.match(dialog, /spellCheck=\{false\}/)
  assert.ok(
    parser.includes('const TIME = /^(\\d{2}):(\\d{2}):(\\d{2})$/'),
    "the strict HH:MM:SS parser must remain unchanged",
  )
  assert.doesNotMatch(shell, /user-scalable\s*=\s*no|maximum-scale/)
  for (const dictionary of [en, ru]) {
    assert.match(dictionary, /^export\.clock:/m)
    assert.match(dictionary, /^filter\.label:/m)
  }
})

test("mobile editable fields use 16px without changing compact desktop density", async () => {
  const styles = await source("styles.css")
  assert.match(styles, /--text-xs: 11px;/)
  assert.match(styles, /--text-sm: 12px;/)
  assert.match(styles, /--text-md: 13px;/)
  assert.match(
    styles,
    /@media \(max-width: 520px\), \(hover: none\), \(pointer: coarse\) \{\s*input\.export-clock,\s*\[data-search-surface\] input\[type="search"\],\s*input\[type="text"\],\s*input\[type="password"\] \{ font-size: 16px; \}\s*\}/,
  )
  assert.match(styles, /\.mobile-search-open \[data-search-surface\] input \{ font-size: 16px; padding-block: 9px; \}/)
})

test("both production artifacts retain the mobile input contract", async () => {
  const [liveBytes, reportBytes] = await Promise.all([
    readFile(new URL("../kronika-ui.html.gz", import.meta.url)),
    readFile(new URL("../../../../crates/kronika-report/assets/kronika-report-shell.html.gz", import.meta.url)),
  ])
  const live = gunzipSync(liveBytes).toString("utf8")
  const report = gunzipSync(reportBytes).toString("utf8")
  const mobileRule = /@media \(max-width:520px\),\(hover:none\),\(pointer:coarse\)\{input\.export-clock,\[data-search-surface\] input\[type=search\],input\[type=text\],input\[type=password\]\{font-size:16px\}\}/
  const searchRule = /\.mobile-search-open \[data-search-surface\] input\{padding-block:9px;font-size:16px\}/

  assert.match(live, /inputMode:"text"/)
  assert.doesNotMatch(live, /inputMode:"numeric"/)
  assert.doesNotMatch(report, /inputMode:"numeric"/)
  for (const [name, html] of [["live", live], ["report", report]]) {
    assert.match(html, mobileRule, `${name} artifact is missing the 16px mobile input rule`)
    assert.match(html, searchRule, `${name} artifact is missing the 16px mobile search rule`)
    assert.doesNotMatch(html, /user-scalable=no|maximum-scale=/, `${name} artifact must not disable pinch zoom`)
  }
})
