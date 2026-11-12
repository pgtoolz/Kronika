import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { gunzipSync } from "node:zlib"

const source = (name) => readFile(new URL(`../src/${name}`, import.meta.url), "utf8")
const artifact = async (name) => gunzipSync(await readFile(new URL(name, import.meta.url))).toString("utf8")

function bodyBetween(text, start, end) {
  const from = text.indexOf(start)
  const to = text.indexOf(end, from + start.length)
  assert.ok(from >= 0 && to > from, `missing ${start} … ${end}`)
  return text.slice(from, to)
}

test("the handoff keeps the blob URL alive and never guesses a timeout", async () => {
  const download = await source("export-download.ts")
  const handoff = bodyBetween(download, "export function handoffHtmlDownload", "export function releaseHtmlDownload")
  assert.doesNotMatch(handoff, /revokeObjectURL/, "revoking in the same tick cancels WebKit saves")
  assert.match(handoff, /link\.click\(\)/)
  assert.match(handoff, /download\.url = objectUrl/)
  assert.doesNotMatch(download, /setTimeout|setInterval/, "URL lifetime must be tied to a verified lifecycle event")
  assert.match(download, /export function releaseHtmlDownload/)
  assert.match(download, /download\.url = null/, "release must be idempotent")
  assert.doesNotMatch(download, /triggerHtmlDownload/)
})

test("the export dialog starts the save from a fresh gesture, not after await", async () => {
  const [dialog, styles] = await Promise.all([source("export-dialog.tsx"), source("styles.css")])
  const submit = bodyBetween(dialog, "const submit = async", "const handoff =")
  assert.doesNotMatch(submit, /handoffHtmlDownload|link\.click/, "a long fetch has consumed transient user activation")
  assert.match(submit, /htmlDownload\(artifact\.blob, artifact\.filename\)/)
  assert.match(submit, /phase: "ready"/)
  assert.match(dialog, /phase: "handed"/)
  assert.match(dialog, /data-testid="export-download"/)
  assert.match(dialog, /data-testid="export-again"/)
  assert.doesNotMatch(dialog, /export\.saved|phase: "saved"/, "link.click() is not a confirmed disk write")
  assert.doesNotMatch(styles, /data-phase="saved"/)
  assert.match(styles, /data-phase="ready"/)
  assert.doesNotMatch(dialog, /userActivation/, "Safari does not expose a reliable activation flag")
  assert.match(dialog, /window\.addEventListener\("pagehide"/)
})

test("every exit path releases the owned blob URL", async () => {
  const dialog = await source("export-dialog.tsx")
  const close = bodyBetween(dialog, "const close = useCallback", "useEffect(() => {\n    const escape")
  assert.match(close, /release\(\)/)
  const submit = bodyBetween(dialog, "const submit = async", "const handoff =")
  assert.match(submit, /release\(\)/, "a new export discards the previous report")
  assert.match(submit, /catch \(reason\) \{\s*release\(\)/, "a transfer error must not leak the URL")
  assert.match(dialog, /onClick=\{\(\) => \{ release\(\); setJob\(\{ phase: "idle" \}\)/)
  const handoff = bodyBetween(dialog, "const handoff =", "const elapsed =")
  assert.match(handoff, /catch \{\s*releaseHtmlDownload\(prepared\)/, "a refused handoff releases the URL but preserves the Blob for retry")
  assert.doesNotMatch(handoff, /download\.current = null|release\(\)/)
  assert.match(dialog, /const suspend = \(\) => releaseHtmlDownload\(download\.current\)/)
  assert.match(dialog, /return \(\) => \{\s*window\.removeEventListener\("pagehide", suspend\)\s*release\(\)/)
})

test("RU/EN strings describe a handoff, not a confirmed save", async () => {
  const [en, ru] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  for (const dictionary of [en, ru]) {
    for (const key of ["export.ready", "export.handed", "export.download", "export.download_again", "export.error.handoff"]) {
      assert.match(dictionary, new RegExp(`^${key.replaceAll(".", "\\.")}:`, "m"), `${key} is missing`)
    }
    assert.doesNotMatch(dictionary, /^export\.saved:/m)
    assert.match(dictionary, /^export\.ready:.*\{name\}.*\{size\}.*\{seconds\}/m)
    assert.match(dictionary, /^export\.handed:.*\{name\}.*\{size\}/m)
  }
  assert.match(en, /check Downloads or Files/)
  assert.match(ru, /«Загрузки» или «Файлы»/)
  assert.match(en, /Download prepared report/)
  assert.match(ru, /Скачать готовый отчёт/)
  assert.doesNotMatch(en, /^export\.(ready|handed):.*\bsaved:/m)
})

test("both production artifacts carry the server-only handoff strings", async () => {
  const [server, report] = await Promise.all([
    artifact("../kronika-ui.html.gz"),
    artifact("../../../../crates/kronika-report/assets/kronika-report-shell.html.gz"),
  ])
  for (const needle of ["Download prepared report", "Скачать готовый отчёт", "check Downloads or Files", "handed to the browser"]) {
    assert.ok(server.includes(needle), `server artifact is missing ${needle}`)
  }
  assert.ok(!server.includes("saved: {name}"), "the server artifact must not claim a confirmed save")
  assert.ok(!report.includes("Download prepared report"), "report UI must stay free of export-only strings")
  assert.ok(!report.includes("export-download"), "report UI must not ship the export button")
  for (const [name, html] of [["server", server], ["report", report]]) {
    assert.equal(scriptBodies(html).length, 2, `${name} artifact must keep exactly two inline scripts`)
  }
})

function scriptBodies(html) {
  const bodies = []
  let tail = html
  for (;;) {
    const start = tail.indexOf("<script>")
    if (start < 0) return bodies
    const body = tail.slice(start + "<script>".length)
    const end = body.toLowerCase().indexOf("</script>")
    assert.ok(end >= 0, "incomplete inline script")
    bodies.push(body.slice(0, end))
    tail = body.slice(end + "</script>".length)
  }
}
