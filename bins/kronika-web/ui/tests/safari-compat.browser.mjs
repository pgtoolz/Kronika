import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { createServer } from "node:http"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"
import { gunzipSync } from "node:zlib"
import { chromium, webkit } from "playwright"
import { AT, answerFixture } from "./compat-fixture.mjs"

const repo = fileURLToPath(new URL("../../../../", import.meta.url))
const results = fileURLToPath(new URL("../.browser-results/", import.meta.url))
const id = (value) => `[data-testid="${value}"]`
const configurations = [
  ["desktop", { width: 1440, height: 1000 }],
  ["phone", { width: 390, height: 844 }],
  ["narrow", { width: 320, height: 700 }],
  ["tablet", { width: 1180, height: 820 }],
]
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex")

// Use the actual generator, not a shell with substituted test bootstrap code.
async function generateReport(directory) {
  const toolchain = process.env.RUST_TOOLCHAIN ?? "1.96.0"
  const host = /^host: (.+)$/m.exec(execFileSync("rustc", [`+${toolchain}`, "-vV"], { encoding: "utf8" }))[1]
  const output = join(directory, "standalone.html")
  execFileSync("cargo", [`+${toolchain}`, "run", "--locked", "--quiet", "--target", host, "-p", "kronika-report-cli", "--",
    join(repo, "crates/kronika-report/tests/fixtures/standalone.zms"), output], { cwd: repo, timeout: 300_000, stdio: ["ignore", "ignore", "inherit"] })
  const bytes = await readFile(output)
  assert.ok(!bytes.includes("/*KRONIKA_REPORT_RUNTIME*/"), "runtime marker must be replaced by the generator")
  return { output, bytes }
}

async function focusInside(page, root) {
  assert.equal(await page.locator(root).evaluate((node) => node.contains(document.activeElement)), true, "focus escaped the surface")
}

async function downloaded(page, click, expected, filename) {
  const waiting = page.waitForEvent("download")
  await click()
  const download = await waiting
  assert.equal(await download.failure(), null)
  assert.equal(download.suggestedFilename(), filename)
  const bytes = await readFile(await download.path())
  assert.equal(hash(bytes), hash(expected), "the saved bytes differ from the prepared report")
}

for (const [engine, type] of [["chromium", chromium], ["webkit", webkit]]) {
  test(`${engine}: production Safari R1 regression`, { timeout: 600_000 }, async (t) => {
    await mkdir(results, { recursive: true })
    const directory = await mkdtemp(join(tmpdir(), "kronika-compat-"))
    t.after(() => rm(directory, { recursive: true, force: true }))
    const artifact = gunzipSync(await readFile(new URL("../kronika-ui.html.gz", import.meta.url)))
    const report = await generateReport(directory)
    const browser = await type.launch({ headless: true, ...(engine === "chromium" && process.env.KRONIKA_CHROMIUM_CHANNEL ? { channel: process.env.KRONIKA_CHROMIUM_CHANNEL } : {}) })
    console.log(JSON.stringify({ engine, version: browser.version(), artifact: hash(artifact), report: hash(report.bytes), reportBytes: report.bytes.length }))
    let exportMode = "ok"
    let exports = 0
    let downloadCount = 0
    const server = createServer((request, response) => answerFixture(request, response, artifact, (res) => {
      exports += 1
      const reply = () => {
        res.writeHead(exportMode === "unauthorized" ? 401 : 200, {
          "Content-Type": "text/html", "Content-Length": String(report.bytes.length),
          "Content-Disposition": 'attachment; filename="kronika-test.html"; filename*=UTF-8\'\'kronika-%D1%82%D0%B5%D1%81%D1%82.html',
        })
        res.end(report.bytes)
      }
      // Deliberately outlive transient activation; tests wait on UI state, not a sleep.
      if (exportMode === "slow") setTimeout(reply, 6500)
      else reply()
    }))
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
    const origin = `http://127.0.0.1:${server.address().port}`
    try {
      for (const [device, viewport] of configurations) {
        await t.test(device, async () => {
          await rm(join(results, `${engine}-${device}.png`), { force: true })
          const context = await browser.newContext({ viewport, hasTouch: device !== "desktop", isMobile: device !== "desktop", locale: "en-US", timezoneId: "UTC", acceptDownloads: true })
          await context.tracing.start({ screenshots: true, snapshots: true })
          const page = await context.newPage()
          page.setDefaultTimeout(10_000)
          const errors = []
          const unexpected = []
          page.on("pageerror", (error) => errors.push(error.message))
          page.on("response", (response) => { if (response.status() >= 400) unexpected.push(`${response.status()} ${response.url()}`) })
          page.on("download", () => { downloadCount += 1 })
          const click = (locator) => device === "desktop" ? locator.click() : locator.tap()
          const boot = async () => {
            await page.goto(`${origin}/?at=${AT}&view=pg.activity`)
            await page.locator(id("hour-picker-trigger")).waitFor()
            await page.locator(".entity-row").first().waitFor()
          }
          try {
            await boot()
            const hourBefore = await page.locator(id("hour-picker-trigger")).getAttribute("aria-label")
            await click(page.locator(id("hour-picker-trigger")))
            await click(page.locator('[data-testid="day-cell"]:not([disabled]):not([aria-pressed="true"])').first())
            assert.equal(await page.locator(id("hour-popover")).count(), 1)
            await click(page.locator(id("hour-cell")).first())
            await page.locator(id("hour-popover")).waitFor({ state: "detached" })
            await page.waitForFunction((before) => document.querySelector('[data-testid="hour-picker-trigger"]').getAttribute("aria-label") !== before, hourBefore)
            await boot()
            await click(page.locator(id("hour-picker-trigger")))
            await page.keyboard.press("Escape")
            await page.locator(id("hour-popover")).waitFor({ state: "detached" })
            assert.equal(await page.locator(id("hour-picker-trigger")).evaluate((node) => node === document.activeElement), true)
            await click(page.locator(id("hour-picker-trigger")))
            for (let n = 0; n < 50 && await page.locator(id("hour-popover")).count(); n++) await page.keyboard.press("Tab")
            await page.locator(id("hour-popover")).waitFor({ state: "detached" })

            await click(page.locator(id("export-trigger")))
            await page.locator(id("export-dialog")).waitFor()
            for (const key of ["Tab", "Shift+Tab"]) for (let n = 0; n < 15; n++) {
              await page.keyboard.press(key)
              await focusInside(page, id("export-dialog"))
            }
            const input = page.locator(id("export-from"))
            assert.equal(await input.getAttribute("inputmode"), "text")
            if (device !== "desktop") assert.ok(await input.evaluate((node) => parseFloat(getComputedStyle(node).fontSize)) >= 16)
            await input.fill("05:01:02")
            assert.equal(await input.getAttribute("aria-invalid"), null)
            await click(page.locator(id("export-from-day")))
            await click(page.locator('[data-testid="export-day"]:not([disabled])').last())
            await page.keyboard.press("Escape")
            await page.locator(id("export-dialog")).waitFor({ state: "detached" })
            assert.equal(await page.locator(id("export-trigger")).evaluate((node) => node === document.activeElement), true)

            const grip = page.locator(".column-grip").first()
            const before = await grip.evaluate((node) => node.parentElement.getBoundingClientRect().width)
            await grip.evaluate((node) => {
              const r = node.getBoundingClientRect()
              window.compatTouch = (type, delta) => {
                const touch = { identifier: 1, target: node, clientX: r.x + delta, clientY: r.y + 5 }
                const event = new Event(type, { bubbles: true, cancelable: true })
                Object.defineProperty(event, "touches", { value: type === "touchstart" || type === "touchmove" ? [touch] : [] })
                ;(type === "touchstart" ? node : document).dispatchEvent(event)
              }
              window.compatTouch("touchstart", 0)
            })
            for (const delta of [15, 40, 60]) {
              await page.evaluate((x) => window.compatTouch("touchmove", x), delta)
              await page.evaluate(() => new Promise(requestAnimationFrame))
            }
            await page.waitForFunction(({ selector, before }) => document.querySelector(selector).parentElement.getBoundingClientRect().width > before + 30, { selector: ".column-grip", before })
            await page.evaluate(() => { window.compatTouch("touchend", 60); delete window.compatTouch })

            await click(page.locator(id("charts-toggle")))
            await page.locator(id("inspector")).waitFor()
            if (viewport.width <= 760) {
              for (let n = 0; n < 12; n++) {
                await page.keyboard.press(n % 2 ? "Tab" : "Shift+Tab")
                await focusInside(page, id("inspector"))
              }
            } else {
              const splitter = page.locator(".inspector-splitter")
              const width = Number(await splitter.getAttribute("aria-valuenow"))
              await splitter.dispatchEvent("pointerdown", { pointerId: 7, isPrimary: true, pointerType: "touch", button: 0, clientX: 1000 })
              await page.dispatchEvent("body", "pointermove", { pointerId: 7, clientX: 940 })
              await page.waitForFunction((width) => Number(document.querySelector(".inspector-splitter").getAttribute("aria-valuenow")) > width, width)
              await page.dispatchEvent("body", "pointercancel", { pointerId: 7 })
              const stopped = await splitter.getAttribute("aria-valuenow")
              await page.dispatchEvent("body", "pointermove", { pointerId: 7, clientX: 900 })
              assert.equal(await splitter.getAttribute("aria-valuenow"), stopped)
            }
            await page.keyboard.press("Escape")
            await page.locator(id("inspector")).waitFor({ state: "detached" })
            await click(page.locator(id("mcp-trigger")))
            await page.locator(id("mcp-panel")).waitFor()
            await page.keyboard.press("Escape")
            await page.locator(id("mcp-panel")).waitFor({ state: "detached" })
            assert.equal(await page.locator(id("mcp-trigger")).evaluate((node) => node === document.activeElement), true)

            // Exercise the real React export path, including retry and lifecycle cleanup.
            await page.evaluate(() => {
              window.compatUrls = { created: [], revoked: [] }
              const create = URL.createObjectURL.bind(URL), revoke = URL.revokeObjectURL.bind(URL)
              URL.createObjectURL = (blob) => { const url = create(blob); window.compatUrls.created.push(url); return url }
              URL.revokeObjectURL = (url) => { window.compatUrls.revoked.push(url); revoke(url) }
            })
            exportMode = device === "desktop" ? "slow" : "ok"
            const count = downloadCount
            await click(page.locator(id("export-trigger")))
            await click(page.locator(id("export-submit")))
            await page.locator('[data-testid="export-dialog"][data-phase="ready"]').waitFor({ timeout: 20_000 })
            assert.equal(downloadCount, count, "preparation must not auto-start a download")
            assert.equal(await page.evaluate(() => window.compatUrls.created.length), 0)
            await page.evaluate(() => {
              const original = HTMLAnchorElement.prototype.click
              HTMLAnchorElement.prototype.click = function () { HTMLAnchorElement.prototype.click = original; throw new Error("test refused handoff") }
            })
            await click(page.locator(id("export-download")))
            assert.match(await page.locator(id("export-status")).textContent(), /browser refused the download/i)
            const filename = "kronika-тест.html"
            await downloaded(page, () => click(page.locator(id("export-download"))), report.bytes, filename)
            assert.match(await page.locator(id("export-status")).textContent(), /handed to the browser/i)
            const created = await page.evaluate(() => window.compatUrls.created.length)
            await downloaded(page, () => click(page.locator(id("export-download"))), report.bytes, filename)
            assert.equal(await page.evaluate(() => window.compatUrls.created.length), created, "retry must reuse the URL")
            // Simulated pagehide tests component ownership, not native bfcache policy.
            await page.evaluate(() => window.dispatchEvent(new Event("pagehide")))
            await downloaded(page, () => click(page.locator(id("export-download"))), report.bytes, filename)
            assert.equal(downloadCount, count + 3, "one gesture must produce one file")
            await click(page.locator(id("export-close")))
            assert.deepEqual(await page.evaluate(() => window.compatUrls.revoked), await page.evaluate(() => window.compatUrls.created))
            assert.ok(exports > 0)
            assert.deepEqual(unexpected, [])
            assert.deepEqual(errors, [])

            // Getter denial happens before render; preferences must still work.
            await page.addInitScript(() => Object.defineProperty(window, "localStorage", { configurable: true, get() { throw new DOMException("denied", "SecurityError") } }))
            await boot()
            await click(page.locator(id("export-trigger")))
            await page.locator(id("export-dialog")).waitFor()
            await page.keyboard.press("Escape")
            assert.deepEqual(errors, [])
          } catch (error) {
            await page.screenshot({ path: join(results, `${engine}-${device}.png`), fullPage: true }).catch(() => {})
            throw error
          } finally {
            await context.tracing.stop({ path: join(results, `${engine}-${device}.zip`) })
            await context.close()
          }
        })
      }
      await t.test("generated offline report via file:// and missing API diagnostic", async () => {
        const page = await browser.newPage({ locale: "en-US" })
        const errors = [], external = []
        page.on("pageerror", (error) => errors.push(error.message))
        page.on("request", (request) => { if (/^https?:/.test(request.url())) external.push(request.url()) })
        const url = pathToFileURL(report.output).href
        try {
          await page.goto(url)
          await page.locator(id("hour-picker-trigger")).waitFor({ timeout: 20_000 })
          assert.equal(await page.evaluate(async () => {
            const session = await globalThis.__KRONIKA_REPORT_RUNTIME__.ready
            const result = session.request("/api/catalog", "")
            try { return result.status } finally { result.free() }
          }), 200)
          for (const view of ["processes", "host.overview", "pg.activity", "events"]) {
            await page.goto(`${url}?view=${view}`)
            await page.locator(".timeline-shell").waitFor({ state: "attached" })
            await page.waitForFunction(() => !document.querySelector('[data-testid="hour-skeleton"], [data-testid="table-skeleton"]'))
            await page.locator("canvas").first().waitFor({ state: "attached" })
            if (view === "processes") await page.locator(".entity-row").first().waitFor()
          }
          assert.deepEqual(errors, [])
          assert.deepEqual(external, [])
          await page.addInitScript(() => { window.DecompressionStream = undefined })
          await page.goto(url)
          await page.locator(id("report-support-error")).waitFor()
          assert.match(await page.locator(id("report-support-error")).textContent(), /Safari.*16\.4/)
          assert.deepEqual(errors, [])
          assert.deepEqual(external, [])
        } finally { await page.close() }
      })
    } finally {
      await browser.close()
      await new Promise((resolve) => server.close(resolve))
      await writeFile(join(results, `${engine}-inputs.json`), JSON.stringify({ engine, version: browser.version(), channel: engine === "chromium" ? process.env.KRONIKA_CHROMIUM_CHANNEL ?? "playwright" : "playwright", artifact: hash(artifact), report: hash(report.bytes), reportBytes: report.bytes.length, configurations }, null, 2))
    }
  })
}
