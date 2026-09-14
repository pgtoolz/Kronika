import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises"
import { createServer } from "node:http"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"
import { gunzipSync } from "node:zlib"

const HOUR_US = 3_600_000_000
const HOUR = Date.UTC(2026, 7, 13, 5) * 1_000
const AT = HOUR + 1_800_000_000
const AUGUST_HOUR = Date.UTC(2026, 7, 10, 3) * 1_000
const DECEMBER_HOUR = Date.UTC(2026, 11, 31, 23) * 1_000
const JANUARY_HOUR = Date.UTC(2027, 0, 15, 9) * 1_000
const FEBRUARY_HOUR = Date.UTC(2027, 1, 1, 2) * 1_000
const DST_EDT_HOUR = Date.UTC(2026, 10, 1, 5) * 1_000
const DST_EST_HOUR = Date.UTC(2026, 10, 1, 6) * 1_000
const AVAILABLE_HOURS = [AUGUST_HOUR, HOUR + HOUR_US, DECEMBER_HOUR, JANUARY_HOUR, FEBRUARY_HOUR]
const SEGMENT = "7300"
const ARTIFACT = process.env.KRONIKA_UI_ARTIFACT ?? new URL("../kronika-ui.html.gz", import.meta.url)
const BEFORE_AT = AT - 5_000_000
const AFTER_AT = AT + 7_000_000
const EVENT_SCOPE_AT = AT + 900_000_000
const EVENT_SCOPE_AFTER = EVENT_SCOPE_AT + 7_000_000
const QUARTER = HOUR + 900_000_000
const QUARTER_PREVIOUS = QUARTER - 5_000_000
const QUARTER_NEXT = QUARTER + 5_000_000
const SESSION_COOKIE = `kronika_session=v1.2000000000.${"A".repeat(43)}`
const SLOW_PATTERN = 'SELECT "bulkoperations_bulktask"."id" FROM "bulkoperations_bulktask" WHERE "bulkoperations_bulktask"."status" = ? AND "bulkoperations_bulktask"."tenant_partition_with_a_deliberately_long_identifier" = ?'
const SLOW_DETAIL_REF = "evt_01K3K7NE2W4VQ9HAG6YCTB8RFS"
const SLOW_STORED_TEXT = `${SLOW_PATTERN} /* exact representative occurrence */`


const ZONE_VALUE = `document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value")`
const ZONE_LABEL = `document.querySelector('[data-testid="timezone-value"]')?.textContent`

async function switchZone(cdp, zone) {
  await cdp.evaluate(`document.querySelector('[data-testid="timezone-select"]').click()`)
  await cdp.waitFor(`document.querySelector('[data-testid="timezone-option-${zone}"]') !== null`, `the ${zone} timezone option`)
  await cdp.evaluate(`document.querySelector('[data-testid="timezone-option-${zone}"]').click()`)
}

async function clickCenter(cdp, selector) {
  const point = await cdp.evaluate(`(() => {
    const bounds = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect()
    return { x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 }
  })()`)
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...point })
  await cdp.send("Input.dispatchMouseEvent", { button: "left", buttons: 1, clickCount: 1, type: "mousePressed", ...point })
  await cdp.send("Input.dispatchMouseEvent", { button: "left", buttons: 0, clickCount: 1, type: "mouseReleased", ...point })
}

async function controlKey(cdp, key, code, windowsVirtualKeyCode) {
  await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Control", code: "ControlLeft", windowsVirtualKeyCode: 17 })
  await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key, code, modifiers: 2, windowsVirtualKeyCode })
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key, code, modifiers: 2, windowsVirtualKeyCode })
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Control", code: "ControlLeft", windowsVirtualKeyCode: 17 })
}

async function pasteClipboard(cdp) {
  const selector = '[data-testid="clipboard-paste-target"]'
  await cdp.evaluate(`(() => {
    const target = document.createElement("textarea")
    target.dataset.testid = "clipboard-paste-target"
    target.setAttribute("aria-label", "clipboard verification target")
    target.style.cssText = "position:fixed;bottom:0;left:0;opacity:0;pointer-events:none"
    document.querySelector('[data-testid="mcp-panel"]').append(target)
    target.focus({ preventScroll: true })
  })()`)
  await controlKey(cdp, "v", "KeyV", 86)
  await cdp.waitFor(`document.querySelector(${JSON.stringify(selector)}).value !== ""`, "the system clipboard paste")
  return cdp.evaluate(`(() => {
    const target = document.querySelector(${JSON.stringify(selector)})
    const value = target.value
    target.remove()
    return value
  })()`)
}

test("the first Process table does not wait for lanes, summary, or enrichment", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  const held = { companion: null, label: null, lanes: null, page: null, summary: null }
  const requested = {}
  const waitForRequest = (name) => new Promise((resolve) => { requested[name] = resolve })
  const companionRequest = waitForRequest("companion")
  const labelRequest = waitForRequest("label")
  const lanesRequest = waitForRequest("lanes")
  const pageRequest = waitForRequest("page")
  const summaryRequest = waitForRequest("summary")
  const abortedCompanionAt = []
  const processPageRequests = (at) => requests.filter(({ path, query }) => {
    const params = new URLSearchParams(query)
    return path.includes("/snapshot") && params.get("at") === String(at)
      && params.getAll("section").length === 1 && params.get("section") === "os_process"
      && params.has("page_size")
  })
  const companionRequests = (at) => requests.filter(({ path, query }) => {
    const params = new URLSearchParams(query)
    const sections = params.getAll("section")
    return path.includes("/snapshot") && params.get("at") === String(at)
      && sections.includes("instance_metadata") && !sections.includes("os_process")
  })
  const baseRecords = timelineRecords(HOUR, true).map((record) => record.record !== "finished_segment"
    ? record
    : {
        ...record,
        sections: [...record.sections, {
          logical_name: "os_meminfo", physical_name: "os_meminfo", type_id: "1104001",
          implementation: "linux", source_family: "system", rows: "1", bytes: "128",
        }],
      })
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") {
      held.label = response
      requested.label()
      return
    }
    if (url.pathname === "/api/hour") {
      if (url.searchParams.get("part") === "base") return ndjson(response, baseRecords)
      if (url.searchParams.get("part") === "lanes") {
        held.lanes = response
        requested.lanes()
        return
      }
      if (url.searchParams.get("section") === "os_process_summary") {
        held.summary = response
        requested.summary()
        return
      }
      return ndjson(response, baseRecords)
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      if (sections.length === 1 && sections[0] === "os_process" && url.searchParams.has("page_size")) {
        held.page = response
        requested.page()
        return
      }
      if (sections.includes("instance_metadata") && !sections.includes("os_process")) {
        response.once("close", () => {
          if (!response.writableEnded) abortedCompanionAt.push(url.searchParams.get("at"))
        })
        held.companion = response
        requested.companion()
        return
      }
      return ndjson(response, snapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("focused browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=disk` })

    await pageRequest
    await companionRequest
    assert.equal(requests.filter(({ path }) => path === "/api/instance-label").length, 0)
    assert.equal(requests.some(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("part") === "lanes"), false)
    assert.equal(requests.some(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_process_summary"), false)

    ndjson(held.page, snapshotRecords())
    held.page = null
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, "the first Process table", 15_000)
    await labelRequest
    await lanesRequest
    await summaryRequest
    const beforeLabel = await cdp.evaluate(`(() => {
      const rect = (node) => { const value = node.getBoundingClientRect(); return { bottom: value.bottom, height: value.height, left: value.left, right: value.right, top: value.top, width: value.width } }
      return {
        at: new URL(location.href).searchParams.get("at"),
        heading: document.querySelector(".topbar h1").textContent,
        rows: document.querySelectorAll('[data-testid="process-table"] .entity-row').length,
        table: rect(document.querySelector('[data-testid="process-table"]')),
        topbar: rect(document.querySelector(".topbar")),
      }
    })()`)
    assert.equal(beforeLabel.heading, "KRONIKA")
    assert.equal(beforeLabel.at, String(AT))
    assert.ok(beforeLabel.rows > 10, JSON.stringify(beforeLabel))
    held.label.writeHead(200, { "Content-Type": "application/json" })
    held.label.end('{"database":"artifact_db"}')
    held.label = null
    await cdp.waitFor(`document.querySelector('.topbar h1')?.textContent === "KRONIKA — artifact_db"`, "the deferred instance label")
    const afterLabel = await cdp.evaluate(`(() => {
      const rect = (node) => { const value = node.getBoundingClientRect(); return { bottom: value.bottom, height: value.height, left: value.left, right: value.right, top: value.top, width: value.width } }
      return {
        at: new URL(location.href).searchParams.get("at"),
        table: rect(document.querySelector('[data-testid="process-table"]')),
        topbar: rect(document.querySelector(".topbar")),
      }
    })()`)
    assert.equal(afterLabel.at, beforeLabel.at)
    for (const key of ["bottom", "height", "left", "right", "top", "width"]) {
      assert.ok(Math.abs(afterLabel.table[key] - beforeLabel.table[key]) <= .5, `${key}: ${JSON.stringify({ beforeLabel, afterLabel })}`)
      assert.ok(Math.abs(afterLabel.topbar[key] - beforeLabel.topbar[key]) <= .5, `${key}: ${JSON.stringify({ beforeLabel, afterLabel })}`)
    }
    assert.equal(requests.filter(({ path }) => path === "/api/instance-label").length, 1)
    const firstScreen = await cdp.evaluate(`(() => ({
      at: new URL(location.href).searchParams.get("at"),
      rows: document.querySelectorAll('[data-testid="process-table"] .entity-row').length,
      summary: document.querySelector('[data-testid="process-summary-status"]')?.textContent ?? "",
      wholeSkeleton: document.querySelector('[data-testid="hour-skeleton"]') !== null,
    }))()`)
    assert.equal(firstScreen.at, String(AT))
    assert.ok(firstScreen.rows > 10, JSON.stringify(firstScreen))
    assert.equal(firstScreen.wholeSkeleton, false, JSON.stringify(firstScreen))
    assert.match(firstScreen.summary, /Loading/)
    assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('.timeline-preview').getBoundingClientRect().height`) - 124) <= .5)

    const snapshotRequests = requests.filter(({ path }) => path.includes("/snapshot"))
    assert.equal(snapshotRequests.some(({ query }) => new URLSearchParams(query).getAll("section").includes("os_meminfo")), false)
    assert.equal(snapshotRequests.some(({ query }) => {
      const queryParams = new URLSearchParams(query)
      return queryParams.getAll("section").length === 1
        && queryParams.get("section") === "instance_metadata"
        && queryParams.getAll("field").includes("postgresql_interval_seconds")
    }), false)

    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent === "Events")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="events-console"]') !== null`, "Events while Process summary is pending")
    await delay(100)
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await waitForRequests(() => requests.filter(({ path, query }) => {
      const params = new URLSearchParams(query)
      return path.includes("/snapshot") && params.getAll("section").length === 1
        && params.get("section") === "os_process" && params.has("page_size")
    }).length === 2)
    await delay(100)
    assert.equal(requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_process_summary").length, 1)
    ndjson(held.page, snapshotRecords())
    held.page = null
    await waitForRequests(() => requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_process_summary").length === 2)

    ndjson(held.companion, snapshotRecords())
    held.companion = null
    ndjson(held.lanes, [
      { record: "lane_context", segment_id: SEGMENT, postgresql_interval_seconds: "30", environment: 0 },
      { record: "lane", segment_id: SEGMENT, lane: "io_stall", ts: String(AT), value: 12 },
    ])
    held.lanes = null
    ndjson(held.summary, processSummaryRecords(HOUR, 3, 80))
    held.summary = null
    await cdp.waitFor(`document.querySelector('.lane-select[aria-pressed="true"]')?.getAttribute("aria-label")?.includes("I/O PSI") === true`, "the pending Disk lane selection")
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]') === null`, "the Process summary")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(AT))
    assert.equal(requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_process_summary").length, 2)

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-cell"][aria-pressed="true"]') !== null`, "the current hour option")
    await cdp.evaluate(`document.querySelector('[data-testid="hour-cell"][aria-pressed="true"]').click()`)
    await cdp.waitFor(`document.querySelector('button[title="Refresh"]')?.disabled === false`, "the advancing refresh action")
    await cdp.evaluate(`document.querySelector('button[title="Refresh"]').click()`)
    await waitForRequests(() => processPageRequests(AFTER_AT).length === 1 && companionRequests(AFTER_AT).length === 1)
    const refreshedPage = held.page
    const refreshedCompanion = held.companion
    assert.notEqual(refreshedPage, null)
    assert.notEqual(refreshedCompanion, null)
    ndjson(refreshedPage, snapshotRecords().filter((record) => record.layout?.logical_name === "os_process" || record.type_id === "1100001"))
    held.page = null
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AFTER_AT}"`, "the advanced cursor")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, "the refreshed Process page")
    await cdp.waitFor(`![...document.querySelectorAll('[data-testid="process-table"] .entity-row span')].some((node) => node.textContent.trim() === "PG")`, "the page before its companion")
    await delay(400)
    assert.equal(processPageRequests(AFTER_AT).length, 1)
    assert.equal(companionRequests(AFTER_AT).length, 1)
    assert.equal(abortedCompanionAt.includes(String(AFTER_AT)), false)
    ndjson(refreshedCompanion, snapshotRecords()
      .filter((record) => record.layout?.logical_name === "pg_stat_activity" || record.type_id === "1001004")
      .map((record) => record.type_id === "1001004"
        ? { ...record, values: record.values.map((value, index) => index === 1 ? 2_686_800 : value) }
        : record))
    held.companion = null
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row span')].some((node) => node.textContent.trim() === "PG")`, "the late Process companion merge")
    assert.equal(processPageRequests(AFTER_AT).length, 1)
    assert.equal(companionRequests(AFTER_AT).length, 1)
    assert.equal(abortedCompanionAt.includes(String(AFTER_AT)), false)

    await cdp.evaluate(`document.querySelector('[data-testid="lens-tree"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="lens-tree"]')?.getAttribute("aria-pressed") === "true"`, "the Process Tree lens")
    await waitForRequests(() => requests.some(({ path, query }) => {
      if (!path.includes("/snapshot")) return false
      const sections = new URLSearchParams(query).getAll("section")
      return sections.includes("os_process") && sections.includes("os_meminfo")
    }))
    const treeRequest = requests.findLast(({ path, query }) => {
      if (!path.includes("/snapshot")) return false
      const sections = new URLSearchParams(query).getAll("section")
      return sections.includes("os_process") && sections.includes("os_meminfo")
    })
    assert.notEqual(treeRequest, undefined)
    const treeQuery = new URLSearchParams(treeRequest.query)
    assert.equal(treeQuery.getAll("section").includes("instance_metadata"), true)
    assert.deepEqual(treeQuery.getAll("field"), [])
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(AFTER_AT))
    assert.equal(requests.filter(({ path }) => path === "/api/instance-label").length, 1)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    for (const response of Object.values(held)) response?.destroy()
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("held current Process and Vacuum views stay locally busy before successful empty results", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const timeline = processReviewTimelineRecords()
  let pendingTree = null
  let pendingVacuum = null
  let releaseTreeRequest
  let releaseVacuumRequest
  const treeRequest = new Promise((resolve) => { releaseTreeRequest = resolve })
  const vacuumRequest = new Promise((resolve) => { releaseVacuumRequest = resolve })
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/hour") {
      if (url.searchParams.get("section") === "os_process_summary") return ndjson(response, processSummaryRecords(HOUR, 3, 0))
      if (url.searchParams.getAll("section").length === 1 && url.searchParams.get("section") === "pg_stat_progress_vacuum") {
        pendingVacuum = response
        releaseVacuumRequest()
        return
      }
      return ndjson(response, timeline)
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      if (sections.includes("os_process") && !url.searchParams.has("page_size") && pendingTree === null) {
        pendingTree = response
        releaseTreeRequest()
        return
      }
      return ndjson(response, [])
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("held Process view server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 800 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=tree` })
    await treeRequest
    await cdp.waitFor(`document.querySelector('[data-testid="process-table"] [data-testid="table-skeleton"]') !== null`, "the local Process loading rows", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the Russian loading label")
    const geometry = () => cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      const scroll = table.querySelector('.entity-scroll')
      const head = table.querySelector('.entity-head')
      const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
      return { head: rect(head), scroll: rect(scroll), table: rect(table) }
    })()`)
    const pending = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      const scroll = table.querySelector('.entity-scroll')
      const live = table.querySelector('[role="status"][aria-live="polite"]')
      return {
        busy: table.getAttribute('aria-busy'),
        empty: table.textContent.includes('На выбранный момент нет данных о процессах'),
        live: live?.textContent ?? '',
        progress: live?.querySelector('progress') !== null,
        rows: table.querySelectorAll('.entity-row').length,
        scrollBusy: scroll.getAttribute('aria-busy'),
        skeleton: table.querySelector('[data-testid="table-skeleton"]') !== null,
      }
    })()`)
    assert.deepEqual(pending, {
      busy: "false",
      empty: false,
      live: "Загружаем строки…",
      progress: true,
      rows: 0,
      scrollBusy: "true",
      skeleton: true,
    })
    const before = await geometry()
    const response = pendingTree
    pendingTree = null
    ndjson(response, [])
    await cdp.waitFor(`document.querySelector('[data-testid="process-table"]')?.textContent.includes('На выбранный момент нет данных о процессах') === true`, "the successful empty Process result", 15_000)
    await settleLayout(cdp)
    const ready = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      const scroll = table.querySelector('.entity-scroll')
      return {
        busy: table.getAttribute('aria-busy'),
        empty: table.textContent.includes('На выбранный момент нет данных о процессах'),
        live: table.querySelector('[role="status"][aria-live="polite"]') !== null,
        progress: table.querySelector('progress') !== null,
        scrollBusy: scroll.getAttribute('aria-busy'),
        skeleton: table.querySelector('[data-testid="table-skeleton"]') !== null,
      }
    })()`)
    assert.deepEqual(ready, {
      busy: "false",
      empty: true,
      live: false,
      progress: false,
      scrollBusy: "false",
      skeleton: false,
    })
    const after = await geometry()
    for (const part of ["head", "scroll", "table"]) {
      for (const edge of ["bottom", "height", "left", "right", "top", "width"]) {
        assert.ok(Math.abs(after[part][edge] - before[part][edge]) <= 1, `${part}.${edge}: ${JSON.stringify({ after, before })}`)
      }
    }
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.vacuum` })
    await vacuumRequest
    await cdp.waitFor(`document.querySelector('[data-testid="pg-vacuum-table"] .entity-scroll')?.getAttribute('aria-busy') === 'true'`, "the local Vacuum loading state", 15_000)
    const vacuumBefore = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-vacuum-table"]')
      const scroll = table.querySelector('.entity-scroll')
      const head = table.querySelector('.entity-head')
      const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
      window.__heldVacuumTable = table
      window.__heldVacuumScroll = scroll
      return {
        contentSized: table.parentElement.dataset.contentSized,
        empty: table.textContent.includes('Нет записанных данных о ходе VACUUM.'),
        geometry: { head: rect(head), scroll: rect(scroll), table: rect(table) },
        live: table.querySelector('[role="status"][aria-live="polite"]')?.textContent ?? '',
        progress: table.querySelector('[role="status"][aria-live="polite"] progress') !== null,
        scrollBusy: scroll.getAttribute('aria-busy'),
      }
    })()`)
    assert.deepEqual({ ...vacuumBefore, geometry: undefined }, {
      contentSized: "true",
      empty: false,
      geometry: undefined,
      live: "Загружаем строки…",
      progress: true,
      scrollBusy: "true",
    })
    ndjson(pendingVacuum, [])
    pendingVacuum = null
    await cdp.waitFor(`document.querySelector('[data-testid="pg-vacuum-table"]')?.textContent.includes('Нет записанных данных о ходе VACUUM.') === true`, "the successful empty Vacuum result", 15_000)
    await settleLayout(cdp)
    const vacuumAfter = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-vacuum-table"]')
      const scroll = table.querySelector('.entity-scroll')
      const head = table.querySelector('.entity-head')
      const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
      return {
        contentSized: table.parentElement.dataset.contentSized,
        empty: table.textContent.includes('Нет записанных данных о ходе VACUUM.'),
        geometry: { head: rect(head), scroll: rect(scroll), table: rect(table) },
        sameNodes: window.__heldVacuumTable === table && window.__heldVacuumScroll === scroll,
        scrollBusy: scroll.getAttribute('aria-busy'),
      }
    })()`)
    assert.deepEqual({ ...vacuumAfter, geometry: undefined }, {
      contentSized: "true",
      empty: true,
      geometry: undefined,
      sameNodes: true,
      scrollBusy: "false",
    })
    for (const part of ["head", "scroll", "table"]) {
      for (const edge of ["bottom", "height", "left", "right", "top", "width"]) {
        assert.ok(Math.abs(vacuumAfter.geometry[part][edge] - vacuumBefore.geometry[part][edge]) <= 1, `${part}.${edge}: ${JSON.stringify({ vacuumAfter, vacuumBefore })}`)
      }
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
    process.stdout.write(`${JSON.stringify({ heldProcessTable: { after, before }, heldVacuumTable: { after: vacuumAfter, before: vacuumBefore } }, null, 2)}\n`)
  } finally {
    pendingVacuum?.destroy()
    pendingTree?.destroy()
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("held first Host snapshot reserves local request frames and filtered cgroup failure keeps primary rows", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const systemSeries = new Set(systemIndexRecords(String(AT)).map(({ series }) => series))
  const extraSections = [{
    logical_name: "os_meminfo", physical_name: "os_meminfo", type_id: "1104001",
    implementation: "linux", source_family: "system", rows: "1", bytes: "128",
  }, {
    logical_name: "os_diskstats", physical_name: "os_diskstats", type_id: "1108001",
    implementation: "linux", source_family: "system", rows: "1", bytes: "256",
  }, {
    logical_name: "os_mountinfo", physical_name: "os_mountinfo", type_id: "1112002",
    implementation: "linux", source_family: "system", rows: "1", bytes: "256",
  }, {
    logical_name: "os_netdev", physical_name: "os_netdev", type_id: "1106001",
    implementation: "linux", source_family: "system", rows: "1", bytes: "256",
  }]
  const timeline = timelineRecords(HOUR, true)
    .filter((record) => record.record !== "point" || !systemSeries.has(record.series))
    .map((record) => record.record === "finished_segment" ? { ...record, sections: [...record.sections, ...extraSections] } : record)
  const requests = []
  let initial = true
  let initialHost = null
  let failedHost = null
  let failedMemory = false
  let heldLanes = null
  let holdAfterMemory = false
  let heldAfterMemory = null
  let readyAt = 0
  const primaryRecords = (at, target) => [
    ...systemSnapshotRecords(true, at).map((record) => record.record === "row" && record.type_id === "1108001"
      ? { ...record, values: record.values.map((stored, index) => index === 3 ? `device_${target}` : stored) }
      : record),
    layout("1112002", "os_mountinfo", ["ts", "major", "minor", "mount_point", "root", "fstype", "source", "is_k8s_infra", "total_bytes", "free_bytes", "total_inodes", "available_inodes", "scope"]),
    row("1112002", `mount-${target}`, [String(at), 8, 0, `/data-${target}`, "/", "ext4", `/dev/${target}`, false, 8_388_608, 4_194_304, 8_192, 7_168, 0], at),
    layout("1106001", "os_netdev", ["ts", "iface", "rx_bytes", "tx_bytes", "rx_packets", "tx_packets", "rx_errs", "tx_errs", "rx_drop", "tx_drop", "scope"]),
    row("1106001", `net-${target}`, [String(at), `interface_${target}`, 1000, 2000, 10, 20, 0, 0, 0, 0, 2], at),
  ]
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/hour") {
      if (url.searchParams.get("part") === "lanes") {
        heldLanes = response
        return
      }
      return ndjson(response, url.searchParams.has("section") ? [] : timeline)
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const at = Number(url.searchParams.get("at") ?? AT)
      const sections = url.searchParams.getAll("section")
      if (sections.includes("os_cpu")) {
        if (at === AT && initial) {
          initialHost = response
          return
        }
        if (at === BEFORE_AT) {
          failedHost = response
          return
        }
        const target = at === AFTER_AT ? "A" : `B${readyAt += 1}`
        return ndjson(response, primaryRecords(at, target))
      }
      if (sections.length === 1 && sections[0].startsWith("os_cgroup")) {
        if (at === AFTER_AT && sections[0] === "os_cgroup_memory" && holdAfterMemory) {
          heldAfterMemory = { response, url }
          return
        }
        if (at === AT && sections[0] === "os_cgroup_memory" && !failedMemory) {
          failedMemory = true
          return brokenNdjson(response)
        }
        return ndjson(response, cgroupSnapshotRecords(url))
      }
      return ndjson(response, [])
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("Host request-state server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Page.addScriptToEvaluateOnNewDocument", { source: `(() => {
      const lateSignals = new WeakSet()
      const nativeFetch = window.fetch.bind(window)
      const nativeAbort = AbortController.prototype.abort
      window.fetch = (input, init = {}) => {
        const requestUrl = new URL(input instanceof Request ? input.url : String(input), location.href)
        if (requestUrl.pathname.endsWith("/snapshot")
          && requestUrl.searchParams.get("at") === "${AFTER_AT}"
          && requestUrl.searchParams.getAll("section").length === 1
          && requestUrl.searchParams.get("section") === "os_cgroup_memory"
          && init.signal !== undefined) lateSignals.add(init.signal)
        return nativeFetch(input, init)
      }
      AbortController.prototype.abort = function(reason) {
        if (lateSignals.has(this.signal)) {
          window.__lateHostAbortSuppressed = (window.__lateHostAbortSuppressed ?? 0) + 1
          return
        }
        return nativeAbort.call(this, reason)
      }
    })()` })
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 800 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await waitForRequests(() => initialHost !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="system-request-state"]')?.getAttribute('aria-busy') === 'true'`, "the first Host request frame", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "the pending disk ledger row", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-disk"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="host-storage-modes"]') !== null`, "the pending storage modes")
    await cdp.evaluate(`document.querySelectorAll('[data-testid="host-storage-modes"] button')[2].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="storage-topology-request-state"]')?.getAttribute('aria-busy') === 'true'`, "the pending storage topology frame")
    await cdp.evaluate(`(() => {
      window.__hostRequestFrames = {
        system: document.querySelector('[data-testid="system-request-state"]'),
        topology: document.querySelector('[data-testid="storage-topology-request-state"]'),
      }
      return true
    })()`)

    const frameState = () => cdp.evaluate(`(() => {
      const read = (name, selector) => {
        const node = document.querySelector(selector)
        const box = node.getBoundingClientRect()
        return {
          busy: node.getAttribute('aria-busy'),
          box: { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width },
          live: node.querySelector('[role="status"]')?.getAttribute('aria-live') ?? null,
          progress: node.querySelector('progress') !== null,
          role: node.querySelector('[role="status"], [role="alert"]')?.getAttribute('role') ?? null,
          same: window.__hostRequestFrames[name] === node,
          text: node.textContent.trim(),
        }
      }
      return {
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        surfaceText: document.querySelector('.system-main').textContent,
        system: read('system', '[data-testid="system-request-state"]'),
        topology: read('topology', '[data-testid="storage-topology-request-state"]'),
      }
    })()`)
    const copy = {
      en: { empty: "No system metrics in the selected hour", error: "Could not load rows.", loading: "Loading rows…", topology: "No data in the selected hour" },
      ru: { empty: "В выбранном часу нет системных показателей", error: "Не удалось загрузить строки.", loading: "Загружаем строки…", topology: "В выбранном часу нет данных" },
    }
    const pending = {}
    for (const locale of ["ru", "en"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === "${locale}"`, `${locale} Host loading copy`)
      pending[locale] = {}
      for (const width of [800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
        await settleLayout(cdp)
        const measured = await frameState()
        assert.equal(measured.overflow, false, `${locale} ${width}px pending overflow`)
        assert.equal(measured.surfaceText.includes(copy[locale].empty), false, `${locale} ${width}px pending Host empty copy`)
        assert.equal(measured.surfaceText.includes(copy[locale].topology), false, `${locale} ${width}px pending topology empty copy`)
        for (const frame of [measured.system, measured.topology]) {
          assert.equal(frame.busy, "true", `${locale} ${width}px pending busy`)
          assert.equal(frame.live, "polite", `${locale} ${width}px pending live region`)
          assert.equal(frame.progress, true, `${locale} ${width}px pending progress`)
          assert.equal(frame.role, "status", `${locale} ${width}px pending role`)
          assert.equal(frame.text, copy[locale].loading, `${locale} ${width}px pending copy`)
          assert.equal(frame.box.height, 72, `${locale} ${width}px pending height`)
        }
        pending[locale][width] = measured
      }
    }

    const response = initialHost
    initialHost = null
    initial = false
    ndjson(response, [])
    await waitForRequests(() => heldLanes !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="system-request-state"]')?.getAttribute('aria-busy') === 'false'`, "the authoritative empty Host result", 15_000)
    const ready = {}
    for (const locale of ["ru", "en"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === "${locale}"`, `${locale} Host empty copy`)
      ready[locale] = {}
      for (const width of [800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
        await settleLayout(cdp)
        const measured = await frameState()
        assert.equal(measured.overflow, false, `${locale} ${width}px ready overflow`)
        assert.equal(measured.system.text, copy[locale].empty, `${locale} ${width}px Host empty copy`)
        assert.equal(measured.topology.text, copy[locale].topology, `${locale} ${width}px topology empty copy`)
        for (const name of ["system", "topology"]) {
          assert.equal(measured[name].busy, "false", `${locale} ${width}px ready busy`)
          assert.equal(measured[name].progress, false, `${locale} ${width}px ready progress`)
          assert.equal(measured[name].role, null, `${locale} ${width}px ready role`)
          assert.equal(measured[name].same, true, `${locale} ${width}px stable node`)
          for (const edge of ["bottom", "height", "left", "right", "top", "width"]) {
            assert.ok(Math.abs(measured[name].box[edge] - pending[locale][width][name].box[edge]) <= 1, `${locale} ${width}px ${name}.${edge}: ${JSON.stringify({ pending: pending[locale][width], ready: measured })}`)
          }
        }
        assert.ok(Math.abs((measured.system.box.top - measured.topology.box.bottom) - (pending[locale][width].system.box.top - pending[locale][width].topology.box.bottom)) <= 1, `${locale} ${width}px frame spacing`)
        ready[locale][width] = measured
      }
    }

    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "the failed Host target")
    await waitForRequests(() => failedHost !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="system-request-state"]')?.getAttribute('aria-busy') === 'true'`, "the failed target pending state")
    const failedPending = {}
    for (const locale of ["ru", "en"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === "${locale}"`, `${locale} Host failure pending copy`)
      failedPending[locale] = {}
      for (const width of [800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
        await settleLayout(cdp)
        const measured = await frameState()
        assert.equal(measured.overflow, false, `${locale} ${width}px failure pending overflow`)
        assert.equal(measured.surfaceText.includes(copy[locale].empty), false, `${locale} ${width}px failure pending Host empty copy`)
        assert.equal(measured.surfaceText.includes(copy[locale].topology), false, `${locale} ${width}px failure pending topology empty copy`)
        for (const name of ["system", "topology"]) {
          assert.equal(measured[name].busy, "true", `${locale} ${width}px failure pending busy`)
          assert.equal(measured[name].live, "polite", `${locale} ${width}px failure pending live region`)
          assert.equal(measured[name].progress, true, `${locale} ${width}px failure pending progress`)
          assert.equal(measured[name].role, "status", `${locale} ${width}px failure pending role`)
          assert.equal(measured[name].text, copy[locale].loading, `${locale} ${width}px failure pending copy`)
        }
        failedPending[locale][width] = measured
      }
    }
    const failedResponse = failedHost
    failedHost = null
    brokenNdjson(failedResponse)
    await cdp.waitFor(`document.querySelector('[data-testid="system-request-state"] [role="alert"]') !== null && document.querySelector('[data-testid="storage-topology-request-state"] [role="alert"]') !== null`, "the local Host failure states", 15_000)
    const failed = {}
    for (const locale of ["ru", "en"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === "${locale}"`, `${locale} Host failure copy`)
      failed[locale] = {}
      for (const width of [800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
        await settleLayout(cdp)
        const measured = await frameState()
        assert.equal(measured.overflow, false, `${locale} ${width}px failure overflow`)
        assert.equal(measured.surfaceText.includes(copy[locale].empty), false, `${locale} ${width}px failure Host empty copy`)
        assert.equal(measured.surfaceText.includes(copy[locale].topology), false, `${locale} ${width}px failure topology empty copy`)
        for (const name of ["system", "topology"]) {
          assert.equal(measured[name].busy, "false", `${locale} ${width}px failure busy`)
          assert.equal(measured[name].progress, false, `${locale} ${width}px failure progress`)
          assert.equal(measured[name].role, "alert", `${locale} ${width}px failure role`)
          assert.equal(measured[name].text, copy[locale].error, `${locale} ${width}px failure copy`)
          assert.equal(measured[name].same, true, `${locale} ${width}px failure stable node`)
          for (const edge of ["bottom", "height", "left", "right", "top", "width"]) {
            assert.ok(Math.abs(measured[name].box[edge] - failedPending[locale][width][name].box[edge]) <= 1, `${locale} ${width}px failure ${name}.${edge}: ${JSON.stringify({ pending: failedPending[locale][width], failed: measured })}`)
          }
        }
        failed[locale][width] = measured
      }
    }

    ndjson(heldLanes, timeline.filter((record) => record.record === "lane" || record.record === "lane_context"))
    heldLanes = null

    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowRight" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}"`, "the Host target with one cgroup failure")
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]') === null
      && document.querySelector('[data-testid="use-row-cgroup_cpu"]') !== null
      && document.querySelector('[data-testid="use-row-cgroup_pids"]') !== null
      && document.querySelector('[data-testid="use-cell-cgroup_memory-utilisation"]') !== null`, "the container USE rows", 15_000)
    assert.equal(failedMemory, true)
    const containerState = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="use-table"]')
      const cell = (key, column) => document.querySelector('[data-testid="use-cell-' + key + '-' + column + '"]')?.getAttribute("aria-label")
        ?? document.querySelector('[data-testid="use-empty-' + key + '-' + column + '"]')?.textContent
      const cells = (key) => ["utilisation", "saturation", "errors"].map((column) => cell(key, column))
      return {
        order: [...table.children].flatMap((node) => node.dataset.testid === undefined ? [] : [node.dataset.testid]),
        hostResourcesPresent: ["cpu", "memory", "disk"].every((key) => document.querySelector('[data-testid="use-toggle-' + key + '"]') !== null),
        network: document.querySelector('[data-testid="use-toggle-network"]') !== null,
        timeline: [...document.querySelectorAll('.timeline-lane-name')].map((node) => node.textContent),
        cgroupTabs: document.querySelector('[data-testid="host-cgroups-modes"]') !== null,
        cells: { cpu: cells("cgroup_cpu"), memory: cells("cgroup_memory"), io: cells("cgroup_io"), pids: cells("cgroup_pids") },
        verdicts: Object.fromEntries(["utilisation", "saturation", "errors"].map((column) => {
          const node = document.querySelector('[data-testid="use-verdict-' + column + '"]')
          return [column, { tag: node?.tagName, text: node?.textContent }]
        })),
        expandedContainer: [...document.querySelectorAll('[data-testid^="use-toggle-cgroup_"]')].filter((node) => node.getAttribute("aria-expanded") === "true").map((node) => node.dataset.testid),
        recordedCopy: table.textContent.includes("Recorded workload"),
      }
    })()`)
    assert.deepEqual(containerState.order, [
      "use-scope-container", "use-group-cgroup_cpu", "use-group-cgroup_memory", "use-group-cgroup_io", "use-group-cgroup_pids",
      "use-scope-namespace", "use-group-network",
      "use-scope-host", "use-group-cpu", "use-group-memory", "use-group-disk",
    ], JSON.stringify(containerState))
    assert.equal(containerState.hostResourcesPresent, true, JSON.stringify(containerState))
    assert.equal(containerState.network, true, JSON.stringify(containerState))
    // The rail is the container's own signal: cgroup shares and pressure, never node-wide /proc lanes.
    assert.deepEqual(containerState.timeline, ["Health", "CPU", "CPU PSI", "Memory", "I/O PSI"], JSON.stringify(containerState))
    assert.equal(containerState.cgroupTabs, false, JSON.stringify(containerState))
    assert.equal(containerState.recordedCopy, false, JSON.stringify(containerState))
    // The container rows are real USE rows read from the collector's own cgroup lanes; none is force-opened
    // (the host row of the auto-selected metric opens as on a machine).
    assert.deepEqual(containerState.expandedContainer, [], JSON.stringify(containerState))
    assert.match(containerState.cells.cpu[0], /^CPU · Utilization · Of limit: 25/, JSON.stringify(containerState.cells))
    assert.match(containerState.cells.cpu[1], /^CPU · Saturation · Throttled: 0.* · CPU PSI: 1\.5/, JSON.stringify(containerState.cells))
    assert.equal(containerState.cells.cpu[2], "—")
    assert.match(containerState.cells.memory[0], /^Memory · Utilization · Of limit: 40/, JSON.stringify(containerState.cells))
    assert.match(containerState.cells.memory[1], /^Memory · Saturation · Memory PSI: 0/, JSON.stringify(containerState.cells))
    assert.match(containerState.cells.memory[2], /^Memory · Errors · OOM kills: 0/, JSON.stringify(containerState.cells))
    assert.match(containerState.cells.io[0], /^I\/O · Utilization · Read: 2(\.0)? KiB\/s · Write: 4(\.0)? KiB\/s/, JSON.stringify(containerState.cells))
    assert.match(containerState.cells.io[1], /^I\/O · Saturation · I\/O PSI: 0/, JSON.stringify(containerState.cells))
    assert.equal(containerState.cells.io[2], "—")
    assert.match(containerState.cells.pids[0], /^Threads · Utilization · Of pids\.max: 3\.1/, JSON.stringify(containerState.cells))
    assert.deepEqual(containerState.cells.pids.slice(1), ["—", "—"])
    // The header is the hour's verdict: the peak share and whose it is, the non-zero pressures, the summed events.
    assert.equal(containerState.verdicts.utilisation.tag, "BUTTON", JSON.stringify(containerState.verdicts))
    assert.match(containerState.verdicts.utilisation.text, /^42% · Storage$/, JSON.stringify(containerState.verdicts))
    assert.equal(containerState.verdicts.saturation.tag, "BUTTON", JSON.stringify(containerState.verdicts))
    assert.match(containerState.verdicts.saturation.text, /CPU PSI 1\.5%/, JSON.stringify(containerState.verdicts))
    assert.deepEqual(containerState.verdicts.errors, { tag: "SPAN", text: "0 over the hour" }, JSON.stringify(containerState.verdicts))
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle.getAttribute("aria-expanded") === "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]')?.getAttribute("aria-expanded") === "false"`, "the disk row closed")
    await cdp.evaluate(`document.querySelector('[data-testid="use-verdict-utilisation"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]')?.getAttribute("aria-expanded") === "true"`, "the verdict opens the row it names")
    const emptyBefore = await cdp.evaluate(`(() => ({
      expanded: document.querySelector('[data-testid="use-toggle-cpu"]').getAttribute('aria-expanded'),
      metric: new URL(location.href).searchParams.get('metric'),
      tag: document.querySelector('[data-testid="use-empty-cpu-errors"]').tagName,
    }))()`)
    await cdp.evaluate(`document.querySelector('[data-testid="use-empty-cpu-errors"]').click()`)
    assert.deepEqual(await cdp.evaluate(`(() => ({
      expanded: document.querySelector('[data-testid="use-toggle-cpu"]').getAttribute('aria-expanded'),
      metric: new URL(location.href).searchParams.get('metric'),
      tag: document.querySelector('[data-testid="use-empty-cpu-errors"]').tagName,
    }))()`), emptyBefore)
    await cdp.evaluate(`document.querySelector('[data-testid="use-cell-disk-utilisation"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('metric') === 'disk_busy'
      && document.querySelector('[data-testid="use-toggle-disk"]')?.getAttribute('aria-expanded') === 'true'
      && document.querySelector('[data-testid="use-cell-disk-utilisation"]')?.getAttribute('aria-pressed') === 'true'`, "the exact USE disk action")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-group-chart-disk"]') !== null`), true)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the RU cgroup controls")
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
      await settleLayout(cdp)
      const controls = await cdp.evaluate(`(() => ({
        headers: [...document.querySelectorAll('[data-testid="use-table"] > header > span')].map((node) => node.querySelector('.label-help > span')?.textContent.trim() ?? node.textContent.trim()),
        verdictsInside: [...document.querySelectorAll('[data-testid^="use-verdict-"]')].every((node) => {
          const cell = node.parentElement.getBoundingClientRect()
          const box = node.getBoundingClientRect()
          return box.left >= cell.left - 1 && box.right <= cell.right + 1 && box.height > 0
        }),
        errors: document.querySelector('[data-testid="use-verdict-errors"]')?.textContent,
        resources: ['network', 'cpu', 'memory', 'disk'].map((key) => document.querySelector('[data-testid="use-toggle-' + key + '"]').textContent.trim()),
        container: ['cgroup_cpu', 'cgroup_memory', 'cgroup_io', 'cgroup_pids'].map((key) => document.querySelector('[data-testid="use-toggle-' + key + '"]').textContent.trim()),
        scopes: [...document.querySelectorAll('[data-testid^="use-scope-"]')].map((node) => node.textContent.trim()),
        documentOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      }))()`)
      assert.deepEqual(controls.headers, ["Ресурс", "Использование", "Насыщение", "Ошибки"], `RU ${width}px: ${JSON.stringify(controls)}`)
      assert.deepEqual(controls.scopes, ["Контейнер", "Сетевое пространство", "Хост"])
      assert.deepEqual(controls.resources, ["Сеть", "CPU", "Память", "Хранилище"])
      assert.deepEqual(controls.container, ["CPU", "Память", "I/O", "Потоки"])
      assert.equal(controls.errors, "0 за час")
      assert.equal(controls.verdictsInside, true, `RU ${width}px verdicts: ${JSON.stringify(controls)}`)
      assert.equal(controls.documentOverflow, false, `RU ${width}px overflow: ${JSON.stringify(controls)}`)
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "en"`, "the EN cgroup controls")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 800 })
    await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-cgroup_cpu"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-os_cgroup_cpu"] .entity-row') !== null`, "the successful cgroup CPU rows")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="use-expansion-cgroup_cpu"] [data-testid^="activity-"]') !== null`), true, "the CPU row discloses the cgroup activity ledger")
    await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-cgroup_memory"]').click()`)
    await settleLayout(cdp)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-panel-os_cgroup_memory"]') === null`), true)
    await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-cgroup_io"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-os_cgroup_io"] .entity-row') !== null`, "the successful cgroup I/O rows")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="use-expansion-cgroup_io"] [data-testid^="activity-"]') !== null`), true, "the I/O row discloses the cgroup activity ledger")
    const ioPresentation = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="system-os_cgroup_io"]')
      return {
        commonPath: document.querySelector('[data-testid="system-common-cgroup-path"]')?.textContent,
        hasPathColumn: [...table.querySelectorAll('.entity-header-cell')].some((node) => node.textContent.trim() === "Cgroup"),
        minCellFont: Math.min(...[...table.querySelectorAll('.entity-header-cell, .entity-cell')].map((node) => Number.parseFloat(getComputedStyle(node).fontSize))),
      }
    })()`)
    assert.deepEqual(ioPresentation, { commonPath: "/collector", hasPathColumn: false, minCellFont: 12 })
    const primaryState = await cdp.evaluate(`({
      emptyCopy: document.querySelector('.system-main').textContent.includes("No system metrics in the selected hour"),
      requestFrame: document.querySelector('[data-testid="system-request-state"]') !== null,
    })`)
    assert.deepEqual(primaryState, { emptyCopy: false, requestFrame: false })
    const failedCgroupRequests = requests.filter(({ path, query }) => {
      const params = new URLSearchParams(query)
      return path === `/api/segments/${SEGMENT}/snapshot` && params.get("at") === String(AT)
        && params.getAll("section").length === 1 && params.get("section") === "os_cgroup_memory"
    })
    assert.equal(failedCgroupRequests.length, 1)

    holdAfterMemory = true
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowRight" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AFTER_AT}"`, "the held later cgroup target")
    await waitForRequests(() => heldAfterMemory !== null)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-network"]'); if (toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-panel-os_netdev"]')?.textContent.includes("interface_B1") === true`, "the retained network rows while the cgroup batch is pending")
    assert.equal(await cdp.evaluate(`document.querySelector('.system-main').textContent.includes("interface_A")`), false)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}"`, "the newest Host target")
    await cdp.waitFor(`document.querySelector('[data-testid="system-panel-os_netdev"]')?.textContent.includes("interface_B2") === true`, "the newest Host snapshot", 15_000)
    const late = heldAfterMemory
    const fields = late.url.searchParams.getAll("field")
    const lateRecords = cgroupSnapshotRecords(late.url).map((record) => record.record === "row"
      ? { ...record, values: record.values.map((value, index) => fields[index] === "cgroup_path" ? "/stale-target-A" : value) }
      : record)
    ndjson(late.response, lateRecords)
    heldAfterMemory = null
    await delay(100)
    await cdp.waitFor(`window.__lateHostAbortSuppressed === 1`, "the delivered late Host response")
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cgroup_memory"]'); if (toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-panel-os_cgroup_memory"] .entity-row') !== null`, "the newest cgroup memory rows")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-panel-os_cgroup_memory"]').textContent.includes("stale-target-A")`), false)
    await cdp.waitFor(`document.querySelector('[data-testid="system-panel-os_netdev"]')?.textContent.includes("interface_B2") === true`, "the newest network rows after the late response")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(AT))
    assert.equal(late.response.writableEnded, true)
    // Every view of a container recording keeps the container rail: the recorded
    // environment travels with the timeline request, not only with the Host batch.
    await cdp.evaluate(`([...document.querySelectorAll(".source-tabs button")].find((button) => button.textContent === "Events")).click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "events" && document.querySelectorAll('.timeline-lane-name').length >= 5`, "the Events rail of the container recording", 15_000)
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('.timeline-lane-name')].map((node) => node.textContent)`), ["Health", "CPU", "CPU PSI", "Memory", "I/O PSI"])
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
    process.stdout.write(`${JSON.stringify({ hostRequestFrames: { failed, failedPending, pending, ready }, lateCgroupDelivered: true }, null, 2)}\n`)
  } finally {
    failedHost?.destroy()
    heldLanes?.destroy()
    heldAfterMemory?.response.destroy()
    initialHost?.destroy()
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the Process Inspector plot keeps its DOM while the shared cursor request settles", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const timeline = processReviewTimelineRecords()
  const heldCompanions = []
  const heldPages = []
  const historyRequests = []
  let holdCursorPages = false
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/hour") {
      const section = url.searchParams.get("section")
      if (section === "os_process_summary") return ndjson(response, processSummaryRecords(HOUR, 3, 80))
      if (section === "os_process" && url.searchParams.has("where.pid")) {
        historyRequests.push(url.search)
        return ndjson(response, processHistoryRecords(url))
      }
      return ndjson(response, timeline)
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const at = Number(url.searchParams.get("at") ?? AT)
      const sections = url.searchParams.getAll("section")
      if (sections.length === 1 && sections[0] === "os_process" && url.searchParams.has("page_size")) {
        if (holdCursorPages) {
          heldPages.push({ at, response })
          return
        }
        return ndjson(response, processSnapshotRecordsAt(at))
      }
      if (holdCursorPages) {
        heldCompanions.push({ at, response })
        return
      }
      return ndjson(response, instanceMetadataRecords(at))
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("stable Inspector server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=cpu` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, "the initial Process snapshot", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "en"`, "the English Inspector labels")
    await cdp.evaluate(`document.querySelector('[data-testid="process-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector"][data-panel="detail"]') !== null`, "the Process Detail Inspector")
    await cdp.evaluate(`([...document.querySelectorAll('.inspector-tabs [role="tab"]')].find((button) => button.textContent === 'Chart')).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector-chart"] [data-testid="process-history"] .uplot-host canvas') !== null && document.querySelector('[data-testid="inspector-chart"] [data-testid="series-status"]') === null`, "the settled Process history plot", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="process-history-metric-stime"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-history-metric-stime"]')?.getAttribute('aria-pressed') === 'true' && document.querySelector('[data-testid="inspector-chart"] [data-testid="series-status"]') === null`, "the selected system-time series")
    await settleLayout(cdp)
    const hoverPoint = await cdp.evaluate(`(() => {
      const box = document.querySelector('[data-testid="process-history"] .u-over').getBoundingClientRect()
      return { x: box.left + box.width * .55, y: box.top + box.height * .5 }
    })()`)
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...hoverPoint })
    await cdp.waitFor(`document.querySelector('[data-testid="process-history"] [data-testid="chart-hover-readout"]') !== null`, "the Process history hover readout")
    const baseline = await cdp.evaluate(`(() => {
      const figure = document.querySelector('[data-testid="process-history"] .uplot-figure')
      const host = figure.querySelector('.uplot-host')
      const plot = host.querySelector('.uplot')
      const over = host.querySelector('.u-over')
      const canvases = [...host.querySelectorAll('canvas')]
      const tooltip = figure.querySelector('[data-testid="chart-hover-readout"]')
      const observerTarget = document.querySelector('[data-testid="inspector-chart"]')
      const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
      const tracked = { blankFrames: 0, canvases, figure, host, observer: null, over, plot, removed: 0, running: true, tooltip }
      tracked.observer = new MutationObserver((records) => {
        for (const record of records) for (const node of record.removedNodes) {
          if (node === host || node === plot || node === tooltip || canvases.includes(node)
            || node.nodeType === Node.ELEMENT_NODE && (node.contains(host) || node.contains(plot) || node.contains(tooltip) || canvases.some((canvas) => node.contains(canvas)))) tracked.removed += 1
        }
      })
      tracked.observer.observe(observerTarget, { childList: true, subtree: true })
      const watch = () => {
        if (!tracked.running) return
        const currentCanvases = [...tracked.host.querySelectorAll('canvas')]
        if (!tracked.figure.isConnected || !tracked.host.isConnected || !tracked.plot.isConnected || !tracked.over.isConnected
          || currentCanvases.length !== tracked.canvases.length || tracked.canvases.some((canvas, index) => !canvas.isConnected || currentCanvases[index] !== canvas || canvas.getBoundingClientRect().width < 1 || canvas.getBoundingClientRect().height < 1)) tracked.blankFrames += 1
        requestAnimationFrame(watch)
      }
      requestAnimationFrame(watch)
      window.__stableProcessChart = tracked
      return {
        ariaLabel: host.getAttribute('aria-label'),
        boxes: { figure: rect(figure), host: rect(host), over: rect(over), plot: rect(plot), canvases: canvases.map(rect) },
        canvasCount: canvases.length,
        hovered: figure.dataset.hoveredTimestamp,
        metric: document.querySelector('[data-testid="process-history"] [aria-pressed="true"]')?.dataset.testid,
        navigationCount: figure.dataset.navigationCount,
        row: new URL(location.href).searchParams.get('row'),
        selected: figure.dataset.selectedTimestamp,
        summaryShape: figure.querySelector('.chart-summary')?.textContent.match(/\\d+ samples, \\d+ explicit nulls/)?.[0] ?? null,
        summaryText: figure.querySelector('.chart-summary')?.textContent ?? null,
        tooltipText: tooltip.textContent,
      }
    })()`)
    assert.ok(baseline.canvasCount > 0, JSON.stringify(baseline))
    assert.equal(baseline.metric, "process-history-metric-stime")
    assert.notEqual(baseline.row, null)
    assert.equal(baseline.selected, String(AT))

    const currentChart = () => cdp.evaluate(`new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => {
      const tracked = window.__stableProcessChart
      const figure = document.querySelector('[data-testid="process-history"] .uplot-figure')
      const host = figure?.querySelector('.uplot-host') ?? null
      const plot = host?.querySelector('.uplot') ?? null
      const over = host?.querySelector('.u-over') ?? null
      const canvases = host === null ? [] : [...host.querySelectorAll('canvas')]
      const tooltip = figure?.querySelector('[data-testid="chart-hover-readout"]') ?? null
      const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
      resolve({
        ariaLabel: host?.getAttribute('aria-label') ?? null,
        blankFrames: tracked.blankFrames,
        boxes: figure === null || host === null || plot === null || over === null ? null : { figure: rect(figure), host: rect(host), over: rect(over), plot: rect(plot), canvases: canvases.map(rect) },
        fallback: document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') !== null,
        identity: figure === tracked.figure && host === tracked.host && plot === tracked.plot && over === tracked.over
          && tooltip === tracked.tooltip && canvases.length === tracked.canvases.length && tracked.canvases.every((canvas, index) => canvases[index] === canvas),
        hovered: figure?.dataset.hoveredTimestamp ?? null,
        metric: document.querySelector('[data-testid="process-history"] [aria-pressed="true"]')?.dataset.testid ?? null,
        navigationCount: figure?.dataset.navigationCount ?? null,
        removed: tracked.removed,
        row: new URL(location.href).searchParams.get('row'),
        selected: figure?.dataset.selectedTimestamp ?? null,
        summaryShape: figure?.querySelector('.chart-summary')?.textContent.match(/\\d+ samples, \\d+ explicit nulls/)?.[0] ?? null,
        summaryText: figure?.querySelector('.chart-summary')?.textContent ?? null,
        tooltipConnected: tracked.tooltip?.isConnected === true,
        tooltipText: tooltip?.textContent ?? null,
      })
    })))`)
    const assertStable = async (target, stage, changedUnit = null) => {
      const current = await currentChart()
      assert.equal(current.identity, true, `${stage} identity: ${JSON.stringify(current)}`)
      assert.equal(current.removed, 0, `${stage} removals: ${JSON.stringify(current)}`)
      assert.equal(current.blankFrames, 0, `${stage} blank frames: ${JSON.stringify(current)}`)
      assert.equal(current.fallback, false, `${stage} fallback: ${JSON.stringify(current)}`)
      assert.equal(current.hovered, baseline.hovered, `${stage} hover timestamp`)
      assert.equal(current.metric, baseline.metric, `${stage} metric`)
      assert.equal(current.navigationCount, baseline.navigationCount, `${stage} timestamps`)
      assert.equal(current.row, baseline.row, `${stage} row`)
      assert.equal(current.selected, String(target), `${stage} selected timestamp`)
      assert.equal(current.summaryShape, baseline.summaryShape, `${stage} samples and nulls`)
      assert.equal(current.tooltipConnected, true, `${stage} hover node`)
      if (changedUnit === null) assert.equal(current.tooltipText, baseline.tooltipText, `${stage} hover text`)
      else {
        assert.match(current.ariaLabel, new RegExp(changedUnit.replace("/", "\\/")), `${stage} image label`)
        assert.match(current.tooltipText, new RegExp(changedUnit.replace("/", "\\/")), `${stage} hover unit`)
        assert.doesNotMatch(current.tooltipText, /—/, `${stage} preserved hover sample`)
        assert.notEqual(current.tooltipText, baseline.tooltipText, `${stage} converted value`)
      }
      assert.notEqual(current.boxes, null, `${stage} boxes`)
      for (const part of ["figure", "host", "over", "plot"]) {
        for (const edge of ["bottom", "height", "left", "right", "top", "width"]) {
          assert.ok(Math.abs(current.boxes[part][edge] - baseline.boxes[part][edge]) <= .75, `${stage} ${part}.${edge}: ${JSON.stringify({ baseline, current })}`)
        }
      }
      assert.equal(current.boxes.canvases.length, baseline.boxes.canvases.length, `${stage} canvases`)
      current.boxes.canvases.forEach((box, index) => {
        for (const edge of ["bottom", "height", "left", "right", "top", "width"]) {
          assert.ok(Math.abs(box[edge] - baseline.boxes.canvases[index][edge]) <= .75, `${stage} canvas ${index}.${edge}: ${JSON.stringify({ baseline, current })}`)
        }
      })
    }
    const moveCursor = async (key, target, leaveCompanionPending = false, ticksPerSecond = 100) => {
      const existingCompanions = heldCompanions.length
      const existing = heldPages.length
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: ${JSON.stringify(key)} }))`)
      await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${target}"`, `the ${target} cursor address`)
      await waitForRequests(() => heldPages.length > existing && heldPages.some((entry) => entry.at === target)
        && heldCompanions.length > existingCompanions && heldCompanions.some((entry) => entry.at === target))
      await cdp.waitFor(`document.querySelector('[data-testid="process-table"] .entity-scroll')?.getAttribute('aria-busy') === 'true'`, `the ${target} Process pending state`)
      assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="process-table"]')?.textContent.includes('No process data at the selected time') === false`), true)
      await assertStable(target, `${target} pending`)
      const held = heldPages.find((entry, index) => index >= existing && entry.at === target)
      assert.notEqual(held, undefined)
      ndjson(held.response, processSnapshotRecordsAt(target))
      await cdp.waitFor(`document.querySelector('[data-testid="process-table"] .entity-scroll')?.getAttribute('aria-busy') === 'false' && document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, `the ${target} Process replacement`, 15_000)
      await assertStable(target, `${target} companion pending`)
      const companion = heldCompanions.find((entry, index) => index >= existingCompanions && entry.at === target)
      assert.notEqual(companion, undefined)
      if (leaveCompanionPending) return companion
      ndjson(companion.response, instanceMetadataRecords(target, ticksPerSecond))
      if (ticksPerSecond === null) {
        await cdp.waitFor(`document.querySelector('[data-testid="process-history"] .uplot-host')?.getAttribute('aria-label').includes('ticks/s') === true`, `the ${target} authoritative raw-tick unit`)
      }
      await delay(50)
      await settleLayout(cdp)
      await assertStable(target, `${target} ready`, ticksPerSecond === null ? "ticks/s" : null)
      return companion
    }
    holdCursorPages = true
    const staleCompanion = await moveCursor("ArrowLeft", BEFORE_AT, true)
    await moveCursor("ArrowRight", AT)
    await waitForRequests(() => staleCompanion.response.destroyed)
    await moveCursor("ArrowRight", AFTER_AT, false, null)
    const finalState = await currentChart()
    assert.equal(historyRequests.length, 1, JSON.stringify(historyRequests))
    assert.equal(finalState.removed, 0)
    assert.equal(finalState.blankFrames, 0)
    await cdp.evaluate(`(() => { window.__stableProcessChart.running = false; window.__stableProcessChart.observer.disconnect() })()`)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
    process.stdout.write(`${JSON.stringify({ stableProcessChart: { baseline, final: finalState } }, null, 2)}\n`)
  } finally {
    for (const held of heldCompanions) if (!held.response.writableEnded) held.response.destroy()
    for (const held of heldPages) if (!held.response.writableEnded) held.response.destroy()
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the MCP drawer copies exactly through real legacy behavior and restores modal focus", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  const page = { errors: [], external: [], responses: [] }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end('{"database":"artifact_db"}')
      return
    }
    if (url.pathname === "/api/mcp-access") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end('{"authorization":null}')
      return
    }
    if (url.pathname === "/api/hour") {
      if (url.searchParams.get("section") === "os_process_summary") return ndjson(response, processSummaryRecords(HOUR, 3, 80))
      return ndjson(response, timelineRecords(HOUR, true))
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, snapshotRecords())
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("MCP modal server has no TCP address")
  const origin = `http://clipboard.test:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile, ["--host-resolver-rules=MAP clipboard.test 127.0.0.1", "--no-proxy-server"])
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=disk` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, "the Process table behind the MCP modal", 15_000)
    assert.equal(requests.filter(({ path }) => path === "/api/mcp-access").length, 0)
    assert.deepEqual(await cdp.evaluate(`({ clipboard: typeof navigator.clipboard, secure: isSecureContext })`), { clipboard: "undefined", secure: false })

    for (const locale of ["ru", "en"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${locale} MCP locale`)
      for (const viewport of [{ height: 800, width: 360 }, { height: 900, width: 1280 }]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: viewport.height, mobile: false, width: viewport.width })
        await settleLayout(cdp)
        const openedBefore = requests.filter(({ path }) => path === "/api/mcp-access").length
        await cdp.evaluate(`document.querySelector('[data-testid="mcp-trigger"]').focus({ preventScroll: true })`)
        await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", nativeVirtualKeyCode: 13, text: "\r", unmodifiedText: "\r", windowsVirtualKeyCode: 13 })
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", nativeVirtualKeyCode: 13, windowsVirtualKeyCode: 13 })
        await cdp.waitFor(`document.querySelector('[data-testid="mcp-panel"]')?.matches(':modal') === true`, `${locale} ${viewport.width}px modal MCP drawer`)
        await waitForRequests(() => requests.filter(({ path }) => path === "/api/mcp-access").length === openedBefore + 1)
        const geometry = await cdp.evaluate(`(() => {
          const dialog = document.querySelector('[data-testid="mcp-panel"]')
          const bounds = dialog.getBoundingClientRect()
          const title = document.getElementById(dialog.getAttribute("aria-labelledby"))
          return {
            activeInside: dialog.contains(document.activeElement),
            activeIsClose: document.activeElement === dialog.querySelector("button"),
            ariaModal: dialog.getAttribute("aria-modal"),
            bottom: bounds.bottom,
            documentOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
            height: bounds.height,
            labelled: title !== null && title.textContent.trim() !== "",
            left: bounds.left,
            modal: dialog.matches(":modal"),
            right: bounds.right,
            role: dialog.getAttribute("role"),
            top: bounds.top,
            viewport: { height: innerHeight, width: innerWidth },
            width: bounds.width,
          }
        })()`)
        assert.equal(geometry.modal, true, `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.equal(geometry.ariaModal, "true", `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.equal(geometry.role, "dialog", `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.equal(geometry.labelled, true, `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.equal(geometry.activeInside, true, `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.equal(geometry.activeIsClose, true, `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.equal(geometry.documentOverflow, false, `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.ok(geometry.left >= -1 && geometry.right <= geometry.viewport.width + 1, `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)
        assert.ok(Math.abs(geometry.top) <= 1 && Math.abs(geometry.bottom - geometry.viewport.height) <= 1, `${locale} ${viewport.width}: ${JSON.stringify(geometry)}`)

        assert.equal(await cdp.evaluate(`(() => { document.querySelector('[data-testid="mcp-trigger"]').focus(); return document.querySelector('[data-testid="mcp-panel"]').contains(document.activeElement) })()`), true, `${locale} ${viewport.width}px outside focus rejection`)
        await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 })
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 })
        assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="mcp-panel"]').contains(document.activeElement)`), true, `${locale} ${viewport.width}px Tab containment`)
        await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Tab", code: "Tab", modifiers: 8, windowsVirtualKeyCode: 9 })
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Tab", code: "Tab", modifiers: 8, windowsVirtualKeyCode: 9 })
        assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="mcp-panel"]').contains(document.activeElement)`), true, `${locale} ${viewport.width}px Shift-Tab containment`)

        const copyMode = locale === "ru" && viewport.width === 360
          ? "legacy"
          : locale === "ru" && viewport.width === 1280
            ? "blocked"
          : locale === "en" && viewport.width === 1280
              ? "legacy"
              : null
        if (copyMode !== null) {
          await cdp.evaluate(`document.querySelector('[data-testid="mcp-tab-codex"]').click()`)
          await cdp.waitFor(`document.querySelector('[data-testid="mcp-panel"] pre') !== null`, `${copyMode} Codex prompt`)
          const setup = await cdp.evaluate(`(() => {
            const prompt = document.querySelector('[data-testid="mcp-panel"] pre').textContent
            globalThis.__copyEvent = null
            globalThis.__copyObserver = (event) => {
              globalThis.__copyEvent = {
                activeIsTarget: document.activeElement === event.target,
                defaultPrevented: event.defaultPrevented,
                targetIsTextarea: event.target instanceof HTMLTextAreaElement,
                text: event.clipboardData?.getData("text/plain") ?? null,
                trusted: event.isTrusted,
              }
            }
            document.addEventListener("copy", globalThis.__copyObserver, { once: true })
            ${copyMode === "blocked" ? `Object.defineProperty(document, "execCommand", { configurable: true, value: () => false })` : ""}
            return { prompt }
          })()`)
          const copySelector = '[data-testid="mcp-panel"] button[aria-label$="Codex CLI"]'
          await clickCenter(cdp, copySelector)
          if (copyMode === "blocked") {
            await cdp.waitFor(`document.querySelector('[data-testid="manual-copy-fallback"]') !== null`, "the selected-text manual fallback")
          } else {
            await cdp.waitFor(`document.querySelector(${JSON.stringify(copySelector)}).textContent.trim() === ${JSON.stringify(locale === "ru" ? "Скопировано" : "Copied")}`, `${copyMode} exact copied state`)
          }
          const outcome = await cdp.evaluate(`(async () => {
            ${copyMode === "blocked" ? "delete document.execCommand" : ""}
            if (${JSON.stringify(copyMode)} !== "blocked") document.removeEventListener("copy", globalThis.__copyObserver)
            const area = document.querySelector('[data-testid="manual-copy-text"]')
            const fallback = document.querySelector('[data-testid="manual-copy-fallback"]')
            const button = document.querySelector(${JSON.stringify(copySelector)})
            const fallbackBounds = fallback?.getBoundingClientRect() ?? null
            return {
              activeIsArea: document.activeElement === area,
              activeIsButton: document.activeElement === button,
              buttonText: button.textContent.trim(),
              copyEvent: globalThis.__copyEvent,
              fallbackInsideModal: fallback === null ? null : document.querySelector('[data-testid="mcp-panel"]').contains(fallback),
              fallbackVisible: fallbackBounds === null ? null : fallbackBounds.width > 0 && fallbackBounds.height > 0
                && fallbackBounds.left >= 0 && fallbackBounds.right <= innerWidth
                && fallbackBounds.top >= 0 && fallbackBounds.bottom <= innerHeight,
              instruction: fallback?.querySelector('[role="alert"]')?.textContent ?? null,
              readOnly: area?.readOnly ?? null,
              selected: area === null ? null : area.value.slice(area.selectionStart, area.selectionEnd),
              selectionEnd: area?.selectionEnd ?? null,
              selectionStart: area?.selectionStart ?? null,
              value: area?.value ?? null,
            }
          })()`)
          if (copyMode === "blocked") {
            assert.equal(outcome.copyEvent, null, JSON.stringify(outcome))
            assert.equal(outcome.buttonText, locale === "ru" ? "Копировать" : "Copy", JSON.stringify(outcome))
            assert.equal(outcome.fallbackInsideModal, true, JSON.stringify(outcome))
            assert.equal(outcome.fallbackVisible, true, JSON.stringify(outcome))
            assert.match(outcome.instruction, /Ctrl\+C/)
            assert.equal(outcome.readOnly, true, JSON.stringify(outcome))
            assert.equal(outcome.activeIsArea, true, JSON.stringify(outcome))
            assert.equal(outcome.value, setup.prompt)
            assert.equal(outcome.selected, setup.prompt)
            assert.equal(outcome.selectionStart, 0)
            assert.equal(outcome.selectionEnd, setup.prompt.length)
            await controlKey(cdp, "c", "KeyC", 67)
            const manualCopy = await cdp.evaluate(`(() => {
              document.removeEventListener("copy", globalThis.__copyObserver)
              return globalThis.__copyEvent
            })()`)
            assert.deepEqual(manualCopy, {
              activeIsTarget: true,
              defaultPrevented: false,
              targetIsTextarea: true,
              text: "",
              trusted: true,
            })
            assert.equal(await pasteClipboard(cdp), setup.prompt)
          } else {
            assert.deepEqual(outcome.copyEvent, {
              activeIsTarget: true,
              defaultPrevented: true,
              targetIsTextarea: true,
              text: setup.prompt,
              trusted: true,
            })
            assert.equal(outcome.activeIsButton, true, JSON.stringify(outcome))
            assert.equal(outcome.fallbackInsideModal, null, JSON.stringify(outcome))
            assert.equal(await pasteClipboard(cdp), setup.prompt)
          }
        }
        await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
        await cdp.waitFor(`document.querySelector('[data-testid="mcp-panel"]') === null`, `${locale} ${viewport.width}px MCP Escape`)
        await cdp.waitFor(`document.activeElement === document.querySelector('[data-testid="mcp-trigger"]')`, `${locale} ${viewport.width}px MCP opener restore`)
        assert.equal(requests.filter(({ path }) => path === "/api/mcp-access").length, openedBefore + 1)
      }
    }
    assert.equal(requests.filter(({ path }) => path === "/api/mcp-access").length, 4)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the Export dialog selects a range on the timeline and reports its transfer as facts", { timeout: 180_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const exportFilename = "kronika-2026-08-13-182958-2026-08-13-183001-utc.html"
  const exportChunks = [
    "<!doctype html><title>Kronika export artifact</title><main>",
    "x".repeat(4_096),
    "y".repeat(8_192) + "</main>",
  ]
  const exportHtml = exportChunks.join("")
  const exportRequests = []
  const exportPlans = []
  const heldExports = []
  let dropExports = false
  const page = { errors: [], external: [], responses: [] }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname === "/api/export") {
      exportRequests.push({ ...requestRecord(request, url), accept: request.headers.accept ?? null })
      if (dropExports) {
        response.destroy()
        return
      }
      const plan = exportPlans.shift()
      if (plan === "stream") {
        heldExports.push(response)
        return
      }
      if (plan === "busy") {
        response.writeHead(503, { "Cache-Control": "no-store", "Content-Type": "application/json" })
        response.end('{"error":"export_busy"}')
        return
      }
      response.writeHead(500, { "Cache-Control": "no-store", "Content-Type": "application/json" })
      response.end('{"error":"export_failed"}')
      return
    }
    if (url.pathname === "/api/hour") {
      const hour = Number(url.searchParams.get("from") ?? HOUR)
      if (url.searchParams.get("section") === "os_process_summary") return ndjson(response, processSummaryRecords(hour, 3, 80))
      return ndjson(response, timelineRecords(hour, true))
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, snapshotRecords())
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("export browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const downloadDirectory = join(profile, "downloads")
  await mkdir(downloadDirectory)
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "Asia/Kolkata" })
    await cdp.send("Page.setDownloadBehavior", { behavior: "allow", downloadPath: downloadDirectory })
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=disk` })
    await cdp.waitFor(`document.querySelector('[data-testid="export-trigger"]:not(:disabled)') !== null`, "the Export action", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .u-over') !== null
      && document.querySelector('[data-testid="process-table"] .entity-row') !== null`, "the interactive timeline and process table", 15_000)

    const HOUR_SECOND = HOUR / 1_000_000
    const CURSOR_SECOND = AT / 1_000_000
    // Touch emulation is only switched on, for the phone width at the end:
    // switching it off again would leave the page without hover semantics.
    const setViewport = async (width) => {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
      if (width === 360) await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 1 })
      await settleLayout(cdp)
    }
    const setLocale = async (locale) => {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${locale} locale`)
    }
    const openDialog = async () => {
      await clickCenter(cdp, '[data-testid="export-trigger"]')
      await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]') !== null`, "the Export dialog")
      await settleLayout(cdp)
    }
    const pressEscape = async () => {
      await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
      await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    }
    const closeIdleDialog = async () => {
      await pressEscape()
      await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]') === null`, "idle Export close")
      await cdp.waitFor(`document.activeElement === document.querySelector('[data-testid="export-trigger"]')`, "Export trigger focus")
    }
    const typeClock = async (selector, value) => {
      await clickCenter(cdp, selector)
      await controlKey(cdp, "a", "KeyA", 65)
      await cdp.send("Input.insertText", { text: value })
      await cdp.waitFor(`document.querySelector(${JSON.stringify(selector)}).value === ${JSON.stringify(value)}`, `${selector} value`)
    }
    const dialogState = () => cdp.evaluate(`(() => {
      const dialog = document.querySelector('[data-testid="export-dialog"]')
      const shell = document.querySelector('.timeline-shell')
      return {
        from: dialog?.dataset.rangeFrom ?? null,
        to: dialog?.dataset.rangeTo ?? null,
        phase: dialog?.dataset.phase ?? null,
        preset: [...document.querySelectorAll('[data-testid^="export-preset-"]')].find((node) => node.getAttribute("aria-pressed") === "true")?.dataset.testid ?? null,
        shown: { from: shell?.dataset.exportFrom ?? null, to: shell?.dataset.exportTo ?? null },
        duration: document.querySelector('[data-testid="export-duration"]')?.textContent ?? null,
        filename: document.querySelector('[data-testid="export-filename"]')?.textContent ?? null,
        coverage: document.querySelector('[data-testid="export-coverage"]')?.textContent ?? null,
        status: document.querySelector('[data-testid="export-status"]')?.textContent.trim() ?? null,
        submitDisabled: document.querySelector('[data-testid="export-submit"]')?.disabled ?? null,
        closeDisabled: document.querySelector('[data-testid="export-close"]')?.disabled ?? null,
        expanded: document.querySelector('[data-testid="export-trigger"]')?.getAttribute("aria-expanded"),
      }
    })()`)

    await setLocale("en")
    await setViewport(1280)
    await openDialog()
    // A modal dialog inside the viewport over a scrim; the whole hour is the selection drawn on the timeline behind it.
    const opened = await dialogState()
    assert.deepEqual({ ...opened, coverage: undefined, duration: undefined }, {
      from: String(HOUR_SECOND), to: String(HOUR_SECOND + 3_599), phase: "idle", preset: "export-preset-hour",
      shown: { from: String(HOUR), to: String(HOUR + HOUR_US) },
      filename: "kronika-2026-08-13-050000-2026-08-13-055959-utc.html",
      status: "", submitDisabled: false, closeDisabled: false, expanded: "true",
      coverage: undefined, duration: undefined,
    }, JSON.stringify(opened))
    assert.equal(opened.duration, "1\u00a0h", JSON.stringify(opened))
    assert.equal(opened.coverage, null, "export shows the selected duration and filename without a collection-quality tally")
    const placement = await cdp.evaluate(`(() => {
      const node = document.querySelector('[data-testid="export-dialog"]')
      const rect = node.getBoundingClientRect()
      return { top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right, viewport: { height: innerHeight, width: innerWidth }, modal: node.getAttribute("aria-modal"), scrim: document.querySelector('[data-testid="export-scrim"]') !== null, overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth, active: document.activeElement?.dataset.testid ?? null }
    })()`)
    assert.ok(placement.top >= 60 && placement.bottom <= placement.viewport.height && Math.abs(placement.left - (placement.viewport.width - placement.right)) <= 2, JSON.stringify(placement))
    assert.deepEqual([placement.modal, placement.scrim, placement.overflow, placement.active], ["true", true, false, "export-preset-hour"], JSON.stringify(placement))

    // Presets read the cursor; shifting by an hour leaves the timeline, which then shows no selection.
    await clickCenter(cdp, '[data-testid="export-preset-around5"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.rangeFrom === "${CURSOR_SECOND - 300}"`, "the five-minute window")
    let state = await dialogState()
    assert.deepEqual([state.to, state.preset, state.shown, state.duration], [String(CURSOR_SECOND + 299), "export-preset-around5", { from: String(AT - 300_000_000), to: String(AT + 300_000_000) }, "10 min"], JSON.stringify(state))
    await clickCenter(cdp, '[data-testid="export-shift-back"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.rangeFrom === "${CURSOR_SECOND - 3_900}"`, "the shifted window")
    state = await dialogState()
    assert.deepEqual([state.preset, state.shown], [null, { from: null, to: null }], JSON.stringify(state))
    await clickCenter(cdp, '[data-testid="export-shift-forward"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.rangeFrom === "${CURSOR_SECOND - 300}"`, "the window back on the hour")

    // Exact seconds are one editable line in the shown zone; the seconds sent never change with the zone.
    await switchZone(cdp, "utc")
    await cdp.waitFor(`document.querySelector('[data-testid="export-from"]').value === "05:25:00"`, "UTC clocks")
    await typeClock('[data-testid="export-from"]', "05:10:00")
    await typeClock('[data-testid="export-to"]', "05:20:30")
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.rangeTo === "${HOUR_SECOND + 1_230}"`, "the typed end")
    state = await dialogState()
    assert.deepEqual([state.from, state.to, state.duration], [String(HOUR_SECOND + 600), String(HOUR_SECOND + 1_230), "10 min 31 s"], JSON.stringify(state))
    await switchZone(cdp, "browser")
    await cdp.waitFor(`document.querySelector('[data-testid="export-from"]').value === "10:40:00"`, "Kolkata clocks")
    assert.deepEqual([(await dialogState()).from, (await dialogState()).to], [String(HOUR_SECOND + 600), String(HOUR_SECOND + 1_230)])
    await typeClock('[data-testid="export-to"]', "25:00:00")
    await cdp.waitFor(`document.querySelector('[data-testid="export-to"]').getAttribute("aria-invalid") === "true"`, "the invalid clock")
    state = await dialogState()
    assert.deepEqual([state.status, state.submitDisabled, state.to], ["Time as HH:mm:ss", true, String(HOUR_SECOND + 1_230)], JSON.stringify(state))
    await typeClock('[data-testid="export-to"]', "10:35:00")
    await cdp.waitFor(`document.querySelector('[data-testid="export-status"]').textContent.trim() === "Start is later than end"`, "the order error")
    assert.equal((await dialogState()).submitDisabled, true)
    await clickCenter(cdp, '[data-testid="export-preset-hour"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.rangeFrom === "${HOUR_SECOND}" && document.querySelector('[data-testid="export-status"]').textContent.trim() === ""`, "the hour restored")

    // The day picker offers only days that hold recordings.
    await clickCenter(cdp, '[data-testid="export-from-day"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-day-grid"]') !== null`, "the day picker")
    const days = await cdp.evaluate(`(() => {
      const nodes = [...document.querySelectorAll('[data-testid="export-day"]')]
      return {
        pressed: nodes.filter((node) => node.getAttribute("aria-pressed") === "true").map((node) => node.dataset.day),
        enabled: nodes.filter((node) => !node.disabled).map((node) => node.dataset.day),
        disabled: nodes.filter((node) => node.disabled).length,
        month: document.querySelector('[data-testid="export-day-month"]').textContent,
      }
    })()`)
    assert.deepEqual(days.pressed, ["2026-08-13"], JSON.stringify(days))
    assert.ok(days.enabled.includes("2026-08-13") && days.disabled > 0 && days.enabled.every((day) => !day.startsWith("2026-08-2")), JSON.stringify(days))
    await clickCenter(cdp, '[data-testid="export-day"][data-day="2026-08-13"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-day-grid"]') === null`, "the day picker closed")
    assert.equal((await dialogState()).from, String(HOUR_SECOND))

    exportPlans.push("busy")
    const requestsBeforeBusy = exportRequests.length
    await clickCenter(cdp, '[data-testid="export-submit"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-status"]').textContent.trim() === "The server is already preparing another export, try again shortly"`, "the busy server")
    assert.equal(exportRequests.length, requestsBeforeBusy + 1)
    assert.equal(exportRequests.at(-1).query, `?from=${HOUR_SECOND}&to=${HOUR_SECOND + 3_599}`)
    assert.equal(exportRequests.at(-1).accept, "text/html")
    state = await dialogState()
    assert.deepEqual([state.phase, state.submitDisabled, state.closeDisabled], ["idle", false, false], JSON.stringify(state))

    // Preparation is counted in seconds without a bar, cannot be cancelled, and the download is one saved file.
    await cdp.evaluate(`(() => {
      const created = []
      const revoked = []
      const createObjectUrl = URL.createObjectURL.bind(URL)
      const revokeObjectUrl = URL.revokeObjectURL.bind(URL)
      globalThis.__exportObjectUrls = { created, revoked }
      URL.createObjectURL = (blob) => { const url = createObjectUrl(blob); created.push({ size: blob.size, type: blob.type, url }); return url }
      URL.revokeObjectURL = (url) => { revoked.push(url); return revokeObjectUrl(url) }
      localStorage.removeItem("kronika.export-seconds")
    })()`)
    exportPlans.push("stream")
    await clickCenter(cdp, '[data-testid="export-submit"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.phase === "preparing"`, "Export preparation")
    await delay(2_600)
    state = await dialogState()
    assert.match(state.status, /^preparing the slice · [2-9] s$/, JSON.stringify(state))
    assert.equal(state.closeDisabled, true, JSON.stringify(state))
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="logout-action"]').disabled`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="export-dialog"]').getAttribute("aria-busy")`), "true")
    await pressEscape()
    await delay(200)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="export-dialog"]')?.dataset.phase`), "preparing", "Escape does not abandon a running export")
    const held = heldExports.shift()
    assert.notEqual(held, undefined)
    held.writeHead(200, {
      "Cache-Control": "no-store",
      "Content-Disposition": `attachment; filename="${exportFilename}"`,
      "Content-Length": Buffer.byteLength(exportHtml),
      "Content-Type": "text/html; charset=utf-8",
    })
    held.flushHeaders()
    held.write(exportChunks[0])
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.phase === "downloading"`, "Export response headers")
    state = await dialogState()
    assert.match(state.status, /^downloading · [\d.,]+ (B|KiB) \/ 12[.,]\d KiB$/, JSON.stringify(state))
    for (const chunk of exportChunks.slice(1, -1)) held.write(chunk)
    held.end(exportChunks.at(-1))
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.phase === "saved"`, "the saved export", 15_000)
    state = await dialogState()
    assert.match(state.status, new RegExp(`^saved: ${exportFilename.replace(/\./g, "\\.")} · 12[.,]\\d KiB · \\d+ s$`), JSON.stringify(state))
    assert.equal(state.closeDisabled, false)
    const urls = await cdp.evaluate(`JSON.stringify(globalThis.__exportObjectUrls)`)
    const parsed = JSON.parse(urls)
    assert.equal(parsed.created.length, 1, urls)
    assert.equal(parsed.created[0].size, Buffer.byteLength(exportHtml), urls)
    assert.deepEqual(parsed.revoked, [parsed.created[0].url], urls)
    assert.ok(Number(await cdp.evaluate(`localStorage.getItem("kronika.export-seconds")`)) > 0)
    await clickCenter(cdp, '[data-testid="export-again"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.phase === "idle" && document.querySelector('[data-testid="export-submit"]') !== null`, "another export offered")

    // The second run remembers how long the first one took.
    exportPlans.push("stream")
    await clickCenter(cdp, '[data-testid="export-submit"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.phase === "preparing"`, "the second preparation")
    await delay(400)
    assert.match((await dialogState()).status, /^preparing the slice · \d+ s · last time \d+ s$/)
    const second = heldExports.shift()
    second.writeHead(200, { "Cache-Control": "no-store", "Content-Disposition": `attachment; filename="${exportFilename}"`, "Content-Type": "text/html; charset=utf-8" })
    second.end(exportHtml)
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.phase === "saved"`, "the second saved export", 15_000)
    await clickCenter(cdp, '[data-testid="export-again"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-submit"]') !== null && !document.querySelector('[data-testid="export-submit"]').disabled`, "a third export offered")

    // A connection lost before any status leaves the server state unknown: no restart, no close.
    dropExports = true
    await clickCenter(cdp, '[data-testid="export-submit"]')
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]').dataset.phase === "unknown"`, "unknown server state after a pre-header disconnect", 10_000)
    state = await dialogState()
    assert.equal(state.status, "Connection lost. The server may still be preparing the report; a new run stays locked", JSON.stringify(state))
    assert.deepEqual([state.closeDisabled, state.submitDisabled], [true, true], JSON.stringify(state))
    await pressEscape()
    await delay(200)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="export-dialog"]')?.dataset.phase`), "unknown")
    dropExports = false
    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector('[data-testid="export-trigger"]:not(:disabled)') !== null && document.querySelector('[data-testid="export-dialog"]') === null`, "the reloaded page", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .u-over') !== null`, "the reloaded timeline", 15_000)

    await openDialog()
    await closeIdleDialog()
    const idleBaseUrl = await cdp.evaluate("location.href")
    await cdp.evaluate(`(() => { const next = new URL(location.href); next.searchParams.set("export_probe", "idle"); history.pushState({}, "", next) })()`)
    await openDialog()
    await cdp.evaluate("history.back()")
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]') === null`, "idle Back close")
    await cdp.waitFor(`location.href === ${JSON.stringify(idleBaseUrl)}`, "idle Back address")
    await openDialog()
    await cdp.evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="export-dialog"]') === null`, "idle hour close")
    await cdp.waitFor(`Math.floor(Number(new URL(location.href).searchParams.get("at")) / ${HOUR_US}) * ${HOUR_US} === ${HOUR + HOUR_US}`, "the next selected hour", 15_000)
    await cdp.evaluate("history.back()")
    await cdp.waitFor(`Math.floor(Number(new URL(location.href).searchParams.get("at")) / ${HOUR_US}) * ${HOUR_US} === ${HOUR}`, "the restored selected hour", 15_000)

    for (const width of [1280, 800, 360]) {
      await setViewport(width)
      for (const locale of ["ru", "en"]) {
        await setLocale(locale)
        await openDialog()
        const measured = await cdp.evaluate(`(() => {
          const node = document.querySelector('[data-testid="export-dialog"]')
          const rect = node.getBoundingClientRect()
          const targets = [...node.querySelectorAll("button, input")].filter((candidate) => candidate.offsetParent !== null).map((candidate) => { const box = candidate.getBoundingClientRect(); return { id: candidate.dataset.testid ?? candidate.textContent, height: box.height, width: box.width } })
          return {
            bottom: rect.bottom, top: rect.top, left: rect.left, right: rect.right, viewport: { height: innerHeight, width: innerWidth },
            overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
            presets: [...node.querySelectorAll('[data-testid^="export-preset-"]')].map((candidate) => candidate.textContent),
            targets,
            title: node.querySelector("h2").textContent,
          }
        })()`)
        const label = `${width}px ${locale}: ${JSON.stringify(measured)}`
        assert.equal(measured.overflow, false, label)
        assert.ok(measured.left >= -1 && measured.right <= measured.viewport.width + 1 && measured.top >= -1 && measured.bottom <= measured.viewport.height + 1, label)
        assert.equal(measured.title, locale === "ru" ? "Экспорт HTML" : "HTML export", label)
        assert.deepEqual(measured.presets, locale === "ru" ? ["Этот час", "±5 мин", "±15 мин", "±30 мин"] : ["This hour", "±5 min", "±15 min", "±30 min"], label)
        if (width === 360) {
          assert.ok(Math.abs(measured.bottom - measured.viewport.height) <= 1 && measured.right - measured.left >= measured.viewport.width - 1, `the phone sheet: ${label}`)
          assert.ok(measured.targets.every(({ height }) => height >= 43.5), label)
        } else {
          assert.ok(Math.abs(measured.left - (measured.viewport.width - measured.right)) <= 2 && measured.right - measured.left <= 600, `centred: ${label}`)
        }
        await closeIdleDialog()
      }
    }
    assert.deepEqual(page.external, [])
  } finally {
    for (const response of heldExports) response.destroy()
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the first Events table settles before timeline lanes start", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  const held = { events: null, lanes: null }
  let eventsRequested
  let lanesRequested
  const eventsRequest = new Promise((resolve) => { eventsRequested = resolve })
  const lanesRequest = new Promise((resolve) => { lanesRequested = resolve })
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end('{"database":"artifact_db"}')
      return
    }
    if (url.pathname === "/api/events") {
      held.events = response
      eventsRequested()
      return
    }
    if (url.pathname === "/api/hour") {
      if (url.searchParams.get("part") === "base") return ndjson(response, slowQueryTimelineRecords())
      if (url.searchParams.get("part") === "lanes") {
        held.lanes = response
        lanesRequested()
        return
      }
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("focused browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=events` })

    await eventsRequest
    await delay(100)
    assert.equal(requests.some(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("part") === "lanes"), false)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="events-console"]') !== null`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="events-console"] > header [role="status"]')?.textContent`), "Loading log…")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="events-console"] > header').textContent.includes("0")`), false)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(AT))

    ndjson(held.events, slowQueryEventRecords())
    held.events = null
    await cdp.waitFor(`document.querySelector('[data-testid="event-entry-title"]') !== null`, "the first Events table", 15_000)
    await lanesRequest
    assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('[data-testid="timeline-empty"], .timeline-preview').getBoundingClientRect().height`) - 124) <= .5)

    ndjson(held.lanes, [{ record: "error", error: "stale" }])
    held.lanes = null
    await delay(100)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="event-entry-title"]') !== null`), true)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(AT))
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    for (const response of Object.values(held)) response?.destroy()
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("display timezone and human chart precision stay global", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  let heldPostgresSummary = null
  let postgresSummaryRequested
  const postgresSummaryRequest = new Promise((resolve) => { postgresSummaryRequested = resolve })
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end('{"database":"artifact_db"}')
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const hour = Number(url.searchParams.get("from") ?? HOUR)
      if (url.searchParams.get("section") === "postgresql_summary") {
        // A scope change re-requests the summary; the superseded request was aborted by the page.
        heldPostgresSummary = { hour, response }
        postgresSummaryRequested()
        return
      }
      const records = timelineRecords(hour).map((record) => record.record === "point" && record.series === "os_health" && record.ts === String(AT)
        ? { ...record, value: 41.729068244136855 }
        : record)
      return ndjson(response, records)
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      if (sections.includes("pg_store_plans")) return ndjson(response, planRecords())
      const statements = sections.includes("pg_stat_statements")
      // Like the server, the workload scope keeps the collector's own statement
      // off the page and reports it in the page trailer.
      const workload = url.searchParams.get("scope") === "workload"
      return ndjson(response, statements
        ? statementRecords(true, workload ? 1 : 2, false, workload ? 1 : 2).map((record) => record.record === "snapshot_page"
            ? { ...record, excluded: workload ? "1" : "0" }
            : record.record !== "row" ? record : {
              ...record,
              values: record.values.map((stored, index) => index === 1
                ? record.ordinal === "91" ? "101" : "102"
                : index === 7 && record.ordinal !== "91" ? "/* kronika:1.0.0 statements */ select monitor" : stored),
            })
        : snapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("focused browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const result = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, result)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "Europe/Moscow" })
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.statements` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length === 1`, "the workload Statements path", 15_000)
    await settleLayout(cdp)

    const defaultScope = await cdp.evaluate(`(() => ({
      activity: document.querySelector('[data-testid="activity-pg_stat_statements"]') !== null,
      checked: document.querySelector('[data-testid="statement-monitor-scope"] input')?.checked,
      count: document.querySelector('[data-testid="statement-monitor-count"]')?.textContent,
      rows: document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length,
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent,
      summary: document.querySelector('[data-summary-fact="active_statements"]') !== null,
    }))()`)
    assert.deepEqual({ ...defaultScope, status: undefined }, { activity: true, checked: false, count: "hidden: 1", rows: 1, status: undefined, summary: false })
    assert.match(defaultScope.status, /Loaded 1 of 1/)
    assert.equal(requests.filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && new URLSearchParams(query).get("scope") === "workload").length, 1, JSON.stringify(requests.map(({ path, query }) => path + query)))

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the Russian statement scope")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 360 })
    await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 })
    await settleLayout(cdp)
    const narrowScope = await cdp.evaluate(`(() => {
      const scope = document.querySelector('[data-testid="statement-monitor-scope"]')
      const label = scope.querySelector('span:not([data-testid])').getBoundingClientRect()
      const count = scope.querySelector('[data-testid="statement-monitor-count"]').getBoundingClientRect()
      const bounds = scope.getBoundingClientRect()
      const toolbar = scope.closest('[data-search-surface]').getBoundingClientRect()
      const query = document.querySelector('[data-testid="pg-statements-table"] [data-field="query"] .entity-value')
      const row = document.querySelector('[data-testid="pg-statements-table"] .entity-row')
      const header = document.querySelector('[data-testid="pg-statements-table"] .entity-head')
      const style = (element) => {
        const computed = getComputedStyle(element)
        return { family: computed.fontFamily, size: parseFloat(computed.fontSize), weight: computed.fontWeight }
      }
      const heights = [...document.querySelectorAll('[data-testid^="statement-lens-"]'), document.querySelector('[data-testid="mobile-search-open"]'), row, header]
        .filter(Boolean).map((element) => ({ name: element.dataset.testid ?? element.className, height: element.getBoundingClientRect().height }))
      return {
        bounds: { left: bounds.left, right: bounds.right },
        coarse: matchMedia('(pointer: coarse)').matches,
        sameLine: count.top < label.bottom && count.left >= label.right,
        documentOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        heights,
        human: style(scope.querySelector('span:not([data-testid])')),
        query: style(query),
        toolbar: { left: toolbar.left, right: toolbar.right },
      }
    })()`)
    assert.equal(narrowScope.coarse, true, JSON.stringify(narrowScope))
    assert.equal(narrowScope.documentOverflow, false, JSON.stringify(narrowScope))
    assert.equal(narrowScope.sameLine, true, JSON.stringify(narrowScope))
    assert.equal(narrowScope.query.size, 12, JSON.stringify(narrowScope))
    assert.match(narrowScope.query.family, /JetBrains Mono/)
    assert.equal(narrowScope.query.weight, "400")
    assert.equal(narrowScope.human.size, 12, JSON.stringify(narrowScope))
    assert.match(narrowScope.human.family, /IBM Plex Sans/)
    assert.ok(narrowScope.heights.every(({ height }) => height >= 44), JSON.stringify(narrowScope))
    assert.ok(narrowScope.bounds.left >= narrowScope.toolbar.left - 1 && narrowScope.bounds.right <= narrowScope.toolbar.right + 1, JSON.stringify(narrowScope))
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "en"`, "the English statement scope")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await settleLayout(cdp)
    await cdp.evaluate(`document.querySelector('[data-testid="statement-monitor-scope"] input').click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length === 2 && document.querySelector('[data-testid="activity-pg_stat_statements"] [data-testid="activity-toggle"]')?.getAttribute("aria-expanded") === "false"`, "the Statements collector-query scope")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="statement-monitor-count"]')?.textContent`), "shown")

    const postgresSummaryRequests = () => requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "postgresql_summary")
    await postgresSummaryRequest
    // The summary follows the same scope as the table: first the workload, then every statement.
    await waitForRequests(() => postgresSummaryRequests().length === 2)
    assert.deepEqual(postgresSummaryRequests().map(({ query }) => new URLSearchParams(query).get("scope")), ["workload", null])
    assert.notEqual(heldPostgresSummary, null)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length >= 1`), true)
    ndjson(heldPostgresSummary.response, postgresSummaryRecords(heldPostgresSummary.hour))
    heldPostgresSummary = null
    await cdp.waitFor(`document.querySelector('[data-summary-fact="active_statements"] strong')?.textContent === "3 · 60%"`, "the PostgreSQL statement context")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 720, mobile: false, width: 320 })
    await settleLayout(cdp)
    const mobileSummary = await cdp.evaluate(`(() => {
      const bar = document.querySelector('.pg-table-workspace > .lensbar')
      const summary = bar.querySelector(':scope > .process-summary-inline').getBoundingClientRect()
      const legend = bar.querySelector(':scope > .process-summary-inline').nextElementSibling.getBoundingClientRect()
      return {
        documentOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        verticalOverlap: Math.min(summary.bottom, legend.bottom) - Math.max(summary.top, legend.top),
      }
    })()`)
    assert.equal(mobileSummary.documentOverflow, false, JSON.stringify(mobileSummary))
    assert.ok(mobileSummary.verticalOverlap > 0, JSON.stringify(mobileSummary))
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await settleLayout(cdp)
    await cdp.evaluate(`document.querySelector('[data-testid="statement-lens-per_call"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-summary-fact="execution_per_call"]') !== null`, "the local statement lens context")
    assert.equal(postgresSummaryRequests().length, 2)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Tables")).click()`)
    await cdp.waitFor(`document.querySelector('[data-summary-fact="scan_methods"]') !== null`, "the table context")
    assert.equal(postgresSummaryRequests().length, 2)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Databases")).click()`)
    await cdp.waitFor(`document.querySelector('[data-summary-fact="rollbacks"] strong')?.textContent === "20%"`, "the database context")
    assert.equal(postgresSummaryRequests().length, 2)
    await cdp.evaluate(`(() => {
      const navigator = document.querySelector('[data-testid="hour-timeline"] input.chart-navigator')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(navigator, "2")
      navigator.dispatchEvent(new Event("input", { bubbles: true }))
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}" && document.querySelector('[data-summary-fact="rollbacks"] strong')?.textContent === "10%"`, "the local PostgreSQL cursor context")
    assert.equal(postgresSummaryRequests().length, 2)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}"`, "the restored PostgreSQL cursor")
    await cdp.waitFor(`document.querySelector('button[title="Refresh"]')?.disabled === false`, "the refresh action")
    await cdp.evaluate(`document.querySelector('button[title="Refresh"]').click()`)
    await waitForRequests(() => postgresSummaryRequests().length === 3)
    assert.equal(postgresSummaryRequests().length, 3)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Statements")).click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length >= 1`, "the restored Statements path")

    await cdp.waitFor(`document.querySelector('[data-testid="activity-toggle"]')?.getAttribute("aria-expanded") === "false"`, "the collapsed activity ledger")
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/heatmap")).length, 0)
    let statementSnapshotsBeforeActivity = requests.filter(({ path }) => path.includes("/snapshot")).length
    await cdp.evaluate(`document.querySelector('[data-testid="activity-toggle"]').click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"]').length === 2`, "the ranked activity ledger", 15_000)
    assert.ok(requests.filter(({ path }) => path.startsWith("/api/heatmap")).length >= 1)
    assert.equal(requests.filter(({ path, query }) => path.includes("/snapshot") && new URLSearchParams(query).get("first_match") === "1").length, 0)
    const ledger = await cdp.evaluate(`(() => ({
      top: document.querySelector('[data-testid="activity-top-count"]')?.textContent ?? "",
      totals: document.querySelector('[data-testid="activity-row-totals"]') !== null,
      others: document.querySelector('[data-testid="activity-row-others"]')?.textContent ?? "",
      cells: document.querySelectorAll('[data-testid="activity-row"] rect').length,
      help: document.querySelectorAll('[data-testid="activity-pg_stat_statements"] .help-dot').length,
      labels: [...document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"] > span[title]')].map((row) => row.getAttribute("title")),
    }))()`)
    assert.equal(ledger.totals, true)
    assert.match(ledger.top, /2/)
    assert.match(ledger.others, /1/)
    assert.equal(ledger.cells, 6)
    assert.ok(ledger.help >= 3)
    assert.deepEqual(ledger.labels, ["select artifact_exact_context", "/* kronika:1.0.0 statements */ select monitor"])
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the Russian Activity previews")
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"] > span[title]')].map((row) => row.getAttribute("title"))`), ["select artifact_exact_context", "/* kronika:1.0.0 statements */ select monitor"])
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "en"`, "the English Activity previews")
    // The ledger follows the same scope as the table: hiding Kronika's queries reranks without them.
    const heatmapScopes = () => requests.filter(({ path }) => path.startsWith("/api/heatmap")).map(({ query }) => new URLSearchParams(query).get("scope"))
    assert.ok(heatmapScopes().every((scope) => scope === null), JSON.stringify(heatmapScopes()))
    await cdp.evaluate(`document.querySelector('[data-testid="statement-monitor-scope"] input').click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"]').length === 1 && document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length === 1`, "the workload-only ledger", 15_000)
    assert.equal(heatmapScopes().at(-1), "workload")
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"] > span[title]')].map((row) => row.getAttribute("title"))`), ["select artifact_exact_context"])
    await cdp.evaluate(`document.querySelector('[data-testid="statement-monitor-scope"] input').click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"]').length === 2 && document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length === 2`, "the restored full ledger", 15_000)
    // Each scope change reloaded the page once; the ledger itself never asks for rows.
    assert.equal(requests.filter(({ path }) => path.includes("/snapshot")).length, statementSnapshotsBeforeActivity + 2)
    statementSnapshotsBeforeActivity += 2
    await cdp.evaluate(`document.querySelector('[data-testid="activity-cut-wal_bytes"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-cut-wal_bytes"]')?.getAttribute("aria-pressed") === "true"`, "the WAL cut")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-pg_stat_statements"] [data-testid="activity-row"]').length === 2`, "the reranked ledger", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="activity-maximize"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-overlay"]') !== null`, "the full-screen ledger")
    await cdp.evaluate(`document.querySelector('[data-testid="activity-top-50"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-top-50"]')?.getAttribute("aria-pressed") === "true"`, "the 50-row limit")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-overlay"] [data-testid="activity-row"]').length === 2`, "the reloaded full-screen ledger", 15_000)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-overlay"]') === null`, "the inline ledger")
    await cdp.evaluate(`document.querySelector('[data-testid="activity-cut-exec_time"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-cut-exec_time"]')?.getAttribute("aria-pressed") === "true"`, "the restored default cut")
    await cdp.evaluate(`document.querySelector('[data-testid="activity-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-toggle"]')?.getAttribute("aria-expanded") === "false"`, "the ledger collapsed again")
    assert.equal(requests.filter(({ path }) => path.includes("/snapshot")).length, statementSnapshotsBeforeActivity)
    assert.equal(requests.filter(({ path, query }) => path.includes("/snapshot") && new URLSearchParams(query).get("first_match") === "1").length, 0)

    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Plans")).click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-plans-table"] .entity-row').length === 1`, "the Plans table behind Activity", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="activity-pg_store_plans"] [data-testid="activity-toggle"]')?.getAttribute("aria-expanded") === "false"`, "the collapsed Plan Activity ledger")
    const planSnapshotsBeforeActivity = requests.filter(({ path }) => path.includes("/snapshot")).length
    const planHeatmapsBeforeActivity = requests.filter(({ path, query }) => path === "/api/heatmap" && new URLSearchParams(query).get("section") === "pg_store_plans").length
    await cdp.evaluate(`document.querySelector('[data-testid="activity-pg_store_plans"] [data-testid="activity-toggle"]').click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="activity-pg_store_plans"] [data-testid="activity-row"]').length === 2`, "the ranked Plan Activity ledger", 15_000)
    const planLabels = await cdp.evaluate(`[...document.querySelectorAll('[data-testid="activity-pg_store_plans"] [data-testid="activity-row"] > span[title]')].map((row) => row.getAttribute("title"))`)
    assert.deepEqual(planLabels, [VADV_TEXT_PLAN.replace(/\s+/g, " ").trim().slice(0, 240), "Plan ID 78"])
    assert.equal(requests.filter(({ path }) => path.includes("/snapshot")).length, planSnapshotsBeforeActivity)
    assert.equal(requests.filter(({ path, query }) => path === "/api/heatmap" && new URLSearchParams(query).get("section") === "pg_store_plans").length, planHeatmapsBeforeActivity + 1)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the Russian Plan Activity previews")
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="activity-pg_store_plans"] [data-testid="activity-row"] > span[title]')].map((row) => row.getAttribute("title"))`), planLabels)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "en"`, "the English Plan Activity previews")
    assert.equal(requests.filter(({ path }) => path.includes("/snapshot")).length, planSnapshotsBeforeActivity)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Statements")).click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length >= 1`, "the Statements table after Plan Activity")
    const browserMode = await cdp.evaluate(`(() => ({
      at: new URL(location.href).searchParams.get("at"),
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent ?? "",
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent ?? "",
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth || document.querySelector(".topbar").scrollWidth > document.querySelector(".topbar").clientWidth,
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      zone: document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value"),
      zoneLabel: document.querySelector('[data-testid="timezone-value"]')?.textContent ?? "",
      zoneSelectors: document.querySelectorAll('[data-testid="timezone-select"]').length,
    }))()`)
    assert.equal(browserMode.at, String(AT))
    assert.equal(browserMode.zone, "browser")
    assert.equal(browserMode.zoneLabel, "Browser time")
    assert.equal(browserMode.zoneSelectors, 1)
    assert.match(browserMode.cursor, /08:30:00/)
    assert.match(browserMode.hour, /08:00–09:00/)
    assert.match(browserMode.status, /08:30:00/)
    assert.doesNotMatch(browserMode.status, /\b\d{2}[./]\d{2}[./]2026\b/)
    for (const output of [browserMode.cursor, browserMode.hour, browserMode.status]) {
      assert.doesNotMatch(output, /GMT|UTC|\.\d{3}(?!\d)/)
    }
    assert.equal(browserMode.overflow, false)

    const apiBeforeSwitch = requests.filter(({ path }) => path.startsWith("/api/")).length
    await switchZone(cdp, "utc")
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value") === "utc" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("05:30:00") === true`, "the UTC display mode")
    const utcMode = await cdp.evaluate(`(() => ({
      at: new URL(location.href).searchParams.get("at"),
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent ?? "",
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent ?? "",
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      zone: document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value"),
      zoneLabel: document.querySelector('[data-testid="timezone-value"]')?.textContent ?? "",
    }))()`)
    assert.equal(utcMode.at, String(AT))
    assert.equal(utcMode.zone, "utc")
    assert.equal(utcMode.zoneLabel, "UTC")
    assert.match(utcMode.cursor, /05:30:00/)
    assert.match(utcMode.hour, /05:00–06:00/)
    assert.match(utcMode.status, /05:30:00/)
    assert.doesNotMatch(utcMode.status, /\b\d{2}[./]\d{2}[./]2026\b/)
    for (const output of [utcMode.cursor, utcMode.hour, utcMode.status]) {
      assert.doesNotMatch(output, /GMT|UTC|\.\d{3}(?!\d)/)
    }
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/")).length, apiBeforeSwitch)
    await switchZone(cdp, "browser")
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-value"]')?.textContent === "Browser time" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("08:30:00") === true`, "the Browser display restore")
    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value") === "browser" && document.querySelector('[data-testid="timezone-value"]')?.textContent === "Browser time" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("08:30:00") === true`, "the persisted Browser display", 15_000)

    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-tab"]')?.getAttribute("aria-current") === "page"`, "the Processes history destination")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("view")`), null)
    await cdp.evaluate(`(() => {
      const navigator = document.querySelector('[data-testid="hour-timeline"] input.chart-navigator')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(navigator, "2")
      navigator.dispatchEvent(new Event("input", { bubbles: true }))
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "the exact replacement cursor")
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}" && new URL(location.href).searchParams.get("view") === "pg.statements"`, "the exact back selection")
    await cdp.evaluate(`history.forward()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}" && new URL(location.href).searchParams.get("view") === null`, "the exact forward selection")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="timezone-select"]')?.getAttribute("data-value")`), "browser")
    assert.equal(await cdp.evaluate(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`), true)
    assert.deepEqual(result.errors, [])
    assert.deepEqual(result.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test.skip("legacy fullscreen uPlot is replaced by the shared Inspector", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const startFinding = {
    record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
    field_ordinal: 11, row_ordinal: "998", ts: String(HOUR + 1),
  }
  const nearEndFinding = {
    record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
    field_ordinal: 11, row_ordinal: "997", ts: String(HOUR + Math.floor(HOUR_US * 0.895)),
  }
  const endFinding = {
    record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
    field_ordinal: 11, row_ordinal: "999", ts: String(HOUR + HOUR_US - 1),
  }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, [
      ...timelineRecords().filter(({ record }) => record !== "finding"),
      startFinding,
      nearEndFinding,
      endFinding,
    ])
    if (url.pathname.startsWith("/api/")) return ndjson(response, [])
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("expanded-chart browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    const viewports = [
      { height: 768, label: "desktop", mobile: false, width: 1366 },
      { height: 768, label: "iPad landscape", mobile: true, width: 1194 },
      { height: 844, label: "phone", mobile: true, width: 390 },
    ]
    for (const viewport of viewports) {
      await cdp.send("Emulation.setDeviceMetricsOverride", {
        deviceScaleFactor: 1,
        height: viewport.height,
        mobile: viewport.mobile,
        width: viewport.width,
      })
      await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}` })
      await cdp.waitFor(`document.querySelectorAll('[data-testid="hour-timeline"] .marker-button').length >= 2`, `${viewport.label} timeline markers`, 15_000)
      await settleLayout(cdp)
      const before = await cdp.evaluate(`(() => {
        document.documentElement.style.overflow = "visible"
        document.body.style.overflow = "auto"
        document.body.style.minHeight = "200vh"
        window.scrollTo(0, 120)
        return { body: document.body.style.overflow, root: document.documentElement.style.overflow, scrollY }
      })()`)
      assert.equal(before.body, "auto", viewport.label)
      assert.equal(before.root, "visible", viewport.label)
      assert.ok(Math.abs(before.scrollY - 120) <= 1, `${viewport.label}: ${JSON.stringify(before)}`)
      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] .chart-expand').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"][role="dialog"].uplot-expanded') !== null`, `${viewport.label} expanded timeline`)
      await settleLayout(cdp)
      const geometry = await cdp.evaluate(`(() => {
        const dialog = document.querySelector('[data-testid="hour-timeline"][role="dialog"]')
        const header = dialog.querySelector("figcaption")
        const title = header.querySelector(".chart-series-labels")
        const current = header.querySelector(".chart-current")
        const control = header.querySelector(".chart-expand")
        const markers = [...dialog.querySelectorAll('[data-testid="chart-marker-track"] .marker-button')]
        const lastMarker = markers.at(-1)
        const bounds = (node) => {
          const rect = node.getBoundingClientRect()
          return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width }
        }
        const intersects = (left, right) => Math.max(left.left, right.left) < Math.min(left.right, right.right)
          && Math.max(left.top, right.top) < Math.min(left.bottom, right.bottom)
        const headerRect = bounds(header)
        const titleRect = bounds(title)
        const currentRect = bounds(current)
        const controlRect = bounds(control)
        const dialogRect = bounds(dialog)
        const markerRect = bounds(lastMarker)
        const markerRects = markers.map(bounds)
        const markersOverlap = markerRects.some((left, index) => markerRects.slice(index + 1).some((right) => intersects(left, right)))
        const hit = [[markerRect.left + 2, (markerRect.top + markerRect.bottom) / 2],
          [(markerRect.left + markerRect.right) / 2, (markerRect.top + markerRect.bottom) / 2],
          [markerRect.right - 2, (markerRect.top + markerRect.bottom) / 2]].every(([x, y]) =>
          document.elementsFromPoint(x, y).some((node) => node === lastMarker || lastMarker.contains(node)))
        return {
          actionCount: dialog.querySelectorAll(".chart-expand, .chart-close").length,
          activeIsControl: document.activeElement === control,
          bodyOverflow: getComputedStyle(document.body).overflow,
          control: controlRect,
          current: currentRect,
          currentControlOverlap: intersects(currentRect, controlRect),
          dialog: dialogRect,
          header: headerRect,
          hit,
          horizontalOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
          lastMarker: markerRect,
          lastMarkerCount: Number(lastMarker.dataset.markerCount),
          markerCount: markers.reduce((count, marker) => count + Number(marker.dataset.markerCount), 0),
          markerControlOverlap: intersects(markerRect, controlRect),
          markersOverlap,
          rootAnchor: getComputedStyle(document.documentElement).overflowAnchor,
          rootOverflow: getComputedStyle(document.documentElement).overflow,
          scrollY,
          title: titleRect,
          titleControlOverlap: intersects(titleRect, controlRect),
          titleCurrentOverlap: intersects(titleRect, currentRect),
          viewport: { height: innerHeight, width: innerWidth },
        }
      })()`)
      assert.equal(geometry.actionCount, 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.activeIsControl, true, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.rootAnchor, "none", `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.rootOverflow, "hidden", `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.bodyOverflow, "hidden", `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.scrollY, before.scrollY, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.horizontalOverflow, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.currentControlOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.titleControlOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.titleCurrentOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.markerControlOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.markersOverlap, false, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.hit, true, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.lastMarkerCount, 2, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.equal(geometry.markerCount, 3, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.control.width >= 44 && geometry.control.height >= 44, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.title.width > 0 && geometry.current.width > 0, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.title.left >= geometry.header.left - 1 && geometry.title.right <= geometry.header.right + 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.current.left >= geometry.header.left - 1 && geometry.current.right <= geometry.header.right + 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.control.left >= geometry.header.left - 1 && geometry.control.right <= geometry.header.right + 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.lastMarker.top >= geometry.header.bottom - 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.lastMarker.right <= geometry.control.left - 8, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.lastMarker.left >= geometry.dialog.left && geometry.lastMarker.right <= geometry.dialog.right, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(Math.abs(geometry.dialog.left) <= 1 && Math.abs(geometry.dialog.top) <= 1, `${viewport.label}: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.dialog.width >= geometry.viewport.width - 1 && geometry.dialog.height >= geometry.viewport.height - 1, `${viewport.label}: ${JSON.stringify(geometry)}`)

      await cdp.send("Input.dispatchMouseEvent", { type: "mouseWheel", x: Math.floor(viewport.width / 2), y: Math.floor(viewport.height / 2), deltaX: 0, deltaY: 500 })
      await delay(120)
      assert.equal(await cdp.evaluate("scrollY"), before.scrollY, viewport.label)
      await cdp.evaluate(`(() => {
        const dialog = document.querySelector('[data-testid="hour-timeline"][role="dialog"]')
        const navigator = dialog.querySelector("input.chart-navigator")
        navigator.focus()
        navigator.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Tab" }))
      })()`)
      assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-series-labels .help-dot')`), true, viewport.label)
      await cdp.evaluate(`(() => {
        const first = document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-series-labels .help-dot')
        first.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Tab", shiftKey: true }))
      })()`)
      assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-timeline"][role="dialog"] input.chart-navigator')`), true, viewport.label)
      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-expand').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"][role="dialog"]') === null`, `${viewport.label} close action`)
      await cdp.waitFor(`document.documentElement.style.overflow === "visible" && document.body.style.overflow === "auto"`, `${viewport.label} scroll restore`)
      await cdp.waitFor(`document.activeElement === document.querySelector('[data-testid="hour-timeline"] .chart-expand')`, `${viewport.label} close focus return`)
      await settleLayout(cdp)
      assert.equal(await cdp.evaluate("scrollY"), before.scrollY, viewport.label)
      assert.equal(await cdp.evaluate(`getComputedStyle(document.documentElement).overflowAnchor`), "none", viewport.label)

      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] .chart-expand').click()`)
      await cdp.waitFor(`(() => {
        const control = document.querySelector('[data-testid="hour-timeline"][role="dialog"] .chart-expand')
        return control !== null && document.activeElement === control
          && document.documentElement.style.overflow === "hidden" && document.body.style.overflow === "hidden"
      })()`, `${viewport.label} Escape setup`)
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"][role="dialog"]') === null`, `${viewport.label} Escape close`)
      await cdp.waitFor(`document.activeElement === document.querySelector('[data-testid="hour-timeline"] .chart-expand')`, `${viewport.label} Escape focus return`)
      await settleLayout(cdp)
      const restored = await cdp.evaluate(`({ body: document.body.style.overflow, root: document.documentElement.style.overflow, scrollY })`)
      assert.equal(restored.body, before.body, viewport.label)
      assert.equal(restored.root, before.root, viewport.label)
      assert.ok(Math.abs(restored.scrollY - before.scrollY) <= 1, `${viewport.label}: ${JSON.stringify({ before, restored })}`)
      assert.equal(await cdp.evaluate(`getComputedStyle(document.documentElement).overflowAnchor`), "none", viewport.label)
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("Statements scope widens for an explicit search and returns to the workload", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const hour = Number(url.searchParams.get("from") ?? HOUR)
      if (url.searchParams.get("section") === "postgresql_summary") return ndjson(response, postgresSummaryRecords(hour))
      return ndjson(response, timelineRecords(hour))
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      if (!url.searchParams.getAll("section").includes("pg_stat_statements")) return ndjson(response, snapshotRecords())
      // The server answers the scope it was asked for: the workload page hides the collector's row and counts it.
      const workload = url.searchParams.get("scope") === "workload"
      return ndjson(response, statementRecords(true, workload ? 1 : 2, false, workload ? 1 : 2).map((record) => record.record === "snapshot_page"
        ? { ...record, excluded: workload ? "1" : "0" }
        : record.record !== "row" || record.ordinal === "91" ? record : {
          ...record,
          values: record.values.map((stored, index) => index === 7 ? "/* kronika:1.0.0 statements */ select monitor" : stored),
        }))
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("statement scope server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.statements` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length === 1`, "the workload Statements page", 15_000)
    const scopeState = () => cdp.evaluate(`(() => ({
      checked: document.querySelector('[data-testid="statement-monitor-scope"] input')?.checked,
      locked: document.querySelector('[data-testid="statement-monitor-scope"] input')?.disabled,
      forced: document.querySelector('[data-testid="statement-monitor-scope"]')?.dataset.forced ?? null,
      count: document.querySelector('[data-testid="statement-monitor-count"]')?.textContent,
      rows: document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length,
      ledger: document.querySelector('[data-testid="activity-pg_stat_statements"]') !== null,
    }))()`)
    const statementScopes = () => requests
      .filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && query.includes("pg_stat_statements"))
      .map(({ query }) => new URLSearchParams(query).get("scope"))
    assert.deepEqual(await scopeState(), { checked: false, locked: false, forced: null, count: "hidden: 1", rows: 1, ledger: true })
    assert.deepEqual(statementScopes(), ["workload"])

    // An explicit search must be able to reach the collector's own statements, so it widens the scope and locks the control.
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "monitor")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "monitor", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "monitor" && document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length === 2`, "the widened search page", 15_000)
    assert.deepEqual(await scopeState(), { checked: true, locked: true, forced: "true", count: "shown", rows: 2, ledger: true })
    assert.equal(statementScopes().at(-1), null)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="statement-monitor-scope"]')?.getAttribute("title")`), "Shown: the filter or the opened row needs them")

    // Clearing the search returns the reader's own choice: the workload scope with its exact hidden count.
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "deleteContentBackward" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null && document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row').length === 1`, "the restored workload page", 15_000)
    assert.deepEqual(await scopeState(), { checked: false, locked: false, forced: null, count: "hidden: 1", rows: 1, ledger: true })
    assert.equal(statementScopes().at(-1), "workload")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the production artifact preserves wire keys and exact finding page state", { timeout: 120_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: false }
  let heldContextPage = null
  let heldSystemPage = null
  let systemPageWasHeld = false
  let relationMode = "single"
  let inlinePlanQueryMode = "ready"
  let contextPageRequested
  let systemPageRequested
  const contextPage = new Promise((resolve) => { contextPageRequested = resolve })
  const systemPage = new Promise((resolve) => { systemPageRequested = resolve })
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") {
      answerSession(request, response, authState)
      return
    }
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) {
      unauthorized(response)
      return
    }
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end(JSON.stringify({ record: "instance_label", database: "artifact_db" }))
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") {
      ndjson(response, [])
      return
    }
    if (url.pathname === "/api/hour") {
      const from = Number(url.searchParams.get("from") ?? HOUR)
      ndjson(response, [...timelineRecords(from), ...networkLaneRecords(from)])
      return
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      if (sections.includes("pg_store_plans")) {
        ndjson(response, planRecords())
      } else if (url.searchParams.has("row_ordinal") && sections.includes("pg_stat_user_indexes")) {
        ndjson(response, exactIndexRecords())
      } else if (url.searchParams.has("row_ordinal")) {
        ndjson(response, statementRecords(false))
      } else if (sections.includes("pg_stat_statements")) {
        const inline = url.searchParams.get("search") === "query_id:42"
          && url.searchParams.get("page_size") === "1"
          && url.searchParams.get("first_match") === "1"
          && !url.searchParams.has("text")
        if (inline) {
          if (inlinePlanQueryMode === "error") {
            response.writeHead(503)
            response.end()
          } else {
            ndjson(response, inlinePlanQueryMode === "empty" ? emptyPlanQueryRecords() : inlinePlanQueryRecords())
          }
          return
        }
        const planNavigation = url.searchParams.get("search") === "database:operators AND role:reporter AND query_id:42"
        if (planNavigation) {
          ndjson(response, planStatementRecords())
          return
        }
        const filtered = ["queryid", "userid", "dbid", "toplevel"].every((field) => url.searchParams.has(`where.${field}`))
        if (filtered && heldContextPage === null) {
          heldContextPage = response
          contextPageRequested()
        } else {
          // The workload scope lists only application statements and counts the collector's own in the trailer.
          const workload = url.searchParams.get("scope") === "workload"
          const records = filtered ? statementRecords(true) : statementRecords(true, 4_807, true, 50)
          ndjson(response, filtered ? records : records.map((record) => record.record === "snapshot_page"
            ? { ...record, excluded: workload ? "1" : "0" }
            : workload || record.record !== "row" || record.ordinal !== "91" ? record : {
              ...record,
              values: record.values.map((stored, index) => index === 7 ? "/* kronika:1.0.0 statements */ select monitor" : stored),
            }))
        }
      } else if (sections.includes("pg_stat_user_tables") || sections.includes("pg_stat_user_indexes")) {
        ndjson(response, relationRecords(url, relationMode))
      } else if (sections.includes("os_cpu")) {
        if (systemPageWasHeld) {
          ndjson(response, systemSnapshotRecords())
        } else {
          systemPageWasHeld = true
          heldSystemPage = response
          systemPageRequested()
        }
      } else if (sections.includes("pg_stat_activity")) {
        ndjson(response, snapshotRecords())
      } else {
        ndjson(response, [])
      }
      return
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("artifact test server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    const errors = []
    const external = []
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data)
      if (message.method === "Runtime.exceptionThrown") {
        errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text)
      }
      if (message.method === "Runtime.consoleAPICalled" && ["assert", "error"].includes(message.params.type)) {
        errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "))
      }
      if (message.method === "Log.entryAdded" && message.params.entry.level === "error") {
        if (!expectedUnauthorizedLog(message.params.entry.text)) errors.push(message.params.entry.text)
      }
      if (message.method === "Network.loadingFailed"
        && message.params.canceled !== true
        && message.params.errorText !== "net::ERR_ABORTED") {
        errors.push(message.params.errorText)
      }
      if (message.method === "Network.responseReceived" && message.params.response.status >= 400) {
        const response = message.params.response
        const url = new URL(response.url)
        if (!(response.status === 401 && url.origin === origin && url.pathname === "/auth/session")) {
          errors.push(`${response.status}:${response.url}`)
        }
      }
      if (message.method === "Network.requestWillBeSent") {
        const requested = message.params.request.url
        if (/^https?:/.test(requested) && new URL(requested).origin !== origin) external.push(requested)
      }
    })
    await Promise.all([
      cdp.send("Page.enable"),
      cdp.send("Runtime.enable"),
      cdp.send("Network.enable"),
      cdp.send("Log.enable"),
    ])
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.send("Emulation.setTimezoneOverride", { timezoneId: "America/New_York" })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "login form")
    await cdp.evaluate(`(() => {
      const set = (name, value) => {
        const input = document.querySelector('[name="' + name + '"]')
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, value)
        input.dispatchEvent(new Event("input", { bubbles: true }))
      }
      set("username", "artifact")
      set("password", "wire")
      document.querySelector("form").requestSubmit()
    })()`)
    await cdp.waitFor(
      `document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`,
      "the snapshot-backed Activity row",
      15_000,
    )
    const rendered = await cdp.evaluate(`(() => ({
      row: document.querySelector('[data-testid="pg-activity-table"] .entity-row').textContent,
      missing: document.querySelector('[data-testid="cursor-behind"]')?.textContent ?? null,
    }))()`)
    assert.match(rendered.row, /4242/)
    assert.match(rendered.row, /select artifact_wire_contract/)
    assert.equal(rendered.missing, null)
    await assertCompactTimelineContained(cdp, ".workspace > .pg-tabs", "PostgreSQL")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    assert.ok(requests.some(({ path }) => path === `/api/segments/${SEGMENT}/snapshot`))
    const firstApi = requests.findIndex(({ path }) => path.startsWith("/api/"))
    const login = requests.findIndex(({ method, path }) => method === "POST" && path === "/auth/session")
    assert.ok(login > requests.findIndex(({ method, path }) => method === "GET" && path === "/auth/session"))
    assert.ok(firstApi > login, JSON.stringify(requests.slice(0, firstApi + 1), null, 2))
    assert.deepEqual(
      requests.filter(({ authorization }) => authorization !== null).map(({ authorization, method, path }) => ({ authorization, method, path })),
      [{ authorization: "Basic YXJ0aWZhY3Q6d2lyZQ==", method: "POST", path: "/auth/session" }],
    )
    assert.ok(requests.filter(({ path }) => path.startsWith("/api/")).every(({ authorization, cookie, marker }) => (
      authorization === null && cookie === SESSION_COOKIE && marker === "1"
    )))
    const localClocks = await cdp.evaluate(`(() => ({
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent,
      cursorSecondary: document.querySelector('[data-testid="cursor-time"] small')?.textContent ?? null,
      hour: document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent,
      hourContext: document.querySelector('[data-testid="hour-picker-trigger"] small')?.textContent,
      sample: document.querySelector('.cursor-time')?.textContent.includes('Sample'),
    }))()`)
    assert.match(localClocks.cursor, /01:30:00/)
    assert.doesNotMatch(localClocks.cursor, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.equal(localClocks.cursorSecondary, null)
    assert.equal(localClocks.hour, "01:00–02:00")
    assert.match(localClocks.hourContext, /08\/13\/2026/)
    assert.doesNotMatch(localClocks.hourContext, /GMT|UTC/)
    assert.equal(localClocks.sample, false)

    const initialTheme = await cdp.evaluate(`document.documentElement.dataset.theme`)
    const alternateTheme = initialTheme === "dark" ? "light" : "dark"
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`document.documentElement.dataset.theme === ${JSON.stringify(alternateTheme)}`, "the alternate theme")

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("August") === true`, "the August calendar")
    const initialPicker = await cdp.evaluate(`(() => ({
      currentAction: document.querySelector('[data-testid="hour-current"]')?.tagName,
      currentDay: document.querySelector('[data-testid="day-cell"][data-day="2026-08-13"]')?.getAttribute('aria-pressed'),
      currentHour: document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR}"]')?.getAttribute('aria-pressed'),
      boundaryDayDisabled: document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.disabled,
      boundaryDayVisible: document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.getBoundingClientRect().height > 0,
      headerToggle: document.querySelector('[data-testid="hour-popover"] > header button') !== null,
      hourCount: document.querySelectorAll('[data-testid="hour-cell"]').length,
      popovers: document.querySelectorAll('[data-testid="hour-popover"]').length,
      separateControls: document.querySelector('[data-testid="hour-popover"]').querySelectorAll('input, select').length,
      unavailableDay: document.querySelector('[data-testid="day-cell"][data-day="2026-08-12"]')?.disabled,
    }))()`)
    assert.deepEqual(initialPicker, {
      currentAction: "STRONG",
      currentDay: "true",
      currentHour: "true",
      boundaryDayDisabled: false,
      boundaryDayVisible: true,
      headerToggle: false,
      hourCount: 2,
      popovers: 1,
      separateControls: 0,
      unavailableDay: true,
    })
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const size = await cdp.evaluate(`(() => {
        const popover = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
        const calendar = document.querySelector('[data-testid="day-picker"]').getBoundingClientRect()
        const hours = document.querySelector('[data-testid="hour-grid"]').getBoundingClientRect()
        const boundaryDay = document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]').getBoundingClientRect()
        return {
          calendar: { bottom: calendar.bottom, left: calendar.left, right: calendar.right, top: calendar.top },
          clientHeight: document.documentElement.clientHeight,
          clientWidth: document.documentElement.clientWidth,
          boundaryDay: { bottom: boundaryDay.bottom, left: boundaryDay.left, right: boundaryDay.right, top: boundaryDay.top },
          hours: { bottom: hours.bottom, left: hours.left, right: hours.right, top: hours.top },
          popover: { bottom: popover.bottom, left: popover.left, right: popover.right, top: popover.top },
          scrollWidth: document.documentElement.scrollWidth,
        }
      })()`)
      assert.ok(size.scrollWidth <= size.clientWidth, `${width}px picker overflow: ${JSON.stringify(size)}`)
      assert.ok(size.popover.left >= 0 && size.popover.right <= size.clientWidth, `${width}px horizontal picker bounds: ${JSON.stringify(size)}`)
      assert.ok(size.popover.top >= 0 && size.popover.bottom <= size.clientHeight, `${width}px vertical picker bounds: ${JSON.stringify(size)}`)
      assert.ok(size.boundaryDay.left >= size.calendar.left && size.boundaryDay.right <= size.calendar.right, `${width}px boundary-day visibility: ${JSON.stringify(size)}`)
      assert.ok(size.calendar.right <= size.hours.left, `${width}px picker columns: ${JSON.stringify(size)}`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 480 })
    await settleLayout(cdp)
    const narrow = await cdp.evaluate(`(() => {
      const calendar = document.querySelector('[data-testid="day-picker"]').getBoundingClientRect()
      const hours = document.querySelector('[data-testid="hour-grid"]').getBoundingClientRect()
      const popover = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
      return { calendarBottom: calendar.bottom, clientWidth: document.documentElement.clientWidth, hoursTop: hours.top, popoverLeft: popover.left, popoverRight: popover.right, scrollWidth: document.documentElement.scrollWidth }
    })()`)
    assert.ok(narrow.calendarBottom <= narrow.hoursTop, `narrow picker stack: ${JSON.stringify(narrow)}`)
    assert.ok(narrow.popoverLeft >= 0 && narrow.popoverRight <= narrow.clientWidth && narrow.scrollWidth <= narrow.clientWidth, `narrow picker bounds: ${JSON.stringify(narrow)}`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR}"]').dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowRight' }))`)
    assert.equal(await cdp.evaluate(`document.activeElement?.dataset.instant`), String(HOUR + HOUR_US))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "picker Escape close")
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`document.documentElement.dataset.theme === ${JSON.stringify(initialTheme)}`, "the initial theme")

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.getBoundingClientRect().height > 0`, "boundary day immediately visible")
    await cdp.evaluate(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-09"]')?.getAttribute('aria-pressed') === "true"`, "the local August 9 hours")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="hour-cell"]').length`), 1)
    await cdp.evaluate(`document.querySelector('[data-testid="hour-cell"][data-instant="${AUGUST_HOUR}"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "23:00–00:00"`, "the exact local boundary hour")
    await cdp.waitFor(`Math.floor(Number(new URLSearchParams(location.search).get("at")) / ${HOUR_US}) * ${HOUR_US} === ${AUGUST_HOUR}`, "the exact boundary address")
    assert.equal(await cdp.evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)
    const augustRequest = requests.find(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("from") === String(AUGUST_HOUR))
    assert.notEqual(augustRequest, undefined)
    assert.equal(new URLSearchParams(augustRequest.query).get("to"), String(AUGUST_HOUR + HOUR_US - 1))

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.evaluate(`document.querySelector('[data-testid="day-cell"][data-day="2026-08-13"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR + HOUR_US}"]') !== null`, "the recorded August 13 hour")
    await cdp.evaluate(`document.querySelector('[data-testid="hour-cell"][data-instant="${HOUR + HOUR_US}"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "02:00–03:00"`, "the recorded local August 13 selection")
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "23:00–00:00"`, "the previous catalogued instant")
    await cdp.waitFor(`Math.floor(Number(new URLSearchParams(location.search).get("at")) / ${HOUR_US}) * ${HOUR_US} === ${AUGUST_HOUR}`, "the previous catalogued address")
    const originalAddress = await cdp.evaluate(`(() => { const url = new URL(location.href); url.searchParams.set("at", "${AT}"); return url.href })()`)
    await cdp.send("Page.navigate", { url: originalAddress })
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong')?.textContent === "01:00–02:00"`, "the original exact selection", 15_000)

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[aria-label="Next month"]') !== null`, "the reopened month controls")
    await cdp.evaluate(`document.querySelector('[aria-label="Next month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("December 2026") === true`, "the next catalog month")
    await cdp.evaluate(`document.querySelector('[aria-label="Next month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("January 2027") === true`, "the next year month")
    await cdp.evaluate(`document.querySelector('[aria-label="Previous month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("December 2026") === true`, "the previous year month")
    await cdp.evaluate(`document.querySelector('[aria-label="Previous month"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.includes("August 2026") === true`, "the selected month restored")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))`)

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click(); document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="picker-month"]')?.textContent.toLocaleLowerCase("ru").includes("август") === true`, "the Russian calendar month")
    const russianPicker = await cdp.evaluate(`(() => ({
      context: document.querySelector('[data-testid="hour-popover"] > header > span')?.textContent ?? null,
      day: document.querySelector('[data-testid="day-cell"][data-day="2026-08-13"]')?.getAttribute('aria-label'),
      text: document.querySelector('[data-testid="hour-popover"]')?.textContent ?? "",
      zoneLabel: ${ZONE_LABEL} ?? "",
    }))()`)
    assert.equal(russianPicker.context, null)
    assert.match(russianPicker.day, /13\.08\.2026/)
    assert.doesNotMatch(russianPicker.text, /GMT|UTC/)
    assert.equal(russianPicker.zoneLabel, "Время браузера")
    assert.equal(await cdp.evaluate(`document.querySelector('.cursor-time')?.textContent.includes('Отсчёт')`), false)
    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'mouse' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "picker outside close")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await switchZone(cdp, "utc")
    await cdp.waitFor(`document.documentElement.lang === "ru" && ${ZONE_VALUE} === "utc" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("05:30:00")`, "the UTC re-render")
    const utcClocks = await cdp.evaluate(`(() => ({
      cursor: document.querySelector('[data-testid="cursor-time"]')?.textContent,
      cursorSecondary: document.querySelector('[data-testid="cursor-time"] small') !== null,
      hour: document.querySelector('[data-testid="hour-picker-trigger"]')?.textContent,
      hourZoneSuffix: document.querySelector('[data-testid="hour-picker-trigger"] small')?.textContent.includes('UTC') ?? false,
      zoneLabel: ${ZONE_LABEL} ?? "",
    }))()`)
    assert.equal(utcClocks.zoneLabel, "UTC")
    assert.match(utcClocks.cursor, /05:30:00/)
    assert.match(utcClocks.hour, /05:00–06:00/)
    assert.doesNotMatch(utcClocks.cursor, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.doesNotMatch(utcClocks.hour, /GMT|UTC|\.\d{3}(?!\d)/)
    assert.equal(utcClocks.cursorSecondary, false)
    assert.equal(utcClocks.hourZoneSuffix, false)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await switchZone(cdp, "browser")
    await cdp.waitFor(`document.documentElement.lang === "en" && ${ZONE_VALUE} === "browser" && ${ZONE_LABEL} === "Browser time" && document.querySelector('[data-testid="cursor-time"]')?.textContent.includes("01:30:00")`, "the local-time restore")

    await cdp.evaluate(`([...document.querySelectorAll(".pg-tabs button")].find((button) => button.textContent === "Tables")).click()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row').length === 1`, "the relation wire row")
    const postgresJoinGap = await cdp.evaluate(`(() => {
      const timeline = document.querySelector('.timeline-shell').getBoundingClientRect()
      const controls = document.querySelector('.workspace > .pg-tabs').getBoundingClientRect()
      return controls.top - timeline.bottom
    })()`)
    assert.ok(postgresJoinGap <= 1, `PostgreSQL major-region gap ${postgresJoinGap}px`)
    const relationRow = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-row').textContent`)
    assert.match(relationRow, /artifact_db/)
    assert.match(relationRow, /artifact_table/)
    const tablePresentation = await cdp.evaluate(`(() => ({
      cells: [...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row [role="cell"]')].map((cell) => cell.textContent),
      headers: [...document.querySelectorAll('[data-testid="pg-tables-table"] [role="columnheader"]')].map((header) => header.textContent),
    }))()`)
    assert.equal(tablePresentation.cells.includes("42"), false)
    assert.equal(tablePresentation.cells.includes("73"), false)
    assert.doesNotMatch(tablePresentation.headers.join(" "), /Database ID|Table OID|Index OID/)
    const relationRequest = requests.find(({ query }) => query.includes("section=pg_stat_user_tables") && query.includes("group=object"))
    assert.notEqual(relationRequest, undefined, JSON.stringify(requests.map(({ query }) => query), null, 2))
    const relationQuery = new URLSearchParams(relationRequest.query)
    assert.equal(relationQuery.get("group"), "object")
    assert.equal(relationQuery.get("page_size"), "200")
    assert.equal(relationQuery.getAll("field").includes("datid"), true)
    assert.equal(relationQuery.getAll("field").includes("relid"), true)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') !== null`, "the table detail")
    const tableDetail = await cdp.evaluate(`(() => {
      const detail = document.querySelector('[data-testid="pg-relation-detail"]')
      const rows = [...detail.querySelectorAll('dl > .detail-row')]
      return {
        compactRows: rows.length,
        maxRowHeight: Math.max(...rows.map((row) => row.getBoundingClientRect().height)),
        aligned: rows.every((row) => { const term = row.querySelector('dt').getBoundingClientRect(); const value = row.querySelector('dd').getBoundingClientRect(); return value.left > term.left && Math.abs(value.top - term.top) <= 2 }),
        labels: [...detail.querySelectorAll('dt')].map((label) => label.textContent),
        values: [...detail.querySelectorAll('dd')].map((value) => value.textContent),
      }
    })()`)
    assert.ok(tableDetail.compactRows > 0 && tableDetail.maxRowHeight <= 35 && tableDetail.aligned, JSON.stringify(tableDetail))
    assert.doesNotMatch(tableDetail.labels.join(" "), /Database ID|Table OID|Index OID/)
    assert.equal(tableDetail.values.includes("42"), false)
    assert.equal(tableDetail.values.includes("73"), false)
    await cdp.evaluate(`document.querySelector(".inspector-close").click()`)

    relationMode = "long"
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "Size and buffers")).click()`)
    await cdp.waitFor(`(() => { const node = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); return node !== null && node.scrollWidth > node.clientWidth })()`, "the wide size and buffers table")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="virtual-body"]')?.style.height === "4800px"`, "the long virtual relation table")
    const estimate = await cdp.evaluate(`(() => {
      const node = [...document.querySelectorAll('[data-testid="pg-tables-table"] [title]')].find((cell) => cell.title.includes('9,007,199,254,740,993'))
      return node === undefined ? null : { label: node.getAttribute('aria-label'), text: node.textContent, title: node.title }
    })()`)
    assert.deepEqual(estimate, { label: "≈9,007,199,254,740,993 rows", text: "≈9.01E15 rows", title: "≈9,007,199,254,740,993 rows" })
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768], [1024, 1366]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const initial = await cdp.evaluate(`(() => {
        const table = document.querySelector('[data-testid="pg-tables-table"]')
        const body = table.querySelector('.entity-scroll')
        const layout = document.querySelector('.pg-entity-layout')
        const paging = document.querySelector('[data-testid="table-paging"]')
        const sticky = table.querySelector('.entity-header-cell.entity-sticky')
        const bodyRect = body.getBoundingClientRect()
        const contentBottom = Math.max(layout.getBoundingClientRect().bottom, paging?.getBoundingClientRect().bottom ?? 0)
        const firstRow = table.querySelector('.entity-row')
        return {
          bodyClientWidth: body.clientWidth,
          bodyHeight: body.clientHeight,
          bodyScrollWidth: body.scrollWidth,
          clientWidth: document.documentElement.clientWidth,
          clientHeight: document.documentElement.clientHeight,
          contentBottom,
          documentScrollWidth: document.documentElement.scrollWidth,
          railBottom: bodyRect.bottom,
          railHeight: body.offsetHeight - body.clientHeight,
          railTabIndex: body.tabIndex,
          railVisible: bodyRect.bottom <= document.documentElement.clientHeight + .5,
          rowHeight: firstRow?.getBoundingClientRect().height ?? 0,
          stickyLeft: sticky.getBoundingClientRect().left,
          tableLeft: bodyRect.left,
          virtualHeight: table.querySelector('[data-testid="virtual-body"]').getBoundingClientRect().height,
          visibleRows: [...table.querySelectorAll('.entity-row')].filter((row) => {
            const rect = row.getBoundingClientRect()
            return rect.bottom > bodyRect.top && rect.top < bodyRect.bottom
          }).length,
        }
      })()`)
      assert.ok(initial.bodyScrollWidth > initial.bodyClientWidth, `${width}px wide table: ${JSON.stringify(initial)}`)
      // The virtualizer fills the scroll body: as many rows as fit, no gap.
      assert.ok(initial.visibleRows >= Math.floor(initial.bodyHeight / initial.rowHeight) - 1, `${width}x${height} visible rows: ${JSON.stringify(initial)}`)
      assert.equal(initial.virtualHeight, 4800, `${width}x${height} virtual height`)
      assert.ok(initial.contentBottom <= initial.clientHeight && initial.clientHeight - initial.contentBottom <= 24, `${width}x${height} remaining viewport: ${JSON.stringify(initial)}`)
      assert.equal(initial.railTabIndex, 0, `${width}px focusable horizontal rail`)
      assert.ok(initial.railHeight > 0 && initial.railVisible, `${width}px visible horizontal rail: ${JSON.stringify(initial)}`)
      assert.ok(Math.abs(initial.stickyLeft - initial.tableLeft) <= 1, `${width}px initial sticky identity: ${JSON.stringify(initial)}`)
      assert.ok(initial.documentScrollWidth <= initial.clientWidth, `${width}px relation document overflow: ${JSON.stringify(initial)}`)

      await cdp.evaluate(`(() => { const body = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); body.focus(); body.scrollLeft = body.scrollWidth })()`)
      await cdp.waitFor(`(() => {
        const body = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll')
        return body.scrollLeft >= body.scrollWidth - body.clientWidth - 1
      })()`, `${width}px rightmost table column`)
      const end = await cdp.evaluate(`(() => {
        const table = document.querySelector('[data-testid="pg-tables-table"]')
        const body = table.querySelector('.entity-scroll')
        const cells = table.querySelectorAll('.entity-header-cell')
        const first = cells[0].getBoundingClientRect()
        const last = cells[cells.length - 1].getBoundingClientRect()
        const viewport = body.getBoundingClientRect()
        return { activeRail: document.activeElement === body, firstLeft: first.left, lastRight: last.right, viewportLeft: viewport.left, viewportRight: viewport.left + body.clientWidth }
      })()`)
      assert.equal(end.activeRail, true, `${width}px focused horizontal rail`)
      assert.ok(Math.abs(end.firstLeft - end.viewportLeft) <= 1, `${width}px sticky identity after horizontal scroll: ${JSON.stringify(end)}`)
      const endGutter = end.viewportRight - end.lastRight
      assert.ok(endGutter >= 7 && endGutter <= 9, `${width}px stable rightmost-column gutter: ${JSON.stringify({ ...end, endGutter })}`)
      await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').scrollLeft = 0`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`(() => { const body = document.querySelector('[data-testid="pg-tables-table"] .entity-scroll'); body.scrollTop = body.scrollHeight })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="virtual-body"]')?.style.height === "4920px"`, "the one guarded relation cursor page")
    const cursorPages = requests.filter(({ query }) => new URLSearchParams(query).get("cursor") === "viewport-page-two")
    assert.equal(cursorPages.length, 1, JSON.stringify(cursorPages))
    await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').scrollTop = 0`)
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row')].some((row) => row.getAttribute('aria-label') === 'artifact_db.public.artifact_table')`, "the first virtual relation row")

    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-tables-table"] .entity-row')].find((row) => row.getAttribute('aria-label') === 'artifact_db.public.artifact_table')).click()`)
    const alignedDetail = await cdp.evaluate(`(() => {
      const inspector = document.querySelector('[data-testid="inspector"]').getBoundingClientRect()
      const body = document.querySelector('.inspector-body').getBoundingClientRect()
      const detail = document.querySelector('[data-testid="pg-relation-detail"]').getBoundingClientRect()
      return { bodyBottom: body.bottom, bodyTop: body.top, detailBottom: detail.bottom, detailTop: detail.top, inspectorBottom: inspector.bottom, inspectorTop: inspector.top, viewport: innerHeight }
    })()`)
    assert.ok(alignedDetail.inspectorTop >= -1 && alignedDetail.inspectorBottom <= alignedDetail.viewport + 1
      && alignedDetail.detailTop >= alignedDetail.bodyTop - 1 && alignedDetail.detailBottom <= alignedDetail.bodyBottom + 1,
    JSON.stringify(alignedDetail))
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "ru"`, "the Russian estimate labels")
    const russianEstimate = await cdp.evaluate(`(() => {
      const nodes = [...document.querySelectorAll('[data-testid="pg-relation-detail"] [title]')]
      const exact = nodes.find((node) => node.title.includes('9 007 199 254 740 993'))
      const toast = nodes.find((node) => node.title.includes('713 456'))
      const labels = [...document.querySelectorAll('[data-testid="pg-relation-detail"] dt')].map((node) => node.textContent.trim())
      return { exact: exact?.title ?? null, labels, toast: toast?.textContent ?? null, toastExact: toast?.title ?? null }
    })()`)
    assert.equal(russianEstimate.exact, "≈9 007 199 254 740 993 строки")
    assert.equal(russianEstimate.toast, "≈713 тыс. строк")
    assert.equal(russianEstimate.toastExact, "≈713 456 строк")
    assert.equal(russianEstimate.labels.filter((label) => /buffer|blks/i.test(label)).some((label) => /[А-Яа-яЁё]/u.test(label)), false)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === "en"`, "the English estimate restore")
    relationMode = "single"
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-link"]') !== null`, "the table-to-index link")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-link"]').click()`)
    await cdp.waitFor(`location.search.includes("view=pg.indexes") && location.search.includes("datid=42") && location.search.includes("relid=73")`, "numeric table identity in index navigation")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("artifact_index") === true`, "the linked index row")
    const linkedScope = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] [data-testid="table-status"]').textContent`)
    assert.match(linkedScope, /artifact_db · artifact_table/)
    assert.doesNotMatch(linkedScope, /\b(?:42|73)\b|\bOID\b/)

    const indexRow = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    assert.match(indexRow, /artifact_db/)
    assert.match(indexRow, /public/)
    assert.match(indexRow, /artifact_table/)
    assert.match(indexRow, /artifact_index/)
    const indexPresentation = await cdp.evaluate(`(() => ({
      cells: [...document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row [role="cell"]')].map((cell) => cell.textContent),
      headers: [...document.querySelectorAll('[data-testid="pg-indexes-table"] [role="columnheader"]')].map((header) => header.textContent),
    }))()`)
    for (const oid of ["42", "73", "74"]) assert.equal(indexPresentation.cells.includes(oid), false)
    assert.doesNotMatch(indexPresentation.headers.join(" "), /Database ID|Table OID|Index OID/)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-exact-indexdef"]')?.textContent.includes("CREATE UNIQUE INDEX artifact_index") === true`, "the exact index definition")
    assert.ok(requests.some(({ query }) => query.includes("row_ordinal=8") && !query.includes("text=")))
    const indexDetail = await cdp.evaluate(`(() => ({
      compactRows: document.querySelectorAll('[data-testid="pg-relation-detail"] dl > .detail-row').length,
      html: document.querySelector('[data-testid="pg-relation-detail"]')?.outerHTML ?? null,
      labels: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dt')].map((label) => label.textContent),
      values: [...document.querySelectorAll('[data-testid="pg-relation-detail"] dd')].map((value) => value.textContent),
    }))()`)
    assert.ok(indexDetail.compactRows > 0, indexDetail.html)
    assert.doesNotMatch(indexDetail.labels.join(" "), /Database ID|Table OID|Index OID/)
    for (const oid of ["42", "73", "74"]) assert.equal(indexDetail.values.includes(oid), false)
    await cdp.evaluate(`document.querySelector(".inspector-close").click()`)
    relationMode = "short"
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "State")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] [data-testid="virtual-body"]')?.style.height === "72px"`, "the short relation set")
    const shortTable = await cdp.evaluate(`(() => {
      const body = document.querySelector('[data-testid="pg-indexes-table"] .entity-scroll')
      const bounds = body.getBoundingClientRect()
      const header = document.querySelector('[data-testid="pg-indexes-table"] .entity-head').getBoundingClientRect()
      return {
        axis: body.dataset.scrollAxis,
        bottom: bounds.bottom,
        clientHeight: body.clientHeight,
        height: bounds.height,
        overflowY: getComputedStyle(body).overflowY,
        railHeight: body.offsetHeight - body.clientHeight,
        rows: document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length,
        scrollHeight: body.scrollHeight,
        verticalOwner: body.scrollHeight > body.clientHeight + 1,
        viewportHeight: document.documentElement.clientHeight,
        header: header.height,
        virtual: document.querySelector('[data-testid="pg-indexes-table"] [data-testid="virtual-body"]').getBoundingClientRect().height,
      }
    })()`)
    assert.equal(shortTable.rows, 3)
    assert.equal(shortTable.virtual, 72)
    assert.equal(shortTable.axis, "horizontal")
    assert.equal(shortTable.overflowY, "hidden")
    assert.equal(shortTable.verticalOwner, false)
    assert.ok(shortTable.scrollHeight <= shortTable.clientHeight + 1, JSON.stringify(shortTable))
    assert.ok(Math.abs(shortTable.height - shortTable.header - shortTable.virtual - shortTable.railHeight) <= 1, JSON.stringify(shortTable))
    assert.ok(shortTable.bottom <= shortTable.viewportHeight + 1, JSON.stringify(shortTable))

    const beforeOidSearch = requests.length
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "74")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await delay(700)
    const oidSearchRequest = requests.slice(beforeOidSearch).find(({ query }) => query.includes("section=pg_stat_user_indexes") && query.includes("search=74"))
    assert.notEqual(oidSearchRequest, undefined, JSON.stringify(requests.slice(beforeOidSearch), null, 2))
    const oidSearchQuery = new URLSearchParams(oidSearchRequest.query)
    assert.equal(oidSearchQuery.get("where.datid"), "42")
    assert.equal(oidSearchQuery.get("where.relid"), "73")
    for (const field of ["datid", "relid", "indexrelid"]) assert.equal(oidSearchQuery.getAll("field").includes(field), true, field)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await delay(400)

    const clickRelation = async (label) => {
      await cdp.evaluate(`([...document.querySelectorAll(".workspace .lensbar button")].find((button) => button.textContent === ${JSON.stringify(label)})).click()`)
    }
    await clickRelation("Schemas")
    await cdp.waitFor(`location.search.includes("level=schema") && document.querySelector('[data-testid="pg-indexes-table"] .entity-row') !== null`, "schema level")
    await clickRelation("Databases")
    await cdp.waitFor(`location.search.includes("level=database") && document.querySelector('[data-testid="pg-indexes-table"] .entity-row') !== null`, "database level")
    const databasePresentation = await cdp.evaluate(`(() => ({
      cells: [...document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row [role="cell"]')].map((cell) => cell.textContent),
      headers: [...document.querySelectorAll('[data-testid="pg-indexes-table"] [role="columnheader"]')].map((header) => header.textContent),
    }))()`)
    assert.equal(databasePresentation.cells.includes("42"), false)
    assert.doesNotMatch(databasePresentation.headers.join(" "), /Database ID|Table OID|Index OID/)
    await cdp.evaluate("history.back()")
    await cdp.waitFor(`location.search.includes("level=schema")`, "schema level restored by browser back")
    await clickRelation("Databases")
    await cdp.waitFor(`location.search.includes("level=database")`, "database rollup restored")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-drill"]') !== null`, "explicit rollup drill action")
    assert.ok(await cdp.evaluate(`document.querySelectorAll('[data-testid="pg-relation-detail"] dl > .detail-row').length > 0`))
    await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-drill"]').click()`)
    await cdp.waitFor(`location.search.includes("level=schema") && location.search.includes("datid=42")`, "database-scoped schema drill")
    await cdp.evaluate(`([...document.querySelectorAll(".workspace .lensbar button")].find((button) => button.textContent === "All")).click()`)
    await cdp.waitFor(`!location.search.includes("level=") && !location.search.includes("datid=")`, "reset to all index objects")

    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "State")).click()`)
    await clickRelation("Databases")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("363") === true`, "categorical index counts")
    const englishCounts = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    assert.doesNotMatch(englishCounts, /(?:363|223|111|0)\/s/)
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "Usage")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("3/s") === true`, "exact English rate unit")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("3/с") === true`, "exact Russian rate unit")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="pg-relation-lenses"] button')].find((button) => button.textContent === "State")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row')?.textContent.includes("363") === true`, "Russian categorical counts")
    const russianCounts = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    assert.doesNotMatch(russianCounts, /(?:363|223|111|0)\/с/)
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const size = await cdp.evaluate(`({ clientWidth: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth })`)
      assert.ok(size.scrollWidth <= size.clientWidth, `${width}px relation overflow: ${JSON.stringify(size)}`)
    }
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click(); document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.querySelector(".system-main") !== null`, "the host workspace")
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await systemPage
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]') !== null`, "the crowded Russian loading bar")
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const size = await cdp.evaluate(`(() => {
        const clientWidth = document.documentElement.clientWidth
        const dataTime = document.querySelector('[data-testid="cursor-time"]').getBoundingClientRect()
        const spinner = document.querySelector('[data-testid="cursor-behind"] .loading-ring').getBoundingClientRect()
        const overflow = [...document.querySelectorAll("body *")].flatMap((node) => {
          const rect = node.getBoundingClientRect()
          return rect.right > clientWidth + 0.5 ? [{ className: node.className, right: rect.right, tag: node.tagName }] : []
        }).slice(0, 8)
        return { clientWidth, overflow, scrollWidth: document.documentElement.scrollWidth, statusGap: spinner.left - dataTime.right }
      })()`)
      assert.ok(size.scrollWidth <= size.clientWidth, `${width}px document overflow: ${JSON.stringify(size)}`)
      assert.ok(size.statusGap >= 15, `${width}px cursor status grouping: ${JSON.stringify(size)}`)
    }
    ndjson(heldSystemPage, systemSnapshotRecords())
    heldSystemPage = null
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)

    await cdp.evaluate(`([...document.querySelectorAll(".source-tabs button")].find((button) => button.textContent === "Events")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-mark"] button') !== null`, "the statement finding")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 360 })
    await settleLayout(cdp)
    // Below 520px the form lives in a panel; the search itself is unchanged.
    assert.equal(await cdp.evaluate(`getComputedStyle(document.querySelector('[data-testid="events-console"] input[type="search"]').closest('form')).display`), "none", "the phone folds the form into a button")
    await cdp.evaluate(`document.querySelector('[data-testid="mobile-search-open"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="mobile-search-head"]') !== null`, "the phone search panel")
    await settleLayout(cdp)
    const eventSearch = await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="events-console"] input[type="search"]')
      const label = input.closest('label')
      const icon = label.querySelector('svg')
      const box = (node) => { const value = node.getBoundingClientRect(); return { left: value.left, right: value.right, width: value.width } }
      return { client: document.documentElement.clientWidth, icon: box(icon), input: box(input), label: box(label), nav: [...document.querySelectorAll('.source-tabs button')].map((button) => button.textContent), placeholder: input.placeholder, scroll: document.documentElement.scrollWidth }
    })()`)
    assert.deepEqual(eventSearch.nav, ["Host", "Processes", "PostgreSQL", "Events"])
    assert.equal(eventSearch.placeholder, "Текст или field:value AND size>100MB")
    assert.ok(eventSearch.icon.right <= eventSearch.input.left + 1, JSON.stringify(eventSearch))
    assert.ok(eventSearch.label.left >= -1 && eventSearch.label.right <= eventSearch.client + 1 && eventSearch.scroll <= eventSearch.client, JSON.stringify(eventSearch))
    await cdp.evaluate(`document.querySelector('[data-testid="events-console"] [aria-label="Синтаксис и поля поиска"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"] [role="dialog"]') !== null`, "the narrow RU search help")
    const searchHelp = await cdp.evaluate(`(() => {
      const overlay = document.querySelector('[data-testid="search-help"]')
      const dialog = overlay.querySelector('[role="dialog"]')
      const bounds = dialog.getBoundingClientRect()
      return {
        active: document.activeElement?.getAttribute("aria-label") ?? "",
        ariaModal: dialog.getAttribute("aria-modal"),
        fields: dialog.textContent,
        left: bounds.left, right: bounds.right,
        viewport: document.documentElement.clientWidth,
        scroll: document.documentElement.scrollWidth,
      }
    })()`)
    assert.equal(searchHelp.ariaModal, "true")
    assert.match(searchHelp.active, /Закрыть/)
    assert.match(searchHelp.fields, /kind/)
    assert.match(searchHelp.fields, /source/)
    assert.match(searchHelp.fields, /category/)
    assert.doesNotMatch(searchHelp.fields, /queryid_stat_statements|planid|relname/)
    assert.ok(searchHelp.left >= -1 && searchHelp.right <= searchHelp.viewport + 1 && searchHelp.scroll <= searchHelp.viewport, JSON.stringify(searchHelp))
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "search help closed by Escape")
    assert.equal(await cdp.evaluate(`document.activeElement?.getAttribute("aria-label")`), "Синтаксис и поля поиска")
    await assertSearchControlContained(cdp, "Events search")
    await cdp.evaluate(`document.querySelector('[data-testid="mobile-search-head"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="mobile-search-head"]') === null`, "the phone search panel closed")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[data-testid="event-mark"] > button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-mark-crossing"]') !== null`, "the grouped crossings of one mark")
    await cdp.evaluate(`document.querySelector('[data-testid="event-mark-crossing"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="entity-context-filter"]') !== null`, "the exact statement context")
    await contextPage
    const preview = await cdp.evaluate(`(() => ({
      activity: document.querySelector('[data-testid="activity-pg_stat_statements"]') !== null,
      checkbox: document.querySelector('[data-testid="statement-monitor-scope"] input')?.checked,
      chip: document.querySelector('[data-testid="entity-context-filter"]')?.textContent ?? "",
      detail: document.querySelector(".pg-detail") !== null,
      row: document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent ?? "",
      search: document.querySelector('[data-testid="table-filter"]')?.getAttribute("aria-label") ?? "",
      locked: document.querySelector('[data-testid="statement-monitor-scope"] input')?.disabled,
      forced: document.querySelector('[data-testid="statement-monitor-scope"]')?.dataset.forced,
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
    }))()`)
    // An entity context widens the scope to every statement: the control is checked and locked, the ledger stays.
    assert.equal(preview.activity, true)
    assert.equal(preview.checkbox, true)
    assert.equal(preview.locked, true)
    assert.equal(preview.forced, "true")
    assert.match(preview.chip, /Query ID 9007199254740991 · operators · reporterShow all/)
    assert.doesNotMatch(preview.chip, /queryid=|userid=|dbid=|toplevel=/)
    assert.match(preview.row, /select artifact_exact_context/)
    assert.match(preview.search, /Search rows/)
    assert.match(preview.status, /Loading rows/)
    assert.match(preview.status, /Previous rows remain visible/)
    assert.doesNotMatch(preview.status, /Loaded 0 of 0/)
    assert.equal(preview.detail, false)

    ndjson(heldContextPage, statementRecords(true))
    heldContextPage = null
    await cdp.waitFor(
      `document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent.includes("Loaded 1 of 1") === true`,
      "the settled identity-filtered page count",
    )
    const identityRequest = requests.find(({ query }) => query.includes("where.queryid=") && query.includes("page_size=200"))
    assert.notEqual(identityRequest, undefined, JSON.stringify(requests.map(({ query }) => query), null, 2))
    const identityQuery = new URLSearchParams(identityRequest.query)
    assert.equal(identityQuery.get("where.queryid"), "9007199254740991")
    assert.equal(identityQuery.get("where.userid"), "10")
    assert.equal(identityQuery.get("where.dbid"), "20")
    assert.equal(identityQuery.get("where.toplevel"), "true")
    assert.equal(identityQuery.get("type_id"), "1002003")
    assert.equal(identityQuery.get("page_size"), "200")
    const exactRequest = requests.find(({ query }) => query.includes("row_ordinal=91"))
    assert.notEqual(exactRequest, undefined)
    assert.equal(new URLSearchParams(exactRequest.query).has("page_size"), false)
    await delay(600)
    assert.equal(requests.filter(({ query }) => query.includes("where.queryid=") && query.includes("page_size=200")).length, 1)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector(".pg-detail") !== null`, "detail after explicit row selection")
    await cdp.evaluate(`document.querySelector(".inspector-close").click(); document.querySelector('[data-testid="entity-context-filter"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent.includes("Loaded 50 of 4,807") === true`, "the paged full statement set")
    await cdp.waitFor(`document.querySelector('[data-testid="table-paging"]') !== null`, "active statement paging")
    const pagedScope = await cdp.evaluate(`(() => ({
      activity: document.querySelector('[data-testid="activity-pg_stat_statements"]') !== null,
      checked: document.querySelector('[data-testid="statement-monitor-scope"] input')?.checked,
      count: document.querySelector('[data-testid="statement-monitor-count"]')?.textContent,
      paging: document.querySelector('[data-testid="table-paging"] button')?.disabled,
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      virtualHeight: document.querySelector('[data-testid="pg-statements-table"] [data-testid="virtual-body"]')?.style.height,
    }))()`)
    // Without a context the default workload scope returns; the page holds 50 application rows and reports one hidden.
    assert.deepEqual(pagedScope, {
      activity: true, checked: false, count: "hidden: 1", paging: false,
      status: pagedScope.status, virtualHeight: "1200px",
    })
    assert.match(pagedScope.status, /Loaded 50 of 4,807/)

    const invalidSearchStart = requests.length
    const lastValidRows = await cdp.evaluate(`(() => ({
      first: document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent ?? "",
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
    }))()`)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "query_id:9007199254740991 AND taname:orders")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "query_id:9007199254740991 AND taname:orders", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-error"] mark')?.textContent === "taname"`, "the exact invalid selector span")
    const invalidSearch = await cdp.evaluate(`(() => ({
      error: document.querySelector('[data-testid="search-error"]')?.textContent ?? "",
      first: document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent ?? "",
      invalid: document.querySelector('[data-testid="table-filter"]')?.getAttribute("aria-invalid"),
      status: document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent ?? "",
      url: new URL(location.href).searchParams.get("find"),
    }))()`)
    assert.match(invalidSearch.error, /Unknown field “taname”/)
    assert.equal(invalidSearch.invalid, "true")
    assert.deepEqual({ first: invalidSearch.first, status: invalidSearch.status }, lastValidRows)
    assert.equal(invalidSearch.url, null)
    assert.equal(requests.slice(invalidSearchStart).some(({ query }) => query.includes("taname")), false)

    const quantitativeSearch = "exec_time_rate>500ms/s AND call_rate>1/s"
    await cdp.evaluate('(() => { const input = document.querySelector("[data-testid=table-filter]"); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "exec_time_rate>500ms/s AND call_rate>1/s"); input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertFromPaste" })); input.form.requestSubmit() })()')
    await cdp.waitFor('new URL(location.href).searchParams.get("find") === "exec_time_rate>500ms/s AND call_rate>1/s" && document.querySelectorAll("[data-testid=search-chips] button").length === 2', "quantitative statement chips")
    assert.deepEqual(await cdp.evaluate(`(() => ({
      activity: document.querySelector('[data-testid="activity-pg_stat_statements"]') !== null,
      checkbox: document.querySelector('[data-testid="statement-monitor-scope"] input')?.checked,
      forced: document.querySelector('[data-testid="statement-monitor-scope"]')?.dataset.forced,
    }))()`), { activity: true, checkbox: true, forced: "true" })
    const quantitativeState = await cdp.evaluate('(() => ({ aria: document.querySelector("[data-testid=search-chips]")?.getAttribute("aria-label") ?? "", fields: [...document.querySelectorAll("[data-testid=search-chips] strong")].map((field) => field.textContent), text: document.querySelector("[data-testid=search-chips]")?.textContent ?? "" }))()')
    assert.deepEqual(quantitativeState.fields, ["Execution time/s", "Calls/s"])
    assert.match(quantitativeState.text, /> 500 ms\/sANDCalls\/s · > 1 \/s/)
    assert.match(quantitativeState.aria, /exec_time_rate>500ms\/s AND call_rate>1\/s/)
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === quantitativeSearch))

    const unitErrorStart = requests.length
    await cdp.evaluate('(() => { const input = document.querySelector("[data-testid=table-filter]"); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "exec_time_rate>500ms"); input.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertFromPaste" })); input.form.requestSubmit() })()')
    await cdp.waitFor('document.querySelector("[data-testid=search-error]")?.textContent.includes("not valid for this field") === true', "duration-rate unit error")
    assert.equal(await cdp.evaluate('new URL(location.href).searchParams.get("find")'), quantitativeSearch)
    assert.equal(requests.slice(unitErrorStart).some(({ query }) => new URLSearchParams(query).get("search") === "exec_time_rate>500ms"), false)

    for (const locale of ["en", "ru"]) {
      await cdp.evaluate('document.querySelector("[data-testid=locale-' + locale + ']").click()')
      await cdp.waitFor('document.documentElement.lang === "' + locale + '"', locale + " quantitative help locale")
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
        const helpLabel = locale === "en" ? "Search syntax and fields" : "Синтаксис и поля поиска"
        await cdp.waitFor(`document.querySelector('[aria-label="${helpLabel}"]') !== null`, locale + " quantitative help trigger at " + width)
        await cdp.evaluate(`document.querySelector('[aria-label="${helpLabel}"]').click()`)
        await cdp.waitFor('document.querySelector("[data-testid=search-help]") !== null', locale + " quantitative help at " + width)
        const help = await cdp.evaluate('(() => { const dialog = document.querySelector("[data-testid=search-help] [role=dialog]"); const bounds = dialog.getBoundingClientRect(); return { fields: dialog.textContent, left: bounds.left, right: bounds.right, scroll: document.documentElement.scrollWidth, viewport: document.documentElement.clientWidth } })()')
        assert.match(help.fields, /exec_time_rate/)
        assert.match(help.fields, /wal_rate/)
        assert.doesNotMatch(help.fields, /cpu_cores|rmem_kb|total_exec_time|shared_blks_read/)
        assert.ok(help.left >= -1 && help.right <= help.viewport + 1 && help.scroll <= help.viewport, locale + " " + width + ": " + JSON.stringify(help))
        await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
        await cdp.waitFor('document.querySelector("[data-testid=search-help]") === null', locale + " quantitative help close at " + width)
      }
    }
    await cdp.evaluate('document.querySelector("[data-testid=locale-en]").click()')
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })

    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "(query_id:9007199254740991 or db:operators) and role:reporter")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "(query_id:9007199254740991 or db:operators) and role:reporter", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "(query_id:9007199254740991 OR database:operators) AND role:reporter" && document.querySelectorAll('[data-testid="search-chips"] button').length === 3`, "canonical pasted boolean search chips")
    const canonicalSearchState = await cdp.evaluate(`(() => ({
      accessible: document.querySelector('[data-testid="search-chips"]').getAttribute("aria-label"),
      input: document.querySelector('[data-testid="table-filter"]').value,
      labels: [...document.querySelectorAll('[data-testid="search-chips"] button')].map((button) => button.getAttribute("aria-label")),
      text: document.querySelector('[data-testid="search-chips"]').textContent,
    }))()`)
    assert.equal(canonicalSearchState.input, "(query_id:9007199254740991 OR database:operators) AND role:reporter")
    assert.match(canonicalSearchState.accessible, /Applied search filters: \(query_id:9007199254740991 OR database:operators\) AND role:reporter/)
    assert.match(canonicalSearchState.text, /\(query_id: 9007199254740991ORdatabase: operators\)ANDrole: reporter/)
    assert.deepEqual(canonicalSearchState.labels, [
      "Remove query_id: 9007199254740991",
      "Remove database: operators",
      "Remove role: reporter",
    ])
    await cdp.evaluate(`(() => {
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text) => { window.__copiedSearch = text } } })
      document.querySelector('[aria-label="Search syntax and fields"]').click()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') !== null`, "boolean search help")
    const copiedExample = await cdp.evaluate(`(() => {
      const button = [...document.querySelectorAll('[data-testid="search-help"] button')].find((candidate) => candidate.querySelector("code") !== null)
      button.click()
      return button.textContent
    })()`)
    assert.equal(await cdp.evaluate(`window.__copiedSearch ?? ""`), copiedExample)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "boolean search help closed")
    await cdp.evaluate(`document.querySelector('[data-testid="search-chips"] button').focus()`)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: " ", code: "Space", windowsVirtualKeyCode: 32 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: " ", code: "Space", windowsVirtualKeyCode: 32 })
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter"`, "keyboard chip removal")
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "(query_id:9007199254740991 OR database:operators) AND role:reporter" && document.querySelectorAll('[data-testid="search-chips"] button').length === 3`, "Back restores the canonical boolean search")
    await cdp.evaluate(`history.forward()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter" && document.querySelectorAll('[data-testid="search-chips"] button').length === 2`, "Forward restores the removed boolean chip")
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null && document.querySelector('[data-testid="search-chips"]') === null`, "clear restores ordinary rows")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]')?.textContent.includes("Loaded 50 of 4,807") === true`, "the search-clear page")

    await cdp.evaluate(`(() => {
      const row = [...document.querySelectorAll('[data-testid="pg-statements-table"] .entity-row')]
        .find((candidate) => candidate.querySelector('[data-field="query"]')?.textContent.includes("select artifact_page_1"))
      if (!(row instanceof HTMLElement)) throw new Error("workload Statement row is missing")
      row.click()
    })()`)
    await cdp.waitFor(`document.querySelector(".pg-detail") !== null`, "detail beside active paging")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-statement-related-plans"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans" && new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter AND query_id:9007199254740990"`, "Statement opens every matching Plan through public search")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "database:operators AND role:reporter AND query_id:9007199254740990"))
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.statements" && document.querySelector(".pg-detail") !== null`, "Back restores Statement detail")
    await assertDetailRowsDoNotOverlap(cdp, "Statement exact ID detail")
    for (const [width, height] of [[1920, 1080], [1366, 768], [1024, 768]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
      const placement = await cdp.evaluate(`(() => {
        const layout = document.querySelector('[data-testid="pg-entity-layout"]')
        const workspace = document.querySelector('.workspace').getBoundingClientRect()
        const table = layout.querySelector('[data-testid="pg-statements-table"]').getBoundingClientRect()
        const paging = layout.querySelector('[data-testid="table-paging"]').getBoundingClientRect()
        const inspector = document.querySelector('[data-testid="inspector"]').getBoundingClientRect()
        return {
          inspector: { left: inspector.left, top: inspector.top },
          paging: { top: paging.top },
          table: { bottom: table.bottom },
          workspace: { right: workspace.right, top: workspace.top },
        }
      })()`)
      assert.ok(placement.inspector.top <= placement.workspace.top + 1, `${width}px Inspector starts with workspace: ${JSON.stringify(placement)}`)
      assert.ok(placement.inspector.left >= placement.workspace.right - 1, `${width}px Inspector beside workspace: ${JSON.stringify(placement)}`)
      assert.ok(placement.paging.top >= placement.table.bottom - 1, `${width}px paging below table: ${JSON.stringify(placement)}`)
    }

    await cdp.evaluate(`document.querySelector('.inspector-close').click(); ([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Plans'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plans-table"] .entity-row') !== null`, "the Vadv Plans row")
    const planTable = await cdp.evaluate(`(() => ({
      headers: [...document.querySelectorAll('[data-testid="pg-plans-table"] [role="columnheader"]')].map((header) => header.textContent),
      row: document.querySelector('[data-testid="pg-plans-table"] .entity-row').textContent,
    }))()`)
    assert.match(planTable.headers.join(" "), /Plan summary/)
    assert.match(planTable.headers.join(" "), /Related query ID/i)
    assert.doesNotMatch(planTable.headers.join(" "), /(?:^|\s)Query ID(?:\s|$)/)
    assert.match(planTable.row, /Merge Join.*cost=0\.85\.\.81\.42/)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="related-planid"]') !== null`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="related-queryid_stat_statements"]') !== null`), true)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'planning_share>20% AND call_rate>1/s')
      input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste' }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "planning_share>20% AND call_rate>1/s"`, "quantitative Plans search")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "planning_share>20% AND call_rate>1/s"))
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="search-chips"] strong')].map((node) => node.textContent)`), ["Planning share", "Calls/s"])
    await cdp.evaluate(`document.querySelector('[aria-label="Search syntax and fields"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') !== null`, "Plans quantitative help")
    const planHelp = await cdp.evaluate(`document.querySelector('[data-testid="search-help"]').textContent`)
    assert.match(planHelp, /slow_call_rate/)
    assert.doesNotMatch(planHelp, /wal_rate|cpu_cores|shared_blks_read/)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "close Plans quantitative help")
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null`, "clear quantitative Plans search")
    await cdp.evaluate(`document.querySelector('[data-testid="related-planid"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans" && new URL(location.href).searchParams.get("find") === "plan_id:77"`, "Plan ID opens the shared Plans filter")
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans" && new URL(location.href).searchParams.get("find") === null`, "Back restores the unfiltered Plans page")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "ready" && document.querySelector('[data-testid="pg-text-plan"]') !== null`, "the related query and native text execution plan")
    const planDetail = await cdp.evaluate(`(() => ({
      copy: document.querySelector('[data-testid="pg-plan-view"] button')?.textContent ?? null,
      bodyCount: document.querySelectorAll('[data-testid="pg-text-plan"]').length,
      queryBodyCount: document.querySelectorAll('[data-testid="pg-plan-query-text"]').length,
      queryCopy: [...document.querySelectorAll('[data-testid="pg-plan-query-view"] button')].map((button) => button.textContent),
      queryText: [...document.querySelectorAll('[data-testid="pg-plan-query-text"] pre')].map((node) => node.textContent),
      secondaryDisclosure: document.querySelector('[data-testid="pg-plan-view"] details') !== null,
      text: document.querySelector('[data-testid="pg-text-plan"]').textContent,
      queryBeforePlan: document.querySelector('[data-testid="pg-plan-query-view"]').getBoundingClientRect().top < document.querySelector('[data-testid="pg-plan-view"]').getBoundingClientRect().top,
    }))()`)
    assert.equal(planDetail.text, VADV_TEXT_PLAN)
    assert.equal(planDetail.copy, "Copy")
    assert.equal(planDetail.bodyCount, 1)
    assert.equal(planDetail.queryBodyCount, 1)
    assert.deepEqual(planDetail.queryCopy, ["Copy"])
    assert.deepEqual(planDetail.queryText, [INLINE_QUERY_PRIMARY])
    assert.equal(planDetail.queryBeforePlan, true)
    assert.equal(planDetail.secondaryDisclosure, false)
    await cdp.evaluate(`Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText(value) { globalThis.__copiedPlanQuery = value; return Promise.resolve() } } })`)
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plan-query-view"] button').click()`)
    await cdp.waitFor(`globalThis.__copiedPlanQuery === ${JSON.stringify(INLINE_QUERY_PRIMARY)}`, "the exact recorded query copy")

    for (const locale of ["en", "ru"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `plan detail ${locale} locale`)
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: width === 360 ? 640 : 800, mobile: false, width })
        await settleLayout(cdp)
        const textLayout = await cdp.evaluate(`(() => {
          const query = document.querySelector('[data-testid="pg-plan-query-view"]')
          const list = document.querySelector('[data-testid="pg-plan-query-list"]')
          const plan = document.querySelector('[data-testid="pg-plan-view"]')
          const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top } }
          return {
            copy: [...query.querySelectorAll('button')].map((button) => button.textContent),
            labels: [query.querySelector('header strong')?.textContent, plan.querySelector('header strong')?.textContent],
            listClientHeight: list.clientHeight,
            listScrollHeight: list.scrollHeight,
            order: bounds(query).top < bounds(plan).top,
            overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
            query: bounds(query),
            plan: bounds(plan),
            whiteSpace: getComputedStyle(query.querySelector('pre')).whiteSpace,
          }
        })()`)
        assert.deepEqual(textLayout.labels, ["Query", "Execution plan"], `${locale} ${width}px labels`)
        assert.deepEqual(textLayout.copy, ["Copy"], `${locale} ${width}px copy actions`)
        assert.equal(textLayout.order, true, `${locale} ${width}px text block order`)
        assert.equal(textLayout.whiteSpace, "pre-wrap", `${locale} ${width}px whitespace`)
        assert.ok(textLayout.listClientHeight <= 320 && textLayout.listScrollHeight > textLayout.listClientHeight, `${locale} ${width}px independent query scroll: ${JSON.stringify(textLayout)}`)
        assert.ok(textLayout.query.left >= -1 && textLayout.query.right <= width + 1 && textLayout.plan.left >= -1 && textLayout.plan.right <= width + 1, `${locale} ${width}px block bounds: ${JSON.stringify(textLayout)}`)
        assert.equal(textLayout.overflow, false, `${locale} ${width}px document overflow`)
      }
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })

    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    const expectedQueryFailureAt = errors.length
    inlinePlanQueryMode = "error"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "error"`, "the related query network failure")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-text-plan"]')?.textContent`), VADV_TEXT_PLAN)
    inlinePlanQueryMode = "ready"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plan-query-error"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "ready"`, "the related query retry")
    const expectedQueryFailures = errors.splice(expectedQueryFailureAt)
    assert.equal(expectedQueryFailures.some((message) => message.startsWith("503:") && message.includes("/snapshot?") && message.includes("query_id%3A42")), true)
    assert.equal(expectedQueryFailures.every((message) => message.startsWith("503:") || message.includes("Failed to load resource: the server responded with a status of 503")), true)

    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    inlinePlanQueryMode = "empty"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "unavailable"`, "the honest related query unavailable state")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-text-plan"]')?.textContent`), VADV_TEXT_PLAN)
    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    inlinePlanQueryMode = "ready"
    await cdp.evaluate(`document.querySelector('[data-testid="pg-plans-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-plan-query-view"]')?.dataset.queryStatus === "ready"`, "the restored related query")
    await cdp.evaluate(`([...document.querySelectorAll('.pg-detail-head button')].find((button) => button.textContent.includes('Related statements'))).click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.statements" && new URL(location.href).searchParams.get("find") === "database:operators AND role:reporter AND query_id:42"`, "the related plan statement route")
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] .entity-row')?.textContent.includes("select from plan_navigation") === true`, "the matched last query")
    const planContext = await cdp.evaluate(`document.querySelector('[data-testid="search-chips"]').textContent`)
    assert.match(planContext, /database: operators/)
    assert.match(planContext, /role: reporter/)
    assert.match(planContext, /query_id: 42/)
    const planQueryRequest = requests.find(({ query }) => {
      const parameters = new URLSearchParams(query)
      return parameters.get("search") === "query_id:42" && parameters.get("first_match") === "1"
    })
    assert.notEqual(planQueryRequest, undefined, JSON.stringify(requests.map(({ query }) => query), null, 2))
    const planQueryParameters = new URLSearchParams(planQueryRequest.query)
    assert.equal(planQueryParameters.has("where.queryid"), false)
    assert.equal(planQueryParameters.has("where.userid"), false)
    assert.equal(planQueryParameters.has("where.dbid"), false)
    assert.equal(planQueryParameters.has("type_id"), false)
    assert.equal(planQueryParameters.has("text"), false)
    assert.equal(planQueryParameters.has("cursor"), false)
    assert.equal(planQueryParameters.get("page_size"), "1")
    assert.equal(planQueryParameters.get("at"), String(AT))
    assert.deepEqual(planQueryParameters.getAll("field"), ["query"])
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(AT))
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.plans"`, "browser back to Plans")

    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Activity'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "Activity rows for related navigation")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-related-statements"]')?.disabled === false`, "Activity related Statements action")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-related-statements"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.statements" && new URL(location.href).searchParams.get("find") === "database:operators AND query_id:991"`, "Activity opens all matching Statements")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "database:operators AND query_id:991"))
    const activityQueryRequest = requests.find(({ query }) => new URLSearchParams(query).get("search") === "database:operators AND query_id:991")
    assert.notEqual(activityQueryRequest, undefined)
    assert.equal(new URLSearchParams(activityQueryRequest.query).has("where.userid"), false)
    assert.equal(new URLSearchParams(activityQueryRequest.query).has("where.toplevel"), false)
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.activity" && document.querySelector('[data-testid="pg-activity-table"]') !== null`, "Back restores Activity")

    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === null && document.querySelector('[data-testid="hour-timeline"]') !== null`, "Processes timeline")
    await assertCompactTimelineContained(cdp, ".workspace > .lensbar", "Processes")

    const hostClick = await cdp.evaluate(`(() => {
      const button = [...document.querySelectorAll(".source-tabs button")].find((candidate) => candidate.textContent === "Host")
      button.click()
      return button.textContent
    })()`)
    assert.equal(hostClick, "Host")
    await cdp.evaluate(`(() => {
      history.pushState({}, "", "/?at=${AT}&view=host.overview")
      dispatchEvent(new PopStateEvent("popstate"))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null && document.querySelector(".system-main") !== null`, "the System Overview")
    await assertCompactTimelineContained(cdp, ".system-main", "Host")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await delay(600)
    const system = await cdp.evaluate(`(() => ({
      chart: document.querySelector('[data-testid^="system-group-chart-"]') !== null,
      metric: new URL(location.href).searchParams.get("metric"),
      source: document.querySelector(".source-active")?.textContent ?? null,
    }))()`)
    assert.equal(system.source, "Host")
    // The ledger opens quiet: no auto-selected metric, no force-opened chart.
    assert.equal(system.chart, false, JSON.stringify(system))
    assert.equal(system.metric, null, JSON.stringify(system))
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the System resource table")
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "the disk ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-expansion-disk"]') !== null`, "the expanded Disk row")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-disk"]')?.getAttribute("aria-expanded")`), "true")
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU cards", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas') !== null`, "the inline CPU chart")
    await assertHoverGeometryStable(cdp, '[data-testid="system-cpu-composition"]', "wide System dock")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 420 })
    await settleLayout(cdp)
    await assertHoverGeometryStable(cdp, '[data-testid="system-cpu-composition"]', "420px System dock")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 667, mobile: true, width: 375 })
    await settleLayout(cdp)
    const ledger = await cdp.evaluate(`(() => {
      const cells = [...document.querySelectorAll('.use-cell')].flatMap((cell) => {
        const label = cell.querySelector('.use-lane > span')
        const value = cell.querySelector('strong')
        if (label === null || value === null) return []
        return [{
          clipped: label.scrollWidth > label.clientWidth || value.scrollWidth > value.clientWidth,
          label: label.textContent,
          value: value.textContent,
        }]
      })
      const network = [...document.querySelectorAll('[data-testid="use-row-network"] .use-lane > span')]
        .map((label) => label.textContent).join(" · ")
      return { cells, network, sideways: document.documentElement.scrollWidth > document.documentElement.clientWidth }
    })()`)
    // A cell that prints two readings names both in its single action.
    assert.equal(ledger.network, "RX · TX", JSON.stringify(ledger))
    assert.deepEqual(ledger.cells.filter((cell) => cell.clipped), [], `phone ledger clips: ${JSON.stringify(ledger.cells)}`)
    assert.equal(ledger.sideways, false, JSON.stringify(ledger))
    for (const [width, height] of [[1920, 1080], [1366, 768], [1280, 431], [1024, 768], [1024, 1366]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      const layout = await cdp.evaluate(`document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => { try {
        window.scrollTo(0, 0)
        const bounds = (node) => {
          const rect = node.getBoundingClientRect()
          return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width }
        }
        const consolePanel = document.querySelector(".system-main")
        const main = document.querySelector(".system-main")
        const history = document.querySelector('[data-testid="system-group-chart-cpu"]')
        const expansion = document.querySelector('[data-testid="use-expansion-cpu"]')
        const chart = history.querySelector(".uplot-figure")
        const canvas = chart.querySelector("canvas")
        const host = chart.querySelector(".uplot-host")
        const plot = chart.querySelector(".u-over")
        const contentBottom = document.querySelector('[data-testid="use-table"]').getBoundingClientRect().bottom
        const panels = [document.querySelector(".timeline-shell"), history]
        const overlaps = []
        for (let left = 0; left < panels.length; left += 1) {
          for (let right = left + 1; right < panels.length; right += 1) {
            const a = panels[left].getBoundingClientRect()
            const b = panels[right].getBoundingClientRect()
            if (Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1
              && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1) {
              overlaps.push([panels[left].className, panels[right].className])
            }
          }
        }
        const historyBounds = bounds(history)
        const chartBounds = bounds(chart)
        resolve({
          chart: chartBounds,
          chartAccess: {
            canvasAriaHidden: canvas.getAttribute("aria-hidden"),
            canvasCount: chart.querySelectorAll(".uplot-host canvas").length,
            hostLabel: host.getAttribute("aria-label"),
            hostRole: host.getAttribute("role"),
            navigator: chart.querySelector('input.chart-navigator[type="range"]') !== null,
            summary: chart.querySelector(".chart-summary")?.textContent ?? "",
          },
          console: bounds(consolePanel),
          contentBottom,
          documentClientWidth: document.documentElement.clientWidth,
          documentScrollWidth: document.documentElement.scrollWidth,
          expansion: bounds(expansion),
          history: historyBounds,
          historyTail: historyBounds.bottom - chartBounds.bottom,
          host: bounds(host),
          main: bounds(main),
          overlaps,
          plot: bounds(plot),
        })
      } catch (error) { resolve({ error: String(error && error.stack || error) }) } }))))`)
      assert.equal(layout.error, undefined, layout.error)
      assert.ok(layout.chart.height >= 300 && layout.chart.height <= 360, `${width}x${height} System chart height: ${JSON.stringify(layout)}`)
      assert.ok(layout.host.height >= 190, `${width}x${height} System chart host height: ${JSON.stringify(layout)}`)
      assert.ok(layout.plot.height >= 145, `${width}x${height} System plot height: ${JSON.stringify(layout)}`)
      assert.ok(layout.history.left >= layout.expansion.left - 1 && layout.history.right <= layout.expansion.right + 1
        && layout.history.top >= layout.expansion.top - 1 && layout.history.bottom <= layout.expansion.bottom + 1,
      `${width}x${height} System expansion containment: ${JSON.stringify(layout)}`)
      assert.deepEqual(layout.chartAccess.canvasAriaHidden, "true")
      assert.equal(layout.chartAccess.canvasCount, 1)
      assert.equal(layout.chartAccess.hostRole, "img")
      assert.match(layout.chartAccess.hostLabel, /CPU used.*cores/)
      assert.equal(layout.chartAccess.navigator, true)
      assert.ok(layout.chartAccess.summary.length > 0)
      assert.ok(layout.history.height - layout.chart.height <= 240 && layout.historyTail >= -1 && layout.historyTail <= 30, `${width}x${height} compact System dock: ${JSON.stringify(layout)}`)
      assert.ok(Math.abs(layout.console.bottom - layout.contentBottom) <= 1.5, `${width}x${height} content-sized System layout: ${JSON.stringify(layout)}`)
      assert.ok(layout.chart.left >= layout.history.left - 1 && layout.chart.right <= layout.history.right + 1
        && layout.chart.top >= layout.history.top - 1 && layout.chart.bottom <= layout.history.bottom + 1,
      `${width}x${height} System chart containment: ${JSON.stringify(layout)}`)
      assert.ok(layout.documentScrollWidth <= layout.documentClientWidth, `${width}x${height} System document overflow: ${JSON.stringify(layout)}`)
      assert.deepEqual(layout.overlaps, [], `${width}x${height} System panel overlaps`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 1080, mobile: false, width: 1920 })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the overview resource table")
    const overviewOrder = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="use-table"]').getBoundingClientRect()
      const inspector = document.querySelector('[data-testid="inspector"]')?.getBoundingClientRect() ?? null
      return { gap: inspector === null ? null : inspector.left - table.right }
    })()`)
    assert.equal(overviewOrder.gap, null, `overview navigation closes the prior Inspector: ${JSON.stringify(overviewOrder)}`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "CPU metrics after Overview")
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas') !== null`, "the inline CPU chart after Overview")
    await cdp.evaluate(`(() => {
      if (window.__kronikaAxisText !== undefined) return
      window.__kronikaAxisText = []
      const original = CanvasRenderingContext2D.prototype.fillText
      CanvasRenderingContext2D.prototype.fillText = function (value, ...args) {
        window.__kronikaAxisText.push(String(value))
        return original.call(this, value, ...args)
      }
    })()`)
    const chartThemes = []
    for (let themeIndex = 0; themeIndex < 2; themeIndex += 1) {
      const theme = await cdp.evaluate(`document.documentElement.dataset.theme`)
      chartThemes.push(theme)
      for (const [width, height] of [[1920, 1080], [1366, 768], [1280, 431], [1024, 768], [390, 844]]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
        const state = await cdp.evaluate(`document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => { try {
          const figure = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-figure')
          const host = figure.querySelector(".uplot-host")
          const canvas = host.querySelector("canvas")
          const plot = host.querySelector(".u-over")
          const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width } }
          resolve({
            backingRatio: canvas.width / canvas.getBoundingClientRect().width,
            canvas: bounds(canvas),
            canvasAriaHidden: canvas.getAttribute("aria-hidden"),
            clientWidth: document.documentElement.clientWidth,
            figure: bounds(figure),
            host: bounds(host),
            hostLabel: host.getAttribute("aria-label"),
            hostRole: host.getAttribute("role"),
            navigatorCount: figure.querySelectorAll('input.chart-navigator[type="range"]').length,
            plot: bounds(plot),
            scrollWidth: document.documentElement.scrollWidth,
            summary: figure.querySelector(".chart-summary")?.textContent ?? "",
          })
        } catch (error) { resolve({ error: String(error && error.stack || error) }) } }))))`)
        assert.equal(state.error, undefined, state.error)
        assert.equal(state.canvasAriaHidden, "true", `${theme} ${width}px canvas accessibility: ${JSON.stringify(state)}`)
        assert.equal(state.hostRole, "img", `${theme} ${width}px chart role: ${JSON.stringify(state)}`)
        assert.match(state.hostLabel, /CPU used.*cores/, `${theme} ${width}px chart unit: ${JSON.stringify(state)}`)
        assert.equal(state.navigatorCount, 1, `${theme} ${width}px sample navigator: ${JSON.stringify(state)}`)
        assert.ok(state.summary.length > 0, `${theme} ${width}px chart summary: ${JSON.stringify(state)}`)
        assert.ok(state.plot.width >= 120, `${theme} ${width}px plot width: ${JSON.stringify(state)}`)
        assert.ok(state.host.height >= 150, `${theme} ${width}px chart host height: ${JSON.stringify(state)}`)
        assert.ok(state.plot.height >= 80, `${theme} ${width}px plot height: ${JSON.stringify(state)}`)
        assert.ok(state.canvas.left >= state.host.left - 1 && state.canvas.right <= state.host.right + 1,
          `${theme} ${width}px canvas bounds: ${JSON.stringify(state)}`)
        assert.ok(state.figure.left >= -1 && state.figure.right <= state.clientWidth + 1,
          `${theme} ${width}px chart viewport: ${JSON.stringify(state)}`)
        assert.ok(state.scrollWidth <= state.clientWidth, `${theme} ${width}px page overflow: ${JSON.stringify(state)}`)
        assert.ok(state.backingRatio >= 0.95 && state.backingRatio <= 1.05,
          `${theme} ${width}px DPR 1 backing store: ${JSON.stringify(state)}`)
      }
      if (themeIndex === 0) {
        await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
        await cdp.waitFor(`document.documentElement.dataset.theme !== ${JSON.stringify(theme)}`, "the second chart theme")
      }
    }
    assert.deepEqual(new Set(chartThemes), new Set(["dark", "light"]))
    const axisText = await cdp.evaluate(`window.__kronikaAxisText`)
    assert.equal(axisText.some((text) => text.includes("Time, browser local")), false, JSON.stringify(axisText))
    const timeAxes = axisText.filter((text) => /^\d{2}:\d{2}$/.test(text))
    assert.equal(timeAxes.length > 0, true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /GMT|UTC|\.\d{3}(?!\d)/.test(text)), false, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => text.includes("%")), true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /^0%?$/.test(text)), true, JSON.stringify(axisText))
    assert.equal(axisText.some((text) => /^100%?$/.test(text)), true, JSON.stringify(axisText))

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 2, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`(() => {
      const canvas = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas')
      return canvas !== null && canvas.width / canvas.getBoundingClientRect().width >= 1.9
    })()`, "the DPR 2 chart backing store")
    const dprTwo = await cdp.evaluate(`(() => {
      const canvas = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas')
      return { ratio: canvas.width / canvas.getBoundingClientRect().width, screen: devicePixelRatio }
    })()`)
    assert.ok(dprTwo.ratio >= 1.9 && dprTwo.ratio <= 2.1, JSON.stringify(dprTwo))
    assert.equal(dprTwo.screen, 2)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.evaluate(`document.querySelector('[aria-label="Theme"]').click()`)
    await cdp.waitFor(`(() => {
      const canvas = document.querySelector('[data-testid="system-group-chart-cpu"] .uplot-host canvas')
      const ratio = canvas?.width / canvas?.getBoundingClientRect().width
      return ratio >= 0.95 && ratio <= 1.05
    })()`, "the restored DPR 1 chart backing store")

    const chartRequestsBeforeInspectorClose = requests.filter(({ path }) => path.startsWith("/api/")).length
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-group-chart-cpu"] .chart-expand, [data-testid="system-group-chart-cpu"] [role="dialog"].uplot-expanded') === null`), true)
    await delay(120)
    assert.equal(requests.filter(({ path }) => path.startsWith("/api/")).length, chartRequestsBeforeInspectorClose)

    await cdp.evaluate(`document.querySelector('[data-testid="use-toggle-cpu"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-expansion-cpu"]') === null`, "the collapsed CPU row")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') === null`), true)

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp`), String(AT))
    const sampleText = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(sampleText, /^01:30:00;/)
    assert.doesNotMatch(sampleText, /GMT|UTC|\.000/)
    assert.match(sampleText, /82/)

    const arrow = async (key, expected) => {
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: ${JSON.stringify(key)} }))`)
      await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${expected}"`, `${key} to ${expected}`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${expected}"`, `${key} recorded sample ${expected}`)
    }
    await arrow("ArrowLeft", BEFORE_AT)
    await arrow("ArrowRight", AT)
    await arrow("ArrowRight", AFTER_AT)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="hour-timeline"] .uplot').length`), 1)

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await switchZone(cdp, "utc")
    await cdp.waitFor(`document.documentElement.lang === "ru" && ${ZONE_VALUE} === "utc" && ${ZONE_LABEL} === "UTC" && document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")?.startsWith("05:30:07;")`, "the chart UTC render")
    const utcSample = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(utcSample, /^05:30:07;/)
    assert.doesNotMatch(utcSample, /GMT|UTC|\.\d{3}(?!\d)/)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await switchZone(cdp, "browser")
    await cdp.waitFor(`document.documentElement.lang === "en" && ${ZONE_VALUE} === "browser" && ${ZONE_LABEL} === "Browser time" && document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")?.startsWith("01:30:07;")`, "the chart local-time restore")

    const navigateTo = async (timestamp) => {
      const target = await cdp.evaluate(`(() => {
        const url = new URL(location.href)
        url.searchParams.set("at", "${timestamp}")
        return url.href
      })()`)
      await cdp.send("Page.navigate", { url: target })
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator') !== null`, `timeline at ${timestamp}`, 15_000)
    }
    await navigateTo(DST_EDT_HOUR + 1_800_000_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${DST_EDT_HOUR + 1_800_000_000}"`, "the repeated first recorded sample")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(DST_EDT_HOUR + 1_800_000_000))
    const dstEdt = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(dstEdt, /^01:30:00;/)
    assert.doesNotMatch(dstEdt, /GMT|UTC|\.000/)
    await navigateTo(DST_EST_HOUR + 1_800_000_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').dataset.recordedTimestamp === "${DST_EST_HOUR + 1_800_000_000}"`, "the repeated second recorded sample")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(DST_EST_HOUR + 1_800_000_000))
    const dstEst = await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(dstEst, /^01:30:00;/)
    assert.doesNotMatch(dstEst, /GMT|UTC|\.000/)
    assert.equal(dstEdt.split(";")[0], dstEst.split(";")[0])
    assert.deepEqual(errors, [])
    assert.deepEqual(external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the minified artifact restores and clears its opaque browser session", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const responses = []
  const errors = []
  const external = []
  const authState = { valid: false }
  let rejectNextApi = false
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") {
      answerSession(request, response, authState)
      return
    }
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/")) {
      if (rejectNextApi) {
        rejectNextApi = false
        unauthorized(response)
        return
      }
      if (!browserIsAuthenticated(request, authState)) {
        unauthorized(response)
        return
      }
    }
    if (url.pathname === "/api/hour") {
      ndjson(response, timelineRecords(Number(url.searchParams.get("from") ?? HOUR)))
      return
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      ndjson(response, url.searchParams.getAll("section").includes("pg_stat_activity") ? snapshotRecords() : [])
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") {
      ndjson(response, [])
      return
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("session test server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const pageUrl = `${origin}/?at=${AT}&view=pg.activity`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  let browser = launchBrowser(profile)
  let socket
  try {
    let debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    let cdp = cdpSession(socket)
    trackPage(socket, origin, { errors, external, responses })
    await enablePage(cdp)

    let started = requests.length
    await cdp.send("Page.navigate", { url: pageUrl })
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "initial login")
    assertBootstrapBeforeApi(requests.slice(started), false)
    await submitLogin(cdp)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "signed-in application")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "initial API data")

    const storage = await cdp.evaluate(`(async () => ({
      cookie: document.cookie,
      indexedDatabases: (await indexedDB.databases()).map(({ name, version }) => ({ name, version })),
      local: Object.entries(localStorage),
      session: Object.entries(sessionStorage),
    }))()`)
    assert.equal(storage.cookie, "")
    assert.deepEqual(storage.indexedDatabases, [])
    assert.deepEqual(storage.session, [])
    const stored = JSON.stringify(storage)
    for (const secret of ["artifact", "wire", "Basic", SESSION_COOKIE, "YXJ0aWZhY3Q6d2lyZQ=="]) {
      assert.equal(stored.includes(secret), false, `browser storage contains ${secret}`)
    }

    started = requests.length
    await cdp.send("Page.reload")
    await waitForRequests(() => requests.slice(started).some(({ method, path }) => method === "GET" && path === "/auth/session"))
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "reload restoration")
    assertBootstrapBeforeApi(requests.slice(started), true)

    started = requests.length
    const created = await cdp.send("Target.createTarget", { url: "about:blank" })
    const tabSocket = await pageSocket(debugPort, created.targetId)
    const tab = cdpSession(tabSocket)
    trackPage(tabSocket, origin, { errors, external, responses })
    await enablePage(tab)
    await tab.send("Page.navigate", { url: pageUrl })
    await tab.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "new-tab restoration")
    assertBootstrapBeforeApi(requests.slice(started), true)
    await cdp.send("Target.closeTarget", { targetId: created.targetId })
    tabSocket.close()

    await cdp.send("Browser.close")
    await Promise.race([
      new Promise((resolve) => browser.once("exit", resolve)),
      delay(2_000),
    ])
    await stopBrowser(browser)
    socket.close()
    socket = undefined
    await delay(200)
    await rm(join(profile, "DevToolsActivePort"), { force: true })
    browser = launchBrowser(profile)
    debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    cdp = cdpSession(socket)
    trackPage(socket, origin, { errors, external, responses })
    await enablePage(cdp)
    started = requests.length
    await cdp.send("Page.navigate", { url: pageUrl })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "browser-restart restoration")
    assertBootstrapBeforeApi(requests.slice(started), true)

    const postsBeforeLogout = requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length
    started = requests.length
    await cdp.evaluate(`document.querySelector('.top-actions button[title="Sign out"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "logout")
    assert.equal(requests.slice(started).filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length, 1)
    started = requests.length
    await cdp.send("Page.reload")
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "signed-out reload")
    await waitForRequests(() => requests.slice(started).some(({ method, path }) => method === "GET" && path === "/auth/session"))
    const signedOutReload = requests.slice(started)
    assert.equal(signedOutReload.some(({ path }) => path.startsWith("/api/")), false)
    assert.equal(requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length, postsBeforeLogout)

    await submitLogin(cdp)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1`, "relogin")
    const postsBeforeExpiry = requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length
    const deletesBeforeExpiry = requests.filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length
    const responsesBeforeExpiry = responses.length
    rejectNextApi = true
    started = requests.length
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="login-message"]')?.textContent.includes("session ended") === true`, "one expired-session transition")
    await delay(300)
    const expiryRequests = requests.slice(started)
    const expiryApi = expiryRequests.filter(({ path }) => path.startsWith("/api/"))
    assert.equal(expiryApi.length, 2)
    assert.equal(expiryApi.filter(({ path, query }) => path === "/api/hour" && query.includes("section=postgresql_summary")).length, 1)
    assert.equal(expiryRequests.filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length, 1)
    assert.equal(requests.filter(({ method, path }) => method === "DELETE" && path === "/auth/session").length, deletesBeforeExpiry + 1)
    assert.equal(requests.filter(({ method, path }) => method === "POST" && path === "/auth/session").length, postsBeforeExpiry)
    const forcedResponses = responses.slice(responsesBeforeExpiry).filter(({ status }) => status === 401)
    assert.equal(forcedResponses.length, 1)
    assert.equal(forcedResponses[0]?.path.startsWith("/api/"), true)
    assert.equal(forcedResponses[0]?.challenge, null)

    const authorizedRequests = requests.filter(({ path }) => path.startsWith("/api/"))
    assert.ok(authorizedRequests.every(({ authorization, cookie, marker }) => (
      authorization === null && cookie === SESSION_COOKIE && marker === "1"
    )))
    const basicRequests = requests.filter(({ authorization }) => authorization !== null)
    assert.ok(basicRequests.every(({ method, path }) => method === "POST" && path === "/auth/session"))
    assert.deepEqual(errors, [])
    assert.deepEqual(external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("the slow-query group loads one opaque representative detail and preserves incomplete results", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: false }
  const requests = []
  let heldDetail = null
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") {
      answerSession(request, response, authState)
      return
    }
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) {
      unauthorized(response)
      return
    }
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end(JSON.stringify({ record: "instance_label", database: null }))
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") {
      ndjson(response, [])
      return
    }
    if (url.pathname === "/api/events") {
      ndjson(response, slowQueryEventRecords(Number(url.searchParams.get("from"))))
      return
    }
    if (url.pathname === "/api/row-detail") {
      heldDetail = response
      return
    }
    if (url.pathname === "/api/hour") {
      if (url.searchParams.has("section")) {
        ndjson(response, [{ record: "series_segment", segment: { id: SEGMENT } }])
        return
      }
      ndjson(response, slowQueryTimelineRecords())
      return
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("artifact test server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    const page = { errors: [], external: [], responses: [] }
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 882, mobile: false, width: 1280 })
    const initialAt = EVENT_SCOPE_AT
    const matchingFind = "source:pg_log_slow_queries"
    await cdp.send("Page.navigate", { url: `${origin}/?at=${initialAt}&view=events&find=${encodeURIComponent(matchingFind)}` })
    await cdp.waitFor(`document.querySelector('[data-testid="login-card"]') !== null`, "login form")
    await submitLogin(cdp)
    await cdp.waitFor(`document.querySelector('[data-testid="event-entry-title"]') !== null`, "the slow-query entry")
    await cdp.waitFor(`document.querySelector('[data-marker-count="4"]') !== null`, "the mixed timeline marker")
    await cdp.waitFor(`document.querySelector('[data-testid="events-truncated"]') !== null`, "the incomplete Events notice")
    await waitForRequests(() => requests.filter(({ path }) => path.startsWith("/api/")).length >= 4)
    await delay(200)

    const initialApi = requests.filter(({ path }) => path.startsWith("/api/"))
    assert.equal(initialApi.length, 4, JSON.stringify(initialApi, null, 2))
    assert.equal(initialApi.filter(({ path }) => path === "/api/events").length, 1)
    assert.equal(initialApi.filter(({ path }) => path === "/api/row-detail").length, 0)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="events-console"]').textContent.includes(${JSON.stringify(SLOW_STORED_TEXT)})`), false)
    assert.deepEqual(await cdp.evaluate(`(() => {
      const url = new URL(location.href)
      return { at: url.searchParams.get("at"), find: url.searchParams.get("find"), view: url.searchParams.get("view") }
    })()`), { at: String(initialAt), find: matchingFind, view: "events" })
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="events-truncated"]').textContent`), /result is incomplete/)

    const pressEnter = async () => {
      await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", nativeVirtualKeyCode: 13, text: "\r", unmodifiedText: "\r", windowsVirtualKeyCode: 13 })
      await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", nativeVirtualKeyCode: 13, windowsVirtualKeyCode: 13 })
    }
    await cdp.evaluate(`document.querySelector('[data-testid="event-entry"] > button').focus()`)
    await pressEnter()
    await cdp.waitFor(`document.querySelector('[data-testid="event-entry-facts"]') !== null`, "the keyboard-expanded slow-query group")
    assert.equal(requests.filter(({ path }) => path === "/api/row-detail").length, 0)

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await settleLayout(cdp)
    const title = await cdp.evaluate(`document.querySelector('[data-testid="event-entry-title"]').textContent`)
    assert.equal(title, SLOW_PATTERN)
    const entryText = await cdp.evaluate(`document.querySelector('[data-testid="event-entry"]').textContent`)
    assert.match(entryText, /3 раз/)
    assert.match(entryText, /6,29 с/)
    assert.match(entryText, /12,6 с/)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="events-truncated"]').textContent`), /результат неполный/)
    await settleLayout(cdp)

    const landscape = await cdp.evaluate(expansionGeometryExpression())
    assert.equal(landscape.innerWidth, 1280)
    assert.ok(landscape.scrollWidth <= landscape.clientWidth, JSON.stringify(landscape))
    assert.deepEqual(landscape.chips.map(({ text }) => text), ["3", "6,29 с", "12,6 с"])
    // A fact is unreadable when its value drifts to the far edge of a
    // full-width surface, which is what the dock's two-track row used to do.
    assert.ok(landscape.chips.every(({ gap }) => gap >= 0 && gap <= 12), JSON.stringify(landscape.chips))
    assert.equal(landscape.chips.every(({ sameRow }) => sameRow), true, JSON.stringify(landscape.chips))
    assert.ok(landscape.chips.every(({ height }) => height <= 28), JSON.stringify(landscape.chips))
    assert.equal(landscape.rawRows, 0)
    assert.doesNotMatch(landscape.text, /тыс\.\s*мс/iu)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector"]') === null`), true)

    await cdp.evaluate(`(() => {
      const button = document.querySelector('[data-testid="event-representative-detail"]')
      window.__eventRepresentativeButton = button
      button.focus()
    })()`)
    await pressEnter()
    await waitForRequests(() => requests.some(({ path }) => path === "/api/row-detail"))
    await cdp.waitFor(`document.querySelector('[data-testid="event-representative-detail"]')?.dataset.detailStatus === "loading"`, "the held representative detail")
    const detailRequests = requests.filter(({ path }) => path === "/api/row-detail")
    assert.equal(detailRequests.length, 1)
    assert.equal(detailRequests[0]?.method, "GET")
    const detailQuery = new URLSearchParams(detailRequests[0]?.query)
    assert.deepEqual([...detailQuery.keys()], ["detail_ref"])
    assert.equal(detailQuery.get("detail_ref"), SLOW_DETAIL_REF)
    assert.deepEqual(await cdp.evaluate(`(() => {
      const button = document.querySelector('[data-testid="event-representative-detail"]')
      const url = new URL(location.href)
      return {
        ariaDisabled: button.getAttribute("aria-disabled"),
        at: url.searchParams.get("at"),
        busy: button.getAttribute("aria-busy"),
        disabled: button.disabled,
        focused: document.activeElement === button,
        sameButton: window.__eventRepresentativeButton === button,
        status: button.dataset.detailStatus,
      }
    })()`), { ariaDisabled: "true", at: String(initialAt), busy: "true", disabled: false, focused: true, sameButton: true, status: "loading" })

    ndjson(heldDetail, slowQueryDetailRecords())
    heldDetail = null
    await cdp.waitFor(`document.querySelector('[data-testid="event-representative-detail"]')?.dataset.detailStatus === "loaded"`, "the representative detail")
    assert.deepEqual(await cdp.evaluate(`(() => {
      const button = document.querySelector('[data-testid="event-representative-detail"]')
      const occurrence = document.querySelector('[data-testid="event-representative-occurrence"]')
      const url = new URL(location.href)
      return {
        at: url.searchParams.get("at"),
        focused: document.activeElement === button,
        sameButton: window.__eventRepresentativeButton === button,
        storedText: occurrence.querySelector("pre").textContent,
      }
    })()`), { at: String(initialAt), focused: true, sameButton: true, storedText: SLOW_STORED_TEXT })

    await cdp.evaluate(`document.querySelector('[data-testid="event-entry"] > button').focus()`)
    await pressEnter()
    await cdp.waitFor(`document.querySelector('[data-testid="event-entry-detail"]') === null`, "the collapsed slow-query group")
    await pressEnter()
    await cdp.waitFor(`document.querySelector('[data-testid="event-representative-detail"]')?.dataset.detailStatus === "loaded"`, "the cached representative detail")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-representative-detail"]')?.textContent.includes("Go to recorded time") === true`, "the English cached detail")
    assert.equal(requests.filter(({ path }) => path === "/api/row-detail").length, 1)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)

    await cdp.evaluate(`document.querySelector('[data-testid="event-representative-detail"]').focus()`)
    await pressEnter()
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === ${JSON.stringify(String(AT))}`, "the exact representative timestamp")
    assert.deepEqual(await cdp.evaluate(`(() => {
      const url = new URL(location.href)
      return { at: url.searchParams.get("at"), find: url.searchParams.get("find"), view: url.searchParams.get("view") }
    })()`), { at: String(AT), find: matchingFind, view: "events" })
    assert.equal(requests.filter(({ path }) => path === "/api/row-detail").length, 1)

    assert.deepEqual(await cdp.evaluate(`(() => {
      const marker = document.querySelector('[data-marker-count="4"]')
      return {
        composition: marker.dataset.markerComposition,
        locators: marker.dataset.markerLocatorCount,
        text: marker.textContent,
      }
    })()`), { composition: "event:2 known_bad:2", locators: "5", text: "22" })
    await cdp.evaluate(`document.querySelector('[data-marker-count="4"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-entry-title"]') !== null`, "the non-representative event scope")
    await cdp.waitFor(`document.querySelector('[data-testid="events-scope"]')?.dataset.eventMarks === "2"`, "the explicit timeline selection")
    await waitForRequests(() => requests.filter(({ path }) => path === "/api/events").length === 2)
    const selectedScope = await cdp.evaluate(`(() => {
      const scope = document.querySelector('[data-testid="events-scope"]')
      const marks = document.querySelector('[data-testid="event-marks"]')
      return {
        log: scope.dataset.eventMarks,
        sharp: scope.dataset.sharpRiseMarks,
        text: scope.textContent,
        threshold: scope.dataset.thresholdMarks,
        thresholdText: marks.textContent,
      }
    })()`)
    assert.equal(selectedScope.log, "2")
    assert.equal(selectedScope.sharp, "0")
    assert.equal(selectedScope.threshold, "2")
    assert.match(selectedScope.text, /Выбрано на шкале.*События журнала: 2.*Срабатывания порогов: 2.*Весь час/s)
    assert.match(selectedScope.thresholdText, /Показатели: 2 · Срабатывания: 2/)
    assert.equal((selectedScope.thresholdText.match(/×1/g) ?? []).length, 2)
    const scopedRequest = requests.filter(({ path }) => path === "/api/events").at(-1)
    const scopedQuery = new URLSearchParams(scopedRequest.query)
    assert.equal(scopedQuery.get("from"), String(EVENT_SCOPE_AT))
    assert.equal(scopedQuery.get("to"), String(EVENT_SCOPE_AFTER + 1))
    assert.deepEqual(scopedQuery.getAll("source"), ["pg_log_slow_queries"])

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 882, mobile: false, width: 480 })
    await settleLayout(cdp)
    const narrow = await cdp.evaluate(expansionGeometryExpression())
    assert.equal(narrow.innerWidth, 480)
    assert.ok(narrow.scrollWidth <= narrow.clientWidth, JSON.stringify(narrow))
    assert.equal(narrow.chips.every(({ gap, sameRow }) => sameRow && gap >= 0 && gap <= 12), true, JSON.stringify(narrow.chips))

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 882, mobile: false, width: 1280 })
    await cdp.evaluate(`document.querySelector('[data-testid="events-scope"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="events-scope"]') === null`, "the whole-hour Events scope")
    await waitForRequests(() => requests.filter(({ path }) => path === "/api/events").length === 3)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="events-console"] input[type="search"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "text:never-present")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "text:never-present" && document.querySelector('[data-testid="event-entry"]') === null`, "the filtered incomplete Events result")
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="events-truncated"]').textContent`), /результат неполный/)
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="events-truncated"]').textContent`), /result is incomplete/)
    assert.equal(requests.filter(({ path }) => path === "/api/events").length, 3)
    assert.equal(requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).has("section")).length, 0)
    assert.equal(requests.filter(({ path }) => path === "/api/row-detail").length, 1)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    heldDetail?.destroy()
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("tablespace rollups keep exact history, URL drill, Back, search, and narrow geometry", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const requests = []
  const authState = { valid: true }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end(JSON.stringify({ record: "instance_label", database: "artifact_db" }))
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, url.searchParams.has("group") ? aggregateRelationHistoryRecords(url) : timelineRecords(HOUR))
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, relationRecords(url, "single"))
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("relation chart server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    const page = { errors: [], external: [], responses: [] }
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.indexes&level=tablespace&pg_lens=state&find=tablespace%3Afast_ssd` })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length === 1`, "the tablespace index aggregate")
    const summary = await cdp.evaluate(`(() => ({
      cells: [...document.querySelector('[data-testid="pg-indexes-table"] .entity-row').querySelectorAll('[role="cell"]')].map((cell) => cell.textContent),
      levels: [...document.querySelectorAll('nav.lensbar button')].map((button) => button.textContent),
    }))()`)
    assert.equal(summary.cells[0], "fast_ssd")
    assert.match(summary.cells[1], /363/)
    assert.deepEqual(summary.levels.slice(0, 4), ["Indexes", "Schemas", "Databases", "Tablespaces"])
    const snapshots = requests.filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && new URLSearchParams(query).get("group") === "tablespace")
    assert.ok(snapshots.some(({ query }) => new URLSearchParams(query).get("search") === "tablespace:fast_ssd"), JSON.stringify(snapshots))

    const comparisonStart = requests.length
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "(tablespace:fast_ssd or tablespace:archive) and (size > 100.000MB or scan_rate>10/s)")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "(tablespace:fast_ssd or tablespace:archive) and (size > 100.000MB or scan_rate>10/s)", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    const groupedBoolean = "(tablespace:fast_ssd OR tablespace:archive) AND (size>100MB OR scan_rate>10/s)"
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === ${JSON.stringify(groupedBoolean)} && document.querySelector('[data-testid="search-chips"]')?.textContent.includes("Size · > 100 MB") === true`, "the hidden-lens grouped boolean chips")
    await waitForRequests(() => requests.slice(comparisonStart).some(({ query }) => new URLSearchParams(query).get("search") === groupedBoolean))
    const comparisonRequest = requests.slice(comparisonStart).find(({ query }) => new URLSearchParams(query).get("search") === groupedBoolean)
    assert.notEqual(comparisonRequest, undefined)
    const comparisonQuery = new URLSearchParams(comparisonRequest.query)
    assert.equal(comparisonQuery.get("group"), "tablespace")
    assert.equal(comparisonQuery.getAll("field").includes("main_fork_bytes"), false)
    const comparisonChip = await cdp.evaluate(`(() => {
      const chip = document.querySelector('[data-testid="search-chips"] [title="size>100MB"]').parentElement
      return {
        label: chip.querySelector("button").getAttribute("aria-label"),
        text: chip.textContent,
        title: chip.querySelector("[title]")?.getAttribute("title"),
      }
    })()`)
    assert.equal(comparisonChip.title, "size>100MB")
    assert.match(comparisonChip.label, /Remove Size: > 100 MB/)
    assert.match(comparisonChip.text, /Size · > 100 MB/)

    const mixedStart = requests.length
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "tablespace:fast_ssd OR size>100MB")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "tablespace:fast_ssd OR size>100MB", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-error"] mark')?.textContent === "OR"`, "the grouped mixed OR span")
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="search-error"]')?.textContent ?? ""`), /cannot mix names or text with metrics/)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), groupedBoolean)
    assert.equal(requests.slice(mixedStart).some(({ query }) => new URLSearchParams(query).get("search") === "tablespace:fast_ssd OR size>100MB"), false)

    const invalidStart = requests.length
    const retainedRow = await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "size>=100MB")
      input.dispatchEvent(new InputEvent("input", { bubbles: true, data: "size>=100MB", inputType: "insertFromPaste" }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-error"] mark')?.textContent === ">="`, "the atomic unsupported operator span")
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), groupedBoolean)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').textContent`), retainedRow)
    assert.equal(requests.slice(invalidStart).some(({ query }) => query.includes("size%3E%3D") || query.includes("size%3E=")), false)

    await assertSearchControlContained(cdp, "Indexes comparison search", '[data-search-surface="pg_stat_user_indexes"]')
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 640, mobile: false, width: 360 })
    await settleLayout(cdp)
    const narrowComparison = await cdp.evaluate(`(() => ({
      chip: document.querySelector('[data-testid="search-chips"]')?.textContent ?? "",
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    }))()`)
    assert.match(narrowComparison.chip, /Size · > 100 MB/)
    assert.match(narrowComparison.chip, /OR/)
    assert.equal(narrowComparison.overflow, false)
    await cdp.evaluate(`document.querySelector('[aria-label="Синтаксис и поля поиска"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') !== null`, "the RU grouped-search help")
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="search-help"]')?.textContent ?? ""`), /оператор OR не может объединять текстовые условия с условиями по метрикам/)
    await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
    await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, "the RU grouped-search help closed")
    for (const width of [800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width })
      await settleLayout(cdp)
      await assertSearchControlContained(cdp, `RU grouped search at ${width}`, '[data-search-surface="pg_stat_user_indexes"]')
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: width === 360 ? 640 : 768, mobile: false, width })
      await settleLayout(cdp)
      await assertSearchControlContained(cdp, `EN grouped search at ${width}`, '[data-search-surface="pg_stat_user_indexes"]')
    }
    await assertSearchChipHierarchyMatrix(cdp, "grouped search chip hierarchy")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null`, "comparison clear")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-indexes-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') !== null`, "the aggregate detail")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-detail"] .uplot-host') === null`), true)
    const detailText = await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-detail"]').textContent`)
    assert.match(detailText, /fast_ssd/)
    assert.match(detailText, /1663/)
    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot .pg-metric-history .uplot-host canvas') !== null`, "the aggregate history Chart")
    await settleLayout(cdp)

    const historyRequests = requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).has("group"))
    assert.equal(historyRequests.length, 1, JSON.stringify(historyRequests))
    const query = new URLSearchParams(historyRequests[0].query)
    assert.equal(query.get("group"), "tablespace")
    assert.equal(query.get("where.tablespace_oid"), "1663")
    assert.equal(query.get("where.datid"), null)
    assert.equal(query.get("where.schemaname"), null)
    assert.equal(query.get("type_id"), null)
    assert.deepEqual(query.getAll("field"), ["index_count", "invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"])
    const layout = await cdp.evaluate(`(() => {
      const slot = document.querySelector('.inspector-chart-slot')
      const chart = slot.querySelector('.uplot-host')
      const plot = slot.querySelector('.u-over')
      const table = document.querySelector('[data-testid="pg-indexes-table"]')
      const selectors = [...slot.querySelectorAll('.history-selector button')]
      return {
        chartWidth: chart.getBoundingClientRect().width,
        slotWidth: slot.getBoundingClientRect().width,
        plotWidth: plot.getBoundingClientRect().width,
        tableWidth: table.getBoundingClientRect().width,
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        selectors: selectors.map((button) => button.textContent),
      }
    })()`)
    assert.ok(layout.chartWidth > 250 && layout.chartWidth <= layout.slotWidth, JSON.stringify(layout))
    assert.ok(layout.plotWidth > 176, JSON.stringify(layout))
    assert.ok(layout.tableWidth >= 500, JSON.stringify(layout))
    assert.equal(layout.overflow, false)
    assert.equal(layout.selectors.length, 6)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') === null`), true)
    assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('.timeline-preview').getBoundingClientRect().height`) - 124) <= .5)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-detail"] .chart-expand, [role="dialog"].uplot-expanded') === null`), true)
    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[0].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector"][data-panel="detail"]') !== null`, "the aggregate Detail restored")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-relation-drill"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('level') === null && new URL(location.href).searchParams.get('tablespace_oid') === '1663'`, "the exact tablespace member URL")
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-indexes-table"] .entity-row').length === 1`, "the tablespace members")
    const memberRequests = requests.filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && new URLSearchParams(query).get("where.tablespace_oid") === "1663")
    assert.ok(memberRequests.length > 0, JSON.stringify(requests))
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('level') === 'tablespace' && document.querySelector('[data-testid="pg-relation-detail"]') !== null`, "the selected tablespace restored by Back")
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 844, mobile: false, width: 390 })
    await settleLayout(cdp)
    const narrow = await cdp.evaluate(`(() => ({
      clientWidth: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
      buttons: [...document.querySelectorAll('nav.lensbar .lens-tabs button')].map((button) => ({ text: button.textContent, width: button.getBoundingClientRect().width })),
    }))()`)
    assert.ok(narrow.scrollWidth <= narrow.clientWidth, JSON.stringify(narrow))
    assert.deepEqual(narrow.buttons.map(({ text }) => text), ["Indexes", "Schemas", "Databases", "Tablespaces"])
    assert.equal(narrow.buttons.every(({ width }) => width > 55), true, JSON.stringify(narrow))
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test.skip("legacy chart visibility preference is replaced by the permanent preview", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  let summaryMode = "initial"
  const heldSummaries = []
  const heldCgroups = []
  const servedActivityHistory = []
  let holdCgroups = false
  const historyRequests = (section) => requests.filter(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === section)
  const snapshotRequests = (section) => requests.filter(({ path, query }) => path === `/api/segments/${SEGMENT}/snapshot` && new URLSearchParams(query).getAll("section").includes(section))
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const hour = Number(url.searchParams.get("from") ?? HOUR)
      const section = url.searchParams.get("section")
      if (section === "os_process_summary") {
        if (summaryMode === "hold") {
          heldSummaries.push(response)
          return
        }
        if (summaryMode === "fail") {
          response.writeHead(200, { "Content-Type": "application/x-ndjson; charset=utf-8" })
          response.end("{")
          return
        }
        if (summaryMode === "empty") return ndjson(response, [])
        return ndjson(response, processSummaryRecords(hour, summaryMode === "initial" ? 719 : 2, summaryMode === "initial" ? 719 : 721))
      }
      if (section === "pg_stat_activity") {
        const records = activityHistoryRecords(url)
        servedActivityHistory.push(records)
        return ndjson(response, records)
      }
      return ndjson(response, section === null ? timelineRecords(hour, true) : [])
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      if (sections.length === 1 && ["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].includes(sections[0])) {
        if (holdCgroups) {
          heldCgroups.push({ response, url })
          return
        }
        return ndjson(response, cgroupSnapshotRecords(url))
      }
      if (sections.includes("pg_stat_user_tables") || sections.includes("pg_stat_user_indexes")) return ndjson(response, relationRecords(url, "single"))
      if (sections.includes("os_cpu")) return ndjson(response, systemSnapshotRecords(true, Number(url.searchParams.get("at") ?? AT)))
      if (sections.includes("pg_stat_activity")) return ndjson(response, snapshotRecords())
      return ndjson(response, [])
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("chart browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=processes&lens=generic` })
    await cdp.waitFor(`document.querySelector('.process-summary > div:first-child strong')?.textContent === "719"`, "719 process-summary rows", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .uplot-host canvas') !== null`, "the Process timeline")
    await settleLayout(cdp)
    const shownProcessHeight = await cdp.evaluate(`document.querySelector('.process-table .entity-scroll').getBoundingClientRect().height`)
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty') === null`, "all Process charts hidden")
    await settleLayout(cdp)
    const hiddenProcessHeight = await cdp.evaluate(`document.querySelector('.process-table .entity-scroll').getBoundingClientRect().height`)
    assert.ok(hiddenProcessHeight > shownProcessHeight, JSON.stringify({ hiddenProcessHeight, shownProcessHeight }))
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').getAttribute("aria-label")`), "Show charts")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').getAttribute("aria-pressed")`), "false")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] .uplot-host canvas') !== null`, "the restored Process timeline")

    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1366 })
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, "the full-height process table")
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "the captured-user process row")
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'cpu_cores>0.1 AND rss>2MiB')
      input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertFromPaste' }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "cpu_cores>0.1 AND rss>2MiB"`, "natural quantitative Processes search")
    await waitForRequests(() => requests.some(({ query }) => new URLSearchParams(query).get("search") === "cpu_cores>0.1 AND rss>2MiB"))
    assert.deepEqual(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="search-chips"] strong')].map((node) => node.textContent)`), ["CPU cores", "RSS"])
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null`, "clear quantitative Processes search")
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "captured-user row after quantitative search")
    const processJoinGap = await cdp.evaluate(`(() => {
      const timeline = document.querySelector('.timeline-shell').getBoundingClientRect()
      const controls = document.querySelector('.process-workspace > .lensbar').getBoundingClientRect()
      return controls.top - timeline.bottom
    })()`)
    assert.ok(processJoinGap <= 1, `Process major-region gap ${processJoinGap}px`)
    const capturedUsers = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      const headers = [...table.querySelectorAll('[role="columnheader"]')].map((cell) => cell.querySelector('.entity-sort span')?.textContent.trim() ?? '')
      const selected = [...table.querySelectorAll('.entity-row')].find((row) => row.textContent.includes('2686712'))
      return Object.fromEntries([...selected.querySelectorAll('[role="cell"]')].map((cell, index) => [headers[index], cell.textContent.trim()]))
    })()`)
    assert.equal(capturedUsers.User, "postgres (26)")
    assert.equal(capturedUsers["Effective user"], "postgres-worker (27)")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="process-table"] .entity-row')].find((row) => row.textContent.includes("2686712"))).querySelector('[data-testid="process-user-filter-user"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "user:postgres"`, "resolved real user opens canonical name search")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="process-dock"]') === null`), true)
    await cdp.evaluate(`document.querySelector('[aria-label="Clear the filter"]').click()`)
    await delay(400)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === null && [...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "process rows restored after real-user search")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="process-table"] .entity-row')].find((row) => row.textContent.includes("2686712"))).querySelector('[data-testid="process-user-filter-effective_user"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "effective_user:postgres-worker"`, "resolved effective user opens canonical name search")
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=processes&lens=generic` })
    await cdp.waitFor(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row')].some((row) => row.textContent.includes("2686712"))`, "ordinary process rows restored after user search")
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="process-table"] .entity-row')].find((row) => row.textContent.includes("2686712"))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-linked-dock"] [data-testid="pg-exact-query"]')?.textContent.includes("select activity_for_2686712") === true`, "the PID-first linked Activity query")
    await cdp.evaluate(`document.querySelector('[data-testid="lens-cpu"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-history-metric-majflt"]') !== null`, "Major page faults history")
    const cpuDetail = await cdp.evaluate(`(() => ({
      labels: [...document.querySelectorAll('[data-testid="pg-linked-dock"] > dl:first-of-type dt')].map((node) => node.textContent),
      schedulerChips: ["nice", "prio", "rtprio"].filter((field) => document.querySelector('[data-testid="process-history-metric-' + field + '"]') !== null),
      snapshot: document.querySelector('[data-testid="pg-linked-dock"]')?.textContent ?? "",
    }))()`)
    assert.equal(cpuDetail.schedulerChips.length, 0)
    for (const label of ["Nice", "Priority", "RT priority"]) assert.ok(cpuDetail.labels.some((value) => value.startsWith(label)), JSON.stringify(cpuDetail.labels))
    assert.equal(cpuDetail.labels.filter((value) => value === "User?").length, 1, JSON.stringify(cpuDetail.labels))
    assert.equal(cpuDetail.labels.filter((value) => value === "Effective user?").length, 1, JSON.stringify(cpuDetail.labels))
    assert.match(cpuDetail.snapshot, /postgres \(26\)/)
    assert.match(cpuDetail.snapshot, /postgres-worker \(27\)/)
    assert.doesNotMatch(cpuDetail.snapshot, /\b\d{2}[./]\d{2}[./]2026\b/)
    for (const lens of ["cpu", "memory", "disk", "generic"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="lens-${lens}"]').click()`)
      await settleLayout(cdp)
      await cdp.waitFor(`document.querySelector('[data-testid="table-paging"]') === null`, `${lens} process page settled`)
      const geometry = await cdp.evaluate(`(() => {
        const box = (node) => { const value = node.getBoundingClientRect(); return { bottom: value.bottom, top: value.top } }
        const main = document.querySelector('.process-main')
        const table = document.querySelector('[data-testid="process-table"]')
        const scroll = table.querySelector('.entity-scroll')
        const dock = document.querySelector('[data-testid="pg-linked-dock"]')
        return { dock: box(dock), dockClient: dock.clientHeight, dockScroll: dock.scrollHeight, main: box(main), scrollClient: scroll.clientHeight, scrollScroll: scroll.scrollHeight, table: box(table), viewport: innerHeight }
      })()`)
      assert.ok(geometry.viewport - geometry.main.bottom >= 0 && geometry.viewport - geometry.main.bottom <= 21, `${lens} remaining row: ${JSON.stringify(geometry)}`)
      assert.ok(Math.abs(geometry.table.top - geometry.main.top) <= 1 && Math.abs(geometry.table.bottom - geometry.main.bottom) <= 1, `${lens} table row: ${JSON.stringify(geometry)}`)
      assert.ok(Math.abs(geometry.dock.top - geometry.main.top) <= 1 && Math.abs(geometry.dock.bottom - geometry.main.bottom) <= 1, `${lens} dock row: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.scrollScroll > geometry.scrollClient, `${lens} independent table scroll: ${JSON.stringify(geometry)}`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 800 })
    await settleLayout(cdp)
    const narrowProcess = await cdp.evaluate(`(() => {
      const dock = document.querySelector('[data-testid="pg-linked-dock"]').getBoundingClientRect()
      const main = document.querySelector('.process-main').getBoundingClientRect()
      return { dockBottom: dock.bottom, dockTop: dock.top, mainBottom: main.bottom, viewport: innerHeight }
    })()`)
    assert.ok(narrowProcess.dockTop >= 0 && narrowProcess.dockBottom <= narrowProcess.viewport && narrowProcess.viewport - narrowProcess.mainBottom <= 21, JSON.stringify(narrowProcess))
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })

    summaryMode = "fail"
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "Host before same-hour summary remount")
    const hostJoinGap = await cdp.evaluate(`(() => {
      const timeline = document.querySelector('.timeline-shell').getBoundingClientRect()
      const primary = document.querySelector('.workspace > .lensbar, .workspace > .system-main').getBoundingClientRect()
      return primary.top - timeline.bottom
    })()`)
    assert.ok(hostJoinGap <= 1, `Host major-region gap ${hostJoinGap}px`)
    // Each container row discloses its own cgroup table; opening a row is idempotent.
    const containerRows = [["os_cgroup_cpu", "cgroup_cpu"], ["os_cgroup_memory", "cgroup_memory"], ["os_cgroup_io", "cgroup_io"]]
    const openContainerRow = (row) => cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-${row}"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cgroup_cpu"]') !== null`, "the container rows", 15_000)
    for (const [panel, row] of containerRows) {
      await openContainerRow(row)
      await cdp.waitFor(`document.querySelector('[data-testid="system-${panel}"] .entity-row') !== null`, `the ${panel} panel`, 15_000)
    }
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU metrics", 15_000)
    const systemHistoryBefore = historyRequests("os_cpu").length
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await waitForRequests(() => historyRequests("os_cpu").length > systemHistoryBefore)
    assert.ok(historyRequests("os_cpu").length > systemHistoryBefore)
    const systemSnapshots = requests.filter(({ path }) => path === `/api/segments/${SEGMENT}/snapshot`).map(({ query }) => new URLSearchParams(query))
    const primarySystem = systemSnapshots.find((query) => query.getAll("section").includes("os_cgroup_context"))
    assert.notEqual(primarySystem, undefined)
    assert.deepEqual(primarySystem.getAll("section").filter((section) => section.startsWith("os_cgroup_") && section !== "os_cgroup_context"), [])
    const expectedCgroups = {
      os_cgroup_cpu: null,
      os_cgroup_memory: null,
      os_cgroup_io: null,
    }
    for (const [section, path] of Object.entries(expectedCgroups)) {
      const matches = systemSnapshots.filter((query) => query.getAll("section").includes(section))
      assert.ok(matches.length >= 1, section)
      for (const query of matches) {
        assert.deepEqual(query.getAll("section"), [section])
        assert.equal(query.get("where.cgroup_path"), path)
        assert.equal(query.get("where.scope"), null)
      }
    }
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cgroup_cpu"]') !== null`, "the container rows", 15_000)
    for (const [, row] of containerRows) await openContainerRow(row)
    holdCgroups = true
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "the changed System cursor")
    // Loading is not absence: the panels may hold their frames with the loading
    // line, but no prior rows are visible while the new key loads.
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => {
      const panel = document.querySelector('[data-testid="system-' + section + '"]')
      return panel === null || (panel.querySelectorAll(".entity-row").length === 0 && panel.textContent.includes("Loading rows"))
    })`, "prior cgroup rows cleared while the new key loads")
    await waitForRequests(() => heldCgroups.length === 3)
    const replacementPaths = {
      os_cgroup_cpu: null,
      os_cgroup_memory: null,
      os_cgroup_io: null,
    }
    assert.deepEqual(Object.fromEntries(heldCgroups.map(({ url }) => [url.searchParams.get("section"), url.searchParams.get("where.cgroup_path")])), replacementPaths)
    assert.equal(heldCgroups.every(({ url }) => url.searchParams.get("at") === String(BEFORE_AT) && url.searchParams.get("where.scope") === null), true)
    holdCgroups = false
    for (const held of heldCgroups.splice(0)) if (!held.response.destroyed) ndjson(held.response, cgroupSnapshotRecords(held.url))
    for (const [panel, row] of containerRows) {
      await openContainerRow(row)
      await cdp.waitFor(`document.querySelector('[data-testid="system-${panel}"] .entity-row') !== null`, "the replacement collector cgroup rows", 15_000)
    }
    holdCgroups = true
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowRight" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${AT}"`, "the failed System cursor")
    await cdp.waitFor(`["os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io"].every((section) => {
      const panel = document.querySelector('[data-testid="system-' + section + '"]')
      return panel === null || (panel.querySelectorAll(".entity-row").length === 0 && panel.textContent.includes("Loading rows"))
    })`, "prior cgroup rows cleared before a failed exact load")
    await waitForRequests(() => heldCgroups.length === 3)
    holdCgroups = false
    for (const held of heldCgroups.splice(0)) {
      if (held.response.destroyed) continue
      held.response.writeHead(200, { "Content-Type": "application/x-ndjson; charset=utf-8" })
      held.response.end("{")
    }
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]') === null`, "the cursor caught up after exact-load failures", 15_000)
    for (const [panel, row] of containerRows) {
      await openContainerRow(row)
      await cdp.waitFor(`document.querySelector('[data-testid="system-${panel}"]') === null`, `no stale ${panel} rows after exact-load failures`, 15_000)
    }
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Could not load process totals" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "719"`, "same-hour error with retained totals", 15_000)

    summaryMode = "hold"
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "System before held same-hour summary remount")
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await waitForRequests(() => heldSummaries.length !== 0)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Loading process totals…" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "719"`, "same-hour loading with retained totals", 15_000)
    const sameHourSummaries = heldSummaries.splice(0)
    for (const held of sameHourSummaries) if (!held.destroyed) ndjson(held, processSummaryRecords(HOUR, 2, 720))
    await cdp.waitFor(`document.querySelector('.process-summary > div:first-child strong')?.textContent === "720" && document.querySelector('[data-testid="process-summary-status"]') === null`, "same-hour replacement totals", 15_000)

    summaryMode = "hold"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
    await waitForRequests(() => heldSummaries.length !== 0)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Loading process totals…" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "—"`, "cleared totals during a cross-hour load", 15_000)
    summaryMode = "good"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('.process-summary > div:first-child strong')?.textContent === "721" && document.querySelector('[data-testid="process-summary-status"]') === null`, "replacement totals after the aborted request", 15_000)
    for (const held of heldSummaries) if (!held.destroyed) ndjson(held, processSummaryRecords(HOUR + HOUR_US, 2, 999))
    await delay(100)
    assert.equal(await cdp.evaluate(`document.querySelector('.process-summary > div:first-child strong')?.textContent`), "721")

    summaryMode = "fail"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "Could not load process totals" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "—"`, "cross-hour summary request failure without prior totals", 15_000)
    summaryMode = "empty"
    await cdp.evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "No data in the selected hour" && document.querySelector('.process-summary > div:first-child strong')?.textContent === "—"`, "successful empty process totals", 15_000)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "the activity table", 15_000)
    await settleLayout(cdp)
    const activityDuration = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-activity-table"]')
      const headers = [...table.querySelectorAll('[role="columnheader"]')].map((cell) => cell.textContent.trim())
      const index = headers.findIndex((label) => label.includes("Query time"))
      return index < 0 ? null : table.querySelector('.entity-row').querySelectorAll('[role="cell"]')[index]?.textContent.trim() ?? null
    })()`)
    assert.match(activityDuration, /\d/)
    const activityHistoryBefore = historyRequests("pg_stat_activity").length
    await cdp.evaluate(`(() => {
      const row = document.querySelector('[data-testid="pg-activity-table"] .entity-row')
      row.focus()
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null`, "Activity detail")
    await waitForRequests(() => historyRequests("pg_stat_activity").length > activityHistoryBefore)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"] .pg-metric-history .uplot-host canvas') !== null`, "numeric Activity history", 15_000)
    const activityHistory = historyRequests("pg_stat_activity")
    const visibleActivityHistoryCount = activityHistory.length
    const activityQuery = new URLSearchParams(activityHistory.at(-1).query)
    assert.equal(activityQuery.get("from"), String(HOUR))
    assert.equal(activityQuery.get("to"), String(HOUR + HOUR_US - 1))
    assert.equal(activityQuery.get("section"), "pg_stat_activity")
    assert.deepEqual(activityQuery.getAll("field"), ["pid", "state", "query_start"])
    assert.deepEqual([...activityQuery.keys()].filter((name) => name.startsWith("where.")), ["where.pid"])
    assert.equal(activityQuery.get("where.pid"), "4242")
    assert.equal(activityQuery.has("type_id"), false)
    const servedActivity = servedActivityHistory.at(-1)
    const servedLayout = servedActivity.find(({ record }) => record === "layout")
    assert.deepEqual(servedLayout.layout.columns.map(({ name }) => name), ["pid", "state", "query_start"])
    assert.equal(servedActivity.filter(({ record }) => record === "row").every(({ values }) => values.length === 3), true)
    const activitySample = await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"] .chart-navigator').getAttribute("aria-valuetext")`)
    assert.match(activitySample, /Query time.*\d/)
    assert.doesNotMatch(activitySample, /—/)

    await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"] .chart-expand').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"] [role="dialog"]') !== null`, "the expanded Activity chart")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[role="dialog"]') === null`, "the Activity chart collapse")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"]') !== null`), true)

    await cdp.evaluate(`document.querySelector('[data-testid="help-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="help-panel"]') !== null`, "Help over Activity detail")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="help-panel"]') === null`, "Help close over Activity detail")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"]') !== null`), true)

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "hour picker over Activity detail")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "hour picker close over Activity detail")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"]') !== null`), true)

    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null && document.activeElement === document.querySelector('[data-testid="pg-activity-table"] .entity-row')`, "Activity detail close and row focus restore")
    const shownActivity = await cdp.evaluate(`(() => {
      const bounds = (selector) => document.querySelector(selector).getBoundingClientRect().height
      return {
        layout: bounds('[data-testid="pg-entity-layout"]'),
        main: bounds('.pg-entity-main'),
        scroll: bounds('[data-testid="pg-activity-table"] .entity-scroll'),
        table: bounds('[data-testid="pg-activity-table"]'),
        workspace: bounds('.workspace'),
      }
    })()`)
    const activitySnapshotsBeforeHiddenDetail = snapshotRequests("pg_stat_activity").length
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.timeline-shell') === null`, "activity charts hidden")
    await settleLayout(cdp)
    const hiddenActivity = await cdp.evaluate(`(() => {
      const bounds = (selector) => document.querySelector(selector).getBoundingClientRect().height
      return {
        layout: bounds('[data-testid="pg-entity-layout"]'),
        main: bounds('.pg-entity-main'),
        scroll: bounds('[data-testid="pg-activity-table"] .entity-scroll'),
        table: bounds('[data-testid="pg-activity-table"]'),
        workspace: bounds('.workspace'),
      }
    })()`)
    assert.ok(hiddenActivity.scroll > shownActivity.scroll + 100, JSON.stringify({ hiddenActivity, shownActivity }))
    await cdp.evaluate(`(() => {
      const row = document.querySelector('[data-testid="pg-activity-table"] .entity-row')
      row.focus()
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null && document.querySelector('[data-testid="pg-detail"] .pg-metric-history') === null`, "Activity detail with charts hidden")
    await waitForRequests(() => snapshotRequests("pg_stat_activity").length > activitySnapshotsBeforeHiddenDetail)
    await delay(100)
    assert.equal(historyRequests("pg_stat_activity").length, visibleActivityHistoryCount)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-exact-query"]')?.textContent`), "select artifact_wire_contract")
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null`, "hidden Activity detail close")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.timeline-shell') !== null`, "activity charts restored")
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent === "Tables")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"] .entity-row') !== null`, "the relation table", 15_000)
    const relationHistoryBefore = historyRequests("pg_stat_user_tables").length
    await cdp.evaluate(`(() => {
      const row = document.querySelector('[data-testid="pg-tables-table"] .entity-row')
      row.focus()
      row.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))
    })()`)
    await cdp.waitFor(`document.querySelector('.pg-metric-history') !== null`, "the relation history panel")
    await waitForRequests(() => historyRequests("pg_stat_user_tables").length > relationHistoryBefore)
    await settleLayout(cdp)
    const shownRelationHeight = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').getBoundingClientRect().height`)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') === null && document.activeElement === document.querySelector('[data-testid="pg-tables-table"] .entity-row')`, "relation detail close and row focus restore")
    await cdp.evaluate(`document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"] .pg-metric-history') !== null`, "the reopened relation history")
    await waitForRequests(() => historyRequests("pg_stat_user_tables").length > relationHistoryBefore + 1)
    const visibleRelationHistoryCount = historyRequests("pg_stat_user_tables").length
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.charts-hidden') !== null && document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .pg-metric-history') === null`, "all relation charts hidden")
    await settleLayout(cdp)
    const hiddenRelationHeight = await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-scroll').getBoundingClientRect().height`)
    assert.ok(hiddenRelationHeight > shownRelationHeight + 100, JSON.stringify({ hiddenRelationHeight, shownRelationHeight }))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') === null`, "hidden relation detail close")
    await cdp.evaluate(`document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-relation-detail"]') !== null && document.querySelector('[data-testid="pg-relation-detail"] .pg-metric-history') === null`, "relation detail reopened with charts hidden")
    await delay(100)
    assert.equal(historyRequests("pg_stat_user_tables").length, visibleRelationHistoryCount)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] .entity-row') !== null`), true)

    const hiddenSystemSnapshotsBefore = snapshotRequests("os_cpu").length
    const hiddenSystemHistoryBefore = historyRequests("os_cpu").length
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.section-tabs [role="tab"]:first-child') !== null`, "Host sections")
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "System with charts hidden")
    await waitForRequests(() => snapshotRequests("os_cpu").length > hiddenSystemSnapshotsBefore)
    await delay(100)
    assert.equal(historyRequests("os_cpu").length, hiddenSystemHistoryBefore)
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .use-table, .use-history, .system-entity-history') === null`), true)
    const hiddenSummaryBefore = historyRequests("os_process_summary").length
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('.process-table') !== null`, "Processes with charts hidden")
    await waitForRequests(() => historyRequests("os_process_summary").length > hiddenSummaryBefore)
    await cdp.waitFor(`document.querySelector('[data-testid="process-summary-status"]')?.textContent === "No data in the selected hour"`, "visible process cards retained with charts hidden")
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty, .process-summary-history, .process-history') === null`), true)
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent === "Events")).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="events-console"]') !== null`, "Events with charts hidden")
    assert.equal(await cdp.evaluate(`document.querySelector('.uplot-figure, .series-chart, .timeline-shell, .timeline-empty') === null`), true)

    const persistedSystemHistoryBefore = historyRequests("os_cpu").length
    const persistedSystemSnapshotsBefore = snapshotRequests("os_cpu").length
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="charts-toggle"]')?.getAttribute("aria-label") === "Show charts" && document.querySelector('.system-main') !== null`, "the persisted hidden preference", 15_000)
    await waitForRequests(() => snapshotRequests("os_cpu").length > persistedSystemSnapshotsBefore)
    await delay(100)
    assert.equal(historyRequests("os_cpu").length, persistedSystemHistoryBefore)
    assert.equal(await cdp.evaluate(`localStorage.getItem("kronika.charts")`), "0")
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`document.querySelector('.timeline-shell') !== null`, "charts shown again")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("PostgreSQL is unavailable without current telemetry and returns for a stored hour", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  let historical = false
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, sourceTimelineRecords(historical))
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      return ndjson(response, sections.includes("pg_stat_activity") ? snapshotRecords() : systemSnapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("source browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.overview` })
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null && document.querySelectorAll('.source-tabs button')[2]?.getAttribute('aria-current') === "page"`, "the explicit PostgreSQL destination", 15_000)
    const unavailable = await cdp.evaluate(`(() => {
      const sourceButtons = document.querySelectorAll('.source-tabs button')
      return {
        pgDisabled: sourceButtons[2].disabled,
        pgPanels: document.querySelectorAll('.pg-tabs, .pg-overview, [data-testid^="pg-"]').length,
        pgHealth: document.querySelector('[data-primary]')?.textContent.includes('PostgreSQL') ?? false,
        view: new URL(location.href).searchParams.get('view'),
      }
    })()`)
    assert.deepEqual(unavailable, { pgDisabled: false, pgHealth: false, pgPanels: 1, view: "pg.overview" })
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('.system-main') !== null`, "the Host destination", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the host CPU cards", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition"] .u-over') !== null`, "the CPU composition history")
    await cdp.evaluate(`document.querySelector('[data-testid="system-cpu-composition-all"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition-all"]')?.getAttribute("aria-pressed") === "true"`, "the whole CPU composition")
    await cdp.evaluate(`(() => {
      const plot = document.querySelector('[data-testid="system-cpu-composition"] .u-over')
      const bounds = plot.getBoundingClientRect()
      const clientX = bounds.left + (${AT} - ${HOUR}) / ${HOUR_US} * bounds.width
      const clientY = bounds.top + bounds.height / 2
      plot.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, clientX, clientY }))
      plot.dispatchEvent(new MouseEvent("mousemove", { bubbles: true, clientX, clientY }))
    })()`)
    await cdp.waitFor(`(() => {
      const chart = document.querySelector('[data-testid="system-cpu-composition"]')
      const tooltip = chart?.querySelector('.chart-tooltip')
      const host = chart?.querySelector('.uplot-host')
      if (chart === null || tooltip === null || host === null) return false
      window.__kronikaCpuChart = { axes: chart.querySelectorAll('.u-axis').length, label: host.getAttribute('aria-label'), tooltip: tooltip.textContent }
      return true
    })()`, "the CPU composition tooltip")
    const cpuChart = await cdp.evaluate(`window.__kronikaCpuChart`)
    assert.equal(cpuChart.axes, 3)
    for (const label of ["CPU used, cores", "CPU cores", "User CPU", "System CPU", "IRQ", "I/O wait", "Steal", "Idle"]) {
      assert.match(cpuChart.label, new RegExp(label))
      assert.match(cpuChart.tooltip, new RegExp(label))
    }
    assert.equal(await cdp.evaluate(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`), true)
    historical = true
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.overview` })
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null && document.querySelectorAll('.source-tabs button')[2]?.getAttribute('aria-current') === "page"`, "the stored PostgreSQL hour", 15_000)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].disabled`), false)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("PostgreSQL detail dock stays inside the viewport", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  const fixture = viewportActivityRows(new URL("http://fixture/snapshot"))
  assert.equal(fixture[0]?.layout?.logical_name, "pg_stat_activity")
  assert.equal(fixture.length, 121)
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(`${url.pathname}?${url.searchParams}`)
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      return ndjson(response, url.searchParams.get("section") === "pg_stat_activity"
        ? viewportActivityHistory(url)
        : viewportActivityTimeline())
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, viewportActivityRows(url))
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("viewport browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  const measurements = []
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", {
      name: "kronika_session",
      url: origin,
      value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1),
    })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]') !== null`, "the viewport activity table", 15_000)
    await waitForRequests(() => requests.some((value) => value.startsWith(`/api/segments/${SEGMENT}/snapshot?`)))
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "the viewport activity rows", 15_000)

    for (const [width, height] of [[1280, 882], [1366, 768], [960, 882], [390, 480]]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
      await settleLayout(cdp)
      await cdp.evaluate(`document.querySelector('.inspector-close')?.click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null`, `${width}x${height} closed detail`)
      await cdp.evaluate(`(() => {
        const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll')
        scroll.scrollLeft = Math.min(211, scroll.scrollWidth - scroll.clientWidth)
        scroll.scrollTop = Math.min(184, scroll.scrollHeight - scroll.clientHeight)
      })()`)
      await settleLayout(cdp)
      const before = await cdp.evaluate(viewportTableGeometry())
      assert.equal(before.table.scrollAxis, "both", `${width}x${height} dense table axis: ${JSON.stringify(before)}`)
      assert.equal(before.table.overflowY, "auto", `${width}x${height} dense vertical owner: ${JSON.stringify(before)}`)
      assert.equal(before.table.vertical, true, `${width}x${height} dense vertical overflow: ${JSON.stringify(before)}`)
      assert.ok(before.table.scrollHeight > before.table.clientHeight, `${width}x${height} dense scroll range: ${JSON.stringify(before)}`)
      await cdp.evaluate(`(() => {
        const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll')
        const row = [...document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row')].find((candidate) => {
          const bounds = candidate.getBoundingClientRect()
          const viewport = scroll.getBoundingClientRect()
          return bounds.top >= viewport.top && bounds.bottom <= viewport.bottom
        })
        row.click()
      })()`)
      await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null`, `${width}x${height} detail dock`, 15_000)
      await settleLayout(cdp)
      const opened = await cdp.evaluate(viewportDockGeometry())

      assert.equal(opened.selectedPid, before.visiblePid, `${width}x${height} selected activity row`)
      assert.ok(Math.abs(opened.table.scrollLeft - before.table.scrollLeft) <= 1, `${width}x${height} horizontal scroll on open: ${JSON.stringify({ before, opened })}`)
      assert.ok(Math.abs(opened.table.scrollTop - before.table.scrollTop) <= 1, `${width}x${height} vertical scroll on open: ${JSON.stringify({ before, opened })}`)
      assert.ok(Math.abs(opened.layout.height - before.layout.height) <= 1, `${width}x${height} outer row height: ${JSON.stringify({ before, opened })}`)
      assert.ok(Math.abs(opened.table.height - before.table.height) <= 1, `${width}x${height} table height: ${JSON.stringify({ before, opened })}`)
      if (width <= 1000) {
        assert.ok(Math.abs(opened.table.width - before.table.width) <= 1, `${width}x${height} overlay table width: ${JSON.stringify({ before, opened })}`)
      }
      assert.ok(opened.document.scrollHeight <= opened.document.clientHeight + 1, `${width}x${height} document height: ${JSON.stringify(opened)}`)
      assert.ok(opened.table.bottom <= opened.document.clientHeight + 1, `${width}x${height} reachable table rail: ${JSON.stringify(opened)}`)
      assert.ok(opened.table.scrollWidth > opened.table.clientWidth, `${width}x${height} horizontal table: ${JSON.stringify(opened)}`)
      assert.ok(opened.table.railHeight > 0, `${width}x${height} horizontal rail height: ${JSON.stringify(opened)}`)
      assert.ok(opened.dock.top >= -1 && opened.dock.bottom <= height + 1 && opened.dock.left >= -1 && opened.dock.right <= width + 1, `${width}x${height} dock bounds: ${JSON.stringify(opened)}`)
      const detailOverflows = opened.body.scrollHeight > opened.body.clientHeight

      await cdp.evaluate(`(() => { const body = document.querySelector('.inspector-body'); body.scrollTop = body.scrollHeight })()`)
      await settleLayout(cdp)
      const scrolled = await cdp.evaluate(viewportDockGeometry())
      if (detailOverflows) assert.ok(scrolled.body.scrollTop > 0, `${width}x${height} detail scroll: ${JSON.stringify(scrolled)}`)
      assert.ok(Math.abs(scrolled.table.scrollTop - before.table.scrollTop) <= 1, `${width}x${height} independent table scroll: ${JSON.stringify({ before, scrolled })}`)
      assert.ok(scrolled.header.top >= scrolled.dock.top - 1 && scrolled.header.bottom <= scrolled.dock.bottom + 1, `${width}x${height} sticky header: ${JSON.stringify(scrolled)}`)
      assert.ok(scrolled.close.top >= scrolled.dock.top - 1 && scrolled.close.bottom <= scrolled.dock.bottom + 1, `${width}x${height} visible close: ${JSON.stringify(scrolled)}`)
      assert.ok(scrolled.lastDetail.top >= scrolled.body.top - 1 && scrolled.lastDetail.bottom <= scrolled.body.bottom + 1, `${width}x${height} reachable detail fields: ${JSON.stringify(scrolled)}`)

      await cdp.evaluate(`([...document.querySelectorAll('.inspector-tabs button')].find((button) => button.textContent === 'Chart')).click()`)
      await cdp.waitFor(`document.querySelector('.inspector-chart-slot .uplot-figure') !== null`, `${width}x${height} detail chart`, 15_000)
      await settleLayout(cdp)
      const chartTab = await cdp.evaluate(`(() => {
        const body = document.querySelector('.inspector-body').getBoundingClientRect()
        const chart = document.querySelector('.inspector-chart-slot .uplot-figure').getBoundingClientRect()
        return { body: { bottom: body.bottom, left: body.left, right: body.right, top: body.top }, chart: { bottom: chart.bottom, height: chart.height, left: chart.left, right: chart.right, top: chart.top } }
      })()`)
      assert.ok(chartTab.chart.height >= 180 && chartTab.chart.height <= 220, `${width}x${height} chart cap: ${JSON.stringify(chartTab)}`)
      assert.ok(chartTab.chart.left >= chartTab.body.left - 1 && chartTab.chart.right <= chartTab.body.right + 1, `${width}x${height} chart inside the Inspector: ${JSON.stringify(chartTab)}`)

      await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') === null`, `${width}x${height} detail close`)
      const closed = await cdp.evaluate(viewportTableGeometry())
      assert.ok(Math.abs(closed.table.scrollLeft - before.table.scrollLeft) <= 1, `${width}x${height} horizontal scroll on close: ${JSON.stringify({ before, closed })}`)
      assert.ok(Math.abs(closed.table.scrollTop - before.table.scrollTop) <= 1, `${width}x${height} vertical scroll on close: ${JSON.stringify({ before, closed })}`)
      await cdp.evaluate(`(() => { const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll'); scroll.focus(); scroll.scrollLeft = scroll.scrollWidth })()`)
      const rail = await cdp.evaluate(`(() => { const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll'); return { active: document.activeElement === scroll, clientWidth: scroll.clientWidth, left: scroll.scrollLeft, scrollWidth: scroll.scrollWidth } })()`)
      assert.equal(rail.active, true, `${width}x${height} focusable rail`)
      assert.ok(rail.left >= rail.scrollWidth - rail.clientWidth - 1, `${width}x${height} rightmost columns: ${JSON.stringify(rail)}`)
      measurements.push({ height, width, before, opened, scrolled: { bodyScrollTop: scrolled.body.scrollTop, headerTop: scrolled.header.top } })
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
    process.stdout.write(`${JSON.stringify({ measurements }, null, 2)}\n`)
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("structured search pending state and snapshot targets preserve exact newest results", { timeout: 120_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  let statementAttempts = 0
  let relationAttempts = 0
  let pendingSystemFailure = null
  let pendingActivityFailure = null
  let pendingStatementFailure = null
  let pendingRelationFailure = null
  let pendingProcessSearch = null
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      return ndjson(response, url.searchParams.has("section") ? [] : snapshotTargetTimelineRecords())
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const at = Number(url.searchParams.get("at") ?? AT)
      const sections = url.searchParams.getAll("section")
      if (sections.includes("os_cpu")) {
        if (at === BEFORE_AT) { pendingSystemFailure = response; return }
        return ndjson(response, systemSnapshotRecords(false, at))
      }
      if (sections.includes("os_process")) {
        if (url.searchParams.get("search") === "cpu_cores>1") { pendingProcessSearch = response; return }
        return ndjson(response, snapshotRecords())
      }
      if (sections.includes("pg_stat_activity")) {
        if (at === BEFORE_AT) { pendingActivityFailure = response; return }
        return ndjson(response, targetedActivityRecords("activity_target_A", at))
      }
      if (sections.includes("pg_stat_statements")) {
        if (url.searchParams.get("search") === "target-b") {
          statementAttempts += 1
          if (statementAttempts === 1) { pendingStatementFailure = response; return }
          return ndjson(response, targetedStatementRecords("statement_target_B", 222))
        }
        return ndjson(response, targetedStatementRecords("statement_target_A", 111))
      }
      if (sections.includes("pg_stat_user_tables")) {
        if (url.searchParams.get("group") === "schema") {
          relationAttempts += 1
          if (relationAttempts === 1) { pendingRelationFailure = response; return }
          return ndjson(response, targetedRelationRecords(url, "relation_target_B", 444))
        }
        return ndjson(response, targetedRelationRecords(url, "relation_target_A", 333))
      }
      return ndjson(response, [])
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("snapshot-target browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the host ledger", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    // CPU, memory and the device panel answer on their own sections now.
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "ordinary System target A cpu", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-memory"]') !== null`, "the memory ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-memory"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-mem_available"]') !== null`, "ordinary System target A memory", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "the disk ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-panel-os_diskstats"]')?.textContent.includes("device_target_A") === true`, "ordinary System target A devices", 15_000)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "ordinary System target B")
    await waitForRequests(() => pendingSystemFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"] .loading-ring') !== null`, "ordinary System target B loading", 15_000)
    // The last successful rows and derived readings stay visible while their
    // same-surface replacement is unresolved.
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-metric-mem_available"]') !== null`), true)
    assert.equal(await cdp.evaluate(`(() => {
      const panel = document.querySelector('[data-testid="system-panel-os_diskstats"]')
      return panel !== null && panel.querySelectorAll(".entity-row").length > 0
        && panel.textContent.includes("device_target_A") && panel.textContent.includes("Previous rows remain visible")
    })()`), true)
    brokenNdjson(pendingSystemFailure)
    pendingSystemFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]')?.classList.contains("cursor-missing") === true`, "ordinary System target B error", 15_000)
    assert.equal(await cdp.evaluate(`(() => {
      const panel = document.querySelector('[data-testid="system-panel-os_diskstats"]')
      return document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null
        && document.querySelector('[data-testid="system-metric-mem_available"]') !== null
        && panel !== null && panel.textContent.includes("device_target_A")
        && panel.textContent.includes("Could not load rows. Previous rows remain visible.")
        && panel.querySelector('[role="alert"]') !== null
    })()`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"]') !== null`), true)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]')?.textContent.includes("activity_target_A") === true`, "ordinary PostgreSQL target A", 15_000)
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "ArrowLeft" }))`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${BEFORE_AT}"`, "ordinary PostgreSQL target B")
    await waitForRequests(() => pendingActivityFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"] .loading-ring') !== null`, "ordinary PostgreSQL target B loading", 15_000)
    assert.equal(await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-activity-table"]')
      return table !== null && table.textContent.includes("activity_target_A")
        && table.textContent.includes("Previous rows remain visible") && table.querySelector('.entity-row') !== null
    })()`), true)
    brokenNdjson(pendingActivityFailure)
    pendingActivityFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="cursor-behind"]')?.classList.contains("cursor-missing") === true`, "ordinary PostgreSQL target B error", 15_000)
    assert.equal(await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-activity-table"]')
      return table !== null && table.textContent.includes("activity_target_A")
        && table.textContent.includes("Could not load rows. Previous rows remain visible.")
        && table.querySelector('.entity-row') !== null && table.querySelector('[role="alert"]') !== null
    })()`), true)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"]') !== null`), true)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.statements` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"]')?.textContent.includes("statement_target_A") === true`, "dense statement target A", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 111/)
    const searchGeometry = async () => cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-statements-table"]')
      const status = table.querySelector('[data-testid="table-status"]')
      const scroll = table.querySelector('.entity-scroll')
      const filter = table.querySelector('[data-search-surface]')
      const rect = (node) => { const value = node.getBoundingClientRect(); return { bottom: value.bottom, height: value.height, left: value.left, right: value.right, top: value.top, width: value.width } }
      return {
        busy: table.getAttribute('aria-busy'),
        client: document.documentElement.clientWidth,
        filter: rect(filter),
        overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        scroll: rect(scroll),
        status: rect(status),
        table: rect(table),
      }
    })()`)
    const readySearchGeometry = new Map()
    for (const locale of ["en", "ru"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
        await settleLayout(cdp)
        readySearchGeometry.set(`${locale}:${width}`, await searchGeometry())
      }
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "target-b AND")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="table-filter"]')?.getAttribute("aria-invalid") === "true"`, "invalid search draft")
    assert.equal(pendingStatementFailure, null)
    assert.equal(statementAttempts, 0)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), null)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "target-b")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await waitForRequests(() => pendingStatementFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"] [role="status"]') !== null`, "dense statement target B search status", 15_000)
    const loadingStatement = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-statements-table"]')
      const request = table.querySelector('[data-testid="table-status"] [role]')
      return {
        busy: table.getAttribute('aria-busy'),
        empty: table.querySelector('.table-empty')?.textContent ?? null,
        live: request?.getAttribute('aria-live'),
        role: request?.getAttribute('role'),
        rows: table.querySelectorAll('.entity-row').length,
        status: table.querySelector('[data-testid="table-status"]').textContent,
        text: table.textContent,
      }
    })()`)
    assert.equal(loadingStatement.rows, 1)
    assert.equal(loadingStatement.busy, "true")
    assert.equal(loadingStatement.live, "polite")
    assert.equal(loadingStatement.role, "status")
    assert.equal(loadingStatement.empty, null)
    assert.match(loadingStatement.text, /statement_target_A/)
    assert.match(loadingStatement.status, /Searching… Rows retained/)
    assert.doesNotMatch(loadingStatement.status, /Loaded|111|0 of 0/)
    for (const locale of ["en", "ru"]) {
      await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
      for (const width of [360, 800, 1280]) {
        await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
        await settleLayout(cdp)
        const pending = await searchGeometry()
        const ready = readySearchGeometry.get(`${locale}:${width}`)
        assert.ok(Math.abs(pending.status.height - ready.status.height) <= 1, `${locale}:${width}:status ${JSON.stringify({ pending, ready })}`)
        assert.ok(Math.abs(pending.scroll.height - ready.scroll.height) <= 1, `${locale}:${width}:scroll ${JSON.stringify({ pending, ready })}`)
        assert.equal(pending.busy, "true", `${locale}:${width}:busy`)
        assert.equal(pending.overflow, false, `${locale}:${width}:overflow ${JSON.stringify(pending)}`)
        assert.ok(pending.filter.left >= -1 && pending.filter.right <= pending.client + 1, `${locale}:${width}:filter ${JSON.stringify(pending)}`)
        const statusText = await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]').textContent`)
        assert.match(statusText, locale === "ru" ? /Идёт поиск/ : /Searching/)
      }
    }
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    brokenNdjson(pendingStatementFailure)
    pendingStatementFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"] [role="alert"]') !== null`, "dense statement target B error", 15_000)
    const failedStatement = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-statements-table"]')
      return { rows: table.querySelectorAll('.entity-row').length, status: table.querySelector('[data-testid="table-status"]').textContent, text: table.textContent }
    })()`)
    assert.equal(failedStatement.rows, 1)
    assert.match(failedStatement.text, /statement_target_A/)
    assert.match(failedStatement.status, /Search failed. Rows retained./)
    assert.doesNotMatch(failedStatement.status, /Loaded|111/)
    await cdp.evaluate(`document.querySelector('[data-testid="table-paging"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-statements-table"]')?.textContent.includes("statement_target_B") === true`, "dense statement target B retry", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-statements-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 222/)

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=cpu&find=cpu_cores%3E1` })
    await waitForRequests(() => pendingProcessSearch !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="process-table"] [data-testid="table-status"] [role="status"]') !== null`, "initial Process search pending", 15_000)
    const emptyPendingProcess = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      return {
        busy: table.getAttribute('aria-busy'),
        empty: table.querySelector('.table-empty')?.textContent ?? null,
        rows: table.querySelectorAll('.entity-row').length,
        status: table.querySelector('[data-testid="table-status"]').textContent,
        text: table.textContent,
      }
    })()`)
    assert.equal(emptyPendingProcess.busy, "true")
    assert.equal(emptyPendingProcess.rows, 0)
    assert.match(emptyPendingProcess.empty, /Searching/)
    assert.match(emptyPendingProcess.status, /^Searching/)
    assert.doesNotMatch(emptyPendingProcess.text, /No rows match|Loaded 0 of 0/)
    ndjson(pendingProcessSearch, snapshotRecords())
    pendingProcessSearch = null
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 0 && document.querySelector('[data-testid="process-table"] [data-testid="table-status"] [role]') === null`, "initial Process search success", 15_000)

    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null`, "PostgreSQL surface before Activity", 15_000)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Activity'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]') !== null`, "remembered Activity surface", 15_000)
    await cdp.evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="process-tab"]')?.getAttribute('aria-current') === 'page'`, "Processes before scoped search", 15_000)
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, "cpu_cores>1")
      input.dispatchEvent(new Event("input", { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await waitForRequests(() => pendingProcessSearch !== null)
    ndjson(pendingProcessSearch, snapshotRecords())
    pendingProcessSearch = null
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "cpu_cores>1" && document.querySelector('[data-testid="process-table"] [data-testid="table-status"] [role]') === null`, "applied Process expression", 15_000)
    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("view") === "pg.activity" && document.querySelector('[data-testid="pg-activity-table"]') !== null`, "Process to Activity navigation", 15_000)
    const activityNavigation = await cdp.evaluate(`(() => ({
      error: document.querySelector('[data-testid="search-error"]')?.textContent ?? null,
      find: new URL(location.href).searchParams.get('find'),
      input: document.querySelector('[data-testid="table-filter"]')?.value ?? null,
    }))()`)
    assert.deepEqual(activityNavigation, { error: null, find: null, input: "" })
    await cdp.evaluate(`history.back()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get("find") === "cpu_cores>1" && document.querySelector('[data-testid="process-tab"]')?.getAttribute('aria-current') === 'page'`, "Back restoring Process expression", 15_000)
    await waitForRequests(() => pendingProcessSearch !== null)
    assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("find")`), "cpu_cores>1")
    ndjson(pendingProcessSearch, snapshotRecords())
    pendingProcessSearch = null

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.tables` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"]')?.textContent.includes("relation_target_A") === true`, "relation target A", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 333/)
    await cdp.evaluate(`document.querySelector('nav.lensbar .lens-tabs button:nth-child(2)').click()`)
    await waitForRequests(() => pendingRelationFailure !== null)
    await cdp.waitFor(`document.querySelector('[data-testid="table-paging"] button')?.textContent === "…"`, "relation target B loading", 15_000)
    const loadingRelation = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-tables-table"]')
      return { rows: table.querySelectorAll('.entity-row').length, status: table.querySelector('[data-testid="table-status"]').textContent, text: table.textContent }
    })()`)
    assert.equal(loadingRelation.rows, 0)
    assert.doesNotMatch(loadingRelation.text, /relation_target_A/)
    assert.doesNotMatch(loadingRelation.status, /333/)
    assert.match(loadingRelation.status, /Loading rows/)
    brokenNdjson(pendingRelationFailure)
    pendingRelationFailure = null
    await cdp.waitFor(`document.querySelector('[data-testid="table-paging"] button')?.textContent === "↻"`, "relation target B error", 15_000)
    const failedRelation = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="pg-tables-table"]')
      return { rows: table.querySelectorAll('.entity-row').length, status: table.querySelector('[data-testid="table-status"]').textContent, text: table.textContent }
    })()`)
    assert.equal(failedRelation.rows, 0)
    assert.doesNotMatch(failedRelation.text, /relation_target_A/)
    assert.doesNotMatch(failedRelation.status, /333/)
    assert.match(failedRelation.status, /Could not load rows/)
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] [role="alert"]') !== null`), true)
    await cdp.evaluate(`document.querySelector('[data-testid="table-paging"] button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-tables-table"]')?.textContent.includes("relation_target_B") === true`, "relation target B retry", 15_000)
    assert.match(await cdp.evaluate(`document.querySelector('[data-testid="pg-tables-table"] [data-testid="table-status"]').textContent`), /Loaded 1 of 444/)

    assert.equal(statementAttempts, 2)
    assert.equal(relationAttempts, 2)
    assert.deepEqual(page.errors.filter((message) => !message.includes("kronika: snapshot at the cursor failed") && !message.includes("kronika: snapshot page failed")), [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("recorded environment owns every rail and Inspector across cold routes and Back", { timeout: 120_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  let container = true
  let heldLanes = null
  let holdLanes = true
  const hostKeys = ["cpu_busy", "cpu_stall", "memory", "io_stall"]
  const containerKeys = ["cg_cpu_share", "cg_cpu_psi", "cg_memory", "cg_io_psi"]
  const lanes = () => [
    ...timelineRecords(HOUR, container),
    ...[["cpu_busy", 88], ["cpu_stall", 12], ["memory", 77], ["io_stall", 9], ["pg_running", 3], ["pg_waiting", 1]].map(([lane, value]) => ({ record: "lane", segment_id: SEGMENT, lane, ts: String(AT), value })),
  ]
  const page = { errors: [], external: [], responses: [] }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") { response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" }); response.end(html); return }
    if (url.pathname === "/auth/session") return answerSession(request, response, { valid: true })
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/events") return ndjson(response, slowQueryEventRecords())
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/hour") {
      if (url.searchParams.has("section")) return ndjson(response, [])
      if (url.searchParams.get("part") === "lanes" && holdLanes) { heldLanes = response; return }
      return ndjson(response, lanes())
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = new Set(url.searchParams.getAll("section"))
      const all = [...snapshotRecords(), ...systemSnapshotRecords(container)]
      const types = new Set(all.filter((record) => record.record === "layout" && sections.has(record.layout.logical_name)).map((record) => record.layout.type_id))
      return ndjson(response, all.filter((record) => record.record === "layout" ? types.has(record.layout.type_id) : record.record === "row" && types.has(record.type_id)))
    }
    response.writeHead(404); response.end()
  })
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const origin = `http://127.0.0.1:${server.address().port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    socket = await pageSocket(await browserDebugPort(profile, browser))
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 1280 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    const previewKeys = `[...document.querySelectorAll('[data-testid="timeline-preview-metric-select"] option')].map((option) => option.value)`
    const inspectorKeys = `[...document.querySelectorAll('[data-testid="timeline-metric-select"] option')].map((option) => option.value)`
    const verify = async (inspector = true) => {
      const expected = container ? containerKeys : hostKeys
      const excluded = container ? hostKeys : containerKeys
      await cdp.waitFor(`${JSON.stringify(expected)}.every((key) => (${previewKeys}).includes(key))`, "the recorded scope in the preview", 15_000)
      assert.deepEqual((await cdp.evaluate(previewKeys)).filter((key) => excluded.includes(key)), [])
      const cpu = container ? "25%" : "88%"
      const memory = container ? "40%" : "77%"
      const readings = await cdp.evaluate(`[...document.querySelectorAll('.timeline-lane-name')].map((name) => [name.textContent, name.closest('.lane-label').querySelector('[data-testid="lane-reading"]').textContent])`)
      assert.equal(readings.find(([name]) => name === "CPU")?.[1], cpu)
      assert.equal(readings.find(([name]) => name === "Memory")?.[1], memory)
      if (!inspector) return
      await cdp.evaluate(`document.querySelector('.timeline-open-chart').click()`)
      await cdp.waitFor(`${JSON.stringify(expected)}.every((key) => (${inspectorKeys}).includes(key))`, "the recorded scope in the Inspector", 15_000)
      assert.deepEqual((await cdp.evaluate(inspectorKeys)).filter((key) => excluded.includes(key)), [])
      await cdp.evaluate(`document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`)
    }
    for (const environment of [true, false]) {
      container = environment
      for (const route of ["events", "postgresql.activity"]) {
        holdLanes = true
        await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=${route}` })
        await waitForRequests(() => heldLanes !== null)
        assert.deepEqual((await cdp.evaluate(previewKeys)).filter((key) => [...hostKeys, ...containerKeys].includes(key)), [], "unknown scope draws no resource substitute")
        holdLanes = false
        ndjson(heldLanes, lanes()); heldLanes = null
        await verify()
      }
      for (const [index, label] of ["Host", "Processes", "Events", "PostgreSQL", "Processes"].entries()) {
        await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent === ${JSON.stringify(label)})).click()`)
        await verify(index < 4)
        if (label === "Host") {
          await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 1 })
          const targets = await cdp.evaluate(`[...document.querySelectorAll('button.use-verdict')].map((button) => { const rect = button.getBoundingClientRect(); return [rect.width, rect.height] })`)
          assert.ok(targets.length > 0)
          assert.ok(targets.every(([width, height]) => width >= 44 && height >= 44), JSON.stringify(targets))
        }
      }
      await cdp.evaluate(`history.back()`)
      await cdp.waitFor(`document.querySelector('.source-tabs button[aria-current="page"]')?.textContent === "PostgreSQL"`, "Back restores PostgreSQL")
      await verify()
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    if (heldLanes !== null) { heldLanes.end(); heldLanes = null }
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("production health keeps staggered components on one stored evaluation", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const page = { errors: [], external: [], responses: [] }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      if (url.searchParams.has("section")) return ndjson(response, [])
      return ndjson(response, healthContractTimeline(Number(url.searchParams.get("from") ?? HOUR)))
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      const sections = url.searchParams.getAll("section")
      return ndjson(response, sections.includes("instance_metadata")
        ? healthMetadataRecords(Number(url.searchParams.get("at") ?? AT))
        : productionSystemSnapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("health contract server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 768, mobile: false, width: 1024 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })

    const readAt = async (hour, fragments) => {
      const evaluation = hour + 1_800_000_000
      await cdp.send("Page.navigate", { url: `${origin}/?at=${evaluation}&view=host.overview` })
      await cdp.waitFor(`document.querySelector('[data-primary] [data-testid="lane-reading"]') !== null`, "the health contract reading", 15_000)
      await cdp.waitFor(`(() => {
        const text = document.querySelector('[data-primary] [data-testid="lane-reading"]')?.textContent ?? ''
        return ${JSON.stringify(fragments)}.every((fragment) => text.includes(fragment))
      })()`, "the expected health component values", 15_000)
      const reading = await cdp.evaluate(`document.querySelector('[data-primary] [data-testid="lane-reading"]').textContent`)
      return reading
    }

    const fresh = await readAt(HOUR, ["Overall 70%", "OS 80%", "PostgreSQL 90%"])
    assert.match(fresh, /Overall[^\d]*70%/)
    assert.match(fresh, /OS[^\d]*80%/)
    assert.match(fresh, /PostgreSQL[^\d]*90%/)

    const staleHour = HOUR + HOUR_US
    const stale = await readAt(staleHour, ["Overall —", "OS 80%", "PostgreSQL —"])
    assert.match(stale, /Overall[^\d]*—/)
    assert.match(stale, /OS[^\d]*80%/)
    assert.match(stale, /PostgreSQL[^\d]*—/)
    assert.doesNotMatch(stale, /PostgreSQL[^\d]*90%/)

    const disabledHour = HOUR + 2 * HOUR_US
    const disabled = await readAt(disabledHour, ["Overall 84%", "OS 84%"])
    assert.match(disabled, /Overall[^\d]*84%/)
    assert.match(disabled, /OS[^\d]*84%/)
    assert.doesNotMatch(disabled, /PostgreSQL/)
    assert.equal(await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].disabled`), false)
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("production System projections show exact CPU memory and device readings", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const requests = []
  const page = { errors: [], external: [], responses: [] }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    requests.push(requestRecord(request, url))
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const section = url.searchParams.get("section")
      return ndjson(response, section === null ? productionSystemTimeline() : productionSystemHistoryRecords(url))
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, productionSystemSnapshotRecords())
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("System contract server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 1280 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the host ledger", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU chip", 15_000)

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition"] .u-over') !== null`, "the CPU contract chart")
    await waitForRequests(() => requests.some(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_cpu"))
    await cdp.evaluate(`document.querySelector('[data-testid="system-cpu-composition-all"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-cpu-composition-all"]')?.getAttribute("aria-pressed") === "true"`, "the whole CPU contract composition")
    await delay(100)
    await hoverContractChart(cdp, "system-cpu-composition")
    const cpu = await cdp.evaluate(`document.querySelector('[data-testid="system-cpu-composition"] .chart-tooltip').textContent`)
    for (const expected of [
      /CPU used, cores[^\d]*0\.9/, /CPU cores[^\d]*2/, /User CPU[^\d]*25%/,
      /System CPU[^\d]*10%/, /I\/O wait[^\d]*5%/, /Steal[^\d]*5%/,
    ]) assert.match(cpu, expected)

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-memory"]') !== null`, "the memory ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-memory"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-mem_available"]') !== null`, "the memory chip", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-memory"]') !== null`, "the memory ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-memory"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-mem_available"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-memory-composition"] .u-over') !== null`, "the memory contract chart")
    await waitForRequests(() => requests.some(({ path, query }) => path === "/api/hour" && new URLSearchParams(query).get("section") === "os_meminfo"))
    await cdp.evaluate(`document.querySelector('[data-testid="system-memory-composition-all"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-memory-composition-all"]')?.getAttribute("aria-pressed") === "true"`, "the whole memory contract composition")
    await delay(100)
    await hoverContractChart(cdp, "system-memory-composition")
    const memoryTooltip = await cdp.evaluate(`document.querySelector('[data-testid="system-memory-composition"] .chart-tooltip').textContent`)
    for (const expected of [
      /MemTotal[^\d]*1 MiB/, /AnonPages[^\d]*256 KiB/, /Page cache[^\d]*192 KiB/,
      /Reclaimable slab[^\d]*64 KiB/, /Unreclaimable slab[^\d]*32 KiB/, /MemFree[^\d]*128 KiB/,
      /Other memory[^\d]*352 KiB/,
    ]) assert.match(memoryTooltip, expected)

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "the disk ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="system-os_diskstats"] .entity-row').length === 2`, "the two projected devices", 15_000)
    const devices = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="system-os_diskstats"]')
      const headers = [...table.querySelectorAll('[role="columnheader"]')].map((cell) => cell.querySelector('.entity-sort span')?.textContent.trim() ?? '')
      return [...table.querySelectorAll('.entity-row')].map((row) => Object.fromEntries(
        [...row.querySelectorAll('[role="cell"]')].map((cell, index) => [headers[index], cell.textContent.trim()]),
      ))
    })()`)
    const sda = devices.find((device) => device.Device === "sda")
    const sdb = devices.find((device) => device.Device === "sdb")
    assert.equal(sda["Major:minor"], "8:0")
    assert.equal(sda["Read latency"], "5 ms")
    assert.equal(sda["Write latency"], "7 ms")
    assert.equal(sdb["Major:minor"], "8:1")
    assert.equal(sdb["Read latency"], "—")
    assert.equal(sdb["Write latency"], "—")

    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="system-os_diskstats"] .entity-row')].find((row) => row.textContent.includes('sda'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-os_diskstats-detail"] dl > .detail-row') !== null`, "the device Detail facts")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-os_diskstats-detail"] .uplot-host') === null`), true)
    await cdp.evaluate(`document.querySelectorAll('.inspector-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot [data-testid="system-os_diskstats-history"]') !== null`, "the device history Chart")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') === null`), true)
    assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('.timeline-preview').getBoundingClientRect().height`) - 124) <= .5)
    await cdp.evaluate(`([...document.querySelectorAll('[data-testid="system-os_diskstats-history"] .system-history-selector button')].find((button) => button.textContent.includes('Read latency'))).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-os_diskstats-history"] .chart-current')?.textContent === "5 ms"`, "the selected read-latency value", 15_000)
    await hoverContractChart(cdp, "system-os_diskstats-history")
    const diskTooltip = await cdp.evaluate(`document.querySelector('[data-testid="system-os_diskstats-history"] .chart-tooltip').textContent`)
    assert.match(diskTooltip, /Read latency[^\d]*5 ms/)
    const diskHistory = requests.map(({ path, query }) => ({ path, query: new URLSearchParams(query) }))
      .find(({ path, query }) => path === "/api/hour" && query.get("section") === "os_diskstats" && query.getAll("field").includes("read_time_ms"))
    assert.notEqual(diskHistory, undefined)
    assert.equal(diskHistory.query.get("where.major"), "8")
    assert.equal(diskHistory.query.get("where.minor"), "0")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

function healthContractTimeline(hour) {
  const evaluation = hour + 1_800_000_000
  const scenario = Math.round((hour - HOUR) / HOUR_US)
  const postgresql = scenario !== 2
  const postgresAt = evaluation - (scenario === 0 ? 5_000_000 : 15_000_000)
  const health = scenario === 2
    ? [
        { record: "point", type_id: "0", series: "os_health", ts: String(evaluation), identity: {}, value: 84 },
        { record: "point", type_id: "0", series: "overall_health", ts: String(evaluation), identity: {}, value: 84 },
      ]
    : [
        { record: "point", type_id: "0", series: "postgres_health", ts: String(postgresAt), identity: {}, value: 90 },
        { record: "point", type_id: "0", series: "os_health", ts: String(evaluation), identity: {}, value: 80 },
        { record: "point", type_id: "0", series: "overall_health", ts: String(evaluation), identity: {}, value: scenario === 0 ? 70 : null },
      ]
  return [
    { record: "hour", from: String(hour), to: String(hour + HOUR_US - 1), available_hours: [HOUR, HOUR + HOUR_US, HOUR + 2 * HOUR_US].map(String) },
    {
      record: "catalog", from: String(hour), to: String(hour + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: postgresql, present: postgresql, metrics_present: postgresql }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(hour), max_ts: String(evaluation),
      sections: [
        { logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001", implementation: "linux", source_family: "system", rows: "3", bytes: "384" },
        ...(postgresql ? [{ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256" }] : []),
      ],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    ...health,
  ]
}

function healthMetadataRecords(at) {
  return [
    {
      record: "layout", rates: [],
      layout: { type_id: "1000001", logical_name: "instance_metadata", columns: [{ name: "postgresql_interval_seconds" }] },
    },
    { record: "row", segment_id: SEGMENT, type_id: "1000001", ordinal: "0", timestamp: String(at), values: [10] },
  ]
}

function productionSystemTimeline() {
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    {
      record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: false, present: false, metrics_present: false }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AT),
      sections: [
        { logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001", implementation: "linux", source_family: "system", rows: "6", bytes: "768" },
        { logical_name: "os_meminfo", physical_name: "os_meminfo", type_id: "1104001", implementation: "linux", source_family: "system", rows: "2", bytes: "256" },
        { logical_name: "os_diskstats", physical_name: "os_diskstats", type_id: "1108001", implementation: "linux", source_family: "system", rows: "4", bytes: "512" },
      ],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(AT), identity: {}, value: 88 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(AT), identity: {}, value: 88 },
  ]
}

function productionSystemSnapshotRecords(at = AT) {
  const cpuColumns = ["ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal", "scope"]
  const cpuRates = ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"]
  const memoryColumns = ["ts", "mem_available", "mem_total", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim", "swap_free", "swap_total"]
  const diskColumns = ["ts", "major", "minor", "device", "reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_in_progress", "io_time_ms", "io_weighted_time_ms", "scope"]
  const diskRates = ["reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_time_ms", "io_weighted_time_ms"]
  return [
    { record: "layout", rates: cpuRates, layout: { type_id: "1102001", logical_name: "os_cpu", columns: cpuColumns.map((name) => ({ name })) } },
    row("1102001", "cpu-all", [String(at), -1, 20, 5, 10, 50, 5, 2, 3, 5, 0], at),
    row("1102001", "cpu-0", [String(at), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    row("1102001", "cpu-1", [String(at), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    { record: "layout", rates: [], layout: { type_id: "1104001", logical_name: "os_meminfo", columns: memoryColumns.map((name) => ({ name })) } },
    row("1104001", "memory", [String(at), 512, 1024, 128, 128, 64, 256, 64, 32, 64, 128], at),
    { record: "layout", rates: diskRates, layout: { type_id: "1108001", logical_name: "os_diskstats", columns: diskColumns.map((name) => ({ name })) } },
    row("1108001", "disk-8-0", [String(at), 8, 0, "sda", 4, 2, 16, 24, 20, 14, 1, 300, 600, 0], at),
    row("1108001", "disk-8-1", [String(at), 8, 1, "sdb", 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
  ]
}

function productionSystemHistoryRecords(url) {
  const section = url.searchParams.get("section")
  const before = AT - 5_000_000
  if (section === "os_cpu") {
    const columns = ["ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal", "scope"]
    return [
      { record: "series_segment", segment: { id: SEGMENT } },
      { record: "layout", rates: [], layout: { type_id: "1102001", logical_name: section, columns: columns.map((name) => ({ name })) } },
      row("1102001", "cpu-all-before", [String(before), -1, 100, 20, 50, 500, 10, 5, 5, 10, 0], before),
      row("1102001", "cpu-0-before", [String(before), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], before),
      row("1102001", "cpu-1-before", [String(before), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], before),
      row("1102001", "cpu-all-current", [String(AT), -1, 120, 25, 60, 550, 15, 7, 8, 15, 0], AT),
      row("1102001", "cpu-0-current", [String(AT), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], AT),
      row("1102001", "cpu-1-current", [String(AT), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], AT),
    ]
  }
  if (section === "os_meminfo") {
    const columns = ["ts", "mem_available", "mem_total", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim", "swap_free", "swap_total"]
    const values = [512, 1024, 128, 128, 64, 256, 64, 32, 64, 128]
    return [
      { record: "series_segment", segment: { id: SEGMENT } },
      { record: "layout", rates: [], layout: { type_id: "1104001", logical_name: section, columns: columns.map((name) => ({ name })) } },
      row("1104001", "memory-before", [String(before), ...values], before),
      row("1104001", "memory-current", [String(AT), ...values], AT),
    ]
  }
  if (section === "os_diskstats") {
    const columns = ["ts", "major", "minor", "device", "reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_in_progress", "io_time_ms", "io_weighted_time_ms", "scope"]
    const major = Number(url.searchParams.get("where.major") ?? 8)
    const minor = Number(url.searchParams.get("where.minor") ?? 0)
    const current = minor === 0
      ? [String(AT), major, minor, "sda", 104, 202, 1016, 2024, 1020, 514, 1, 1300, 2600, 0]
      : [String(AT), major, minor, "sdb", 100, 200, 1000, 2000, 1000, 500, 0, 1000, 2000, 0]
    return [
      { record: "series_segment", segment: { id: SEGMENT } },
      { record: "layout", rates: [], layout: { type_id: "1108001", logical_name: section, columns: columns.map((name) => ({ name })) } },
      row("1108001", "disk-before", [String(before), major, minor, minor === 0 ? "sda" : "sdb", 100, 200, 1000, 2000, 1000, 500, 0, 1000, 2000, 0], before),
      row("1108001", "disk-current", current, AT),
    ]
  }
  return []
}

async function hoverContractChart(cdp, testId) {
  await cdp.evaluate(`(() => {
    const plot = document.querySelector('[data-testid="${testId}"] .u-over')
    const bounds = plot.getBoundingClientRect()
    const clientX = bounds.right - 1
    const clientY = bounds.top + bounds.height / 2
    plot.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, clientX, clientY }))
    plot.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX, clientY }))
  })()`)
  await cdp.waitFor(`document.querySelector('[data-testid="${testId}"] .chart-tooltip') !== null`, `${testId} tooltip`)
}

function processSummaryRecords(hour, count, processes) {
  const fields = [
    "processes", "threads", "runnable", "postgresql", "user_cores", "system_cores", "run_delay_ms_per_second", "context_switches_per_second",
    "resident_kib", "virtual_kib", "swap_kib", "major_faults_per_second", "read_bytes_per_second", "write_bytes_per_second", "read_calls_per_second", "write_calls_per_second",
  ]
  const values = [processes, processes * 2, 2, 1, 1.5, 0.5, 12, 33, 1024, 2048, 0, 1, 4096, 8192, 3, 4]
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    layout("0", "os_process_summary", fields),
    ...Array.from({ length: count }, (_, index) => ({
      record: "row", type_id: "0", ordinal: String(index), timestamp: String(hour + Math.min(3_590_000_000, (index + 1) * Math.max(5_000_000, Math.floor(3_590_000_000 / (count + 1))))), values,
    })),
  ]
}

function viewportActivityTimeline() {
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    { record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1), source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }] },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AFTER_AT),
      sections: [{ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "120", bytes: "4096" }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(AT), identity: {}, value: 81 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(AT), identity: {}, value: 76 },
    { record: "lane", segment_id: SEGMENT, lane: "pg_waiting", ts: String(AT), value: 4 },
  ]
}

function viewportActivityRows(url) {
  const selected = url.searchParams.get("where.pid")
  const [activityLayout, template] = activityFixtureSeed()
  const rows = Array.from({ length: 120 }, (_, index) => viewportActivityRow(template, 3_000 + index, AT, index))
    .filter((record) => selected === null || String(record.values[1]) === selected)
  return [activityLayout, ...rows]
}

function viewportActivityHistory(url) {
  const pid = Number(url.searchParams.get("where.pid") ?? "3000")
  const [activityLayout, template] = activityFixtureSeed()
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    activityLayout,
    ...Array.from({ length: 24 }, (_, index) => {
      const timestamp = HOUR + (index + 1) * 120_000_000
      return viewportActivityRow(template, pid, timestamp, index)
    }),
  ]
}

function activityFixtureSeed() {
  const records = snapshotRecords().filter((record) => (record.record === "layout" ? record.layout.type_id : record.type_id) === "1001004")
  return [records.find(({ record }) => record === "layout"), records.find(({ record }) => record === "row")]
}

function viewportActivityRow(template, pid, timestamp, index) {
  const query = `select pg_sleep(0.01), payload from operator_activity where pid = ${pid} and payload = '${"activity-detail-".repeat(24)}'`
  const values = [
    String(timestamp), pid, 1, 20, "operator_database", "operator_role", "viewport-regression", "192.0.2.42", "client backend",
    "active", "Lock", "transactionid", query, String(9_000_000 + pid), 42 + index, 21 + index,
    String(timestamp - 3_000_000_000), String(timestamp - 900_000_000), String(timestamp - 180_000_000), String(timestamp - 60_000_000),
  ]
  return { ...template, ordinal: String(pid), timestamp: String(timestamp), values }
}

function viewportTableGeometry() {
  return `(() => {
    const layout = document.querySelector('[data-testid="pg-entity-layout"]')
    const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll')
    const visible = [...document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row')].find((row) => {
      const bounds = row.getBoundingClientRect()
      const viewport = scroll.getBoundingClientRect()
      return bounds.top >= viewport.top && bounds.bottom <= viewport.bottom
    })
    const rect = (node) => { const bounds = node.getBoundingClientRect(); return { bottom: bounds.bottom, height: bounds.height, left: bounds.left, right: bounds.right, top: bounds.top, width: bounds.width } }
    const style = getComputedStyle(scroll)
    return {
      document: { clientHeight: document.documentElement.clientHeight, scrollHeight: document.documentElement.scrollHeight },
      layout: rect(layout),
      table: { ...rect(scroll), clientHeight: scroll.clientHeight, clientWidth: scroll.clientWidth, overflowY: style.overflowY, railHeight: scroll.offsetHeight - scroll.clientHeight, scrollAxis: scroll.dataset.scrollAxis, scrollHeight: scroll.scrollHeight, scrollLeft: scroll.scrollLeft, scrollTop: scroll.scrollTop, scrollWidth: scroll.scrollWidth, vertical: scroll.scrollHeight > scroll.clientHeight + 1 },
      visiblePid: visible?.querySelector('[role="cell"]')?.textContent.trim() ?? null,
    }
  })()`
}

function viewportDockGeometry() {
  return `(() => {
    const base = ${viewportTableGeometry()}
    const dock = document.querySelector('[data-testid="inspector"]')
    const body = dock.querySelector('.inspector-body')
    const header = dock.querySelector('.inspector-head')
    const close = dock.querySelector('.inspector-close')
    const chart = body.querySelector('.uplot-figure')
    const zero = { bottom: 0, height: 0, left: 0, right: 0, top: 0, width: 0 }
    const lastDetail = body.querySelector('[data-testid="pg-detail"] dl > div:last-child')
    const rect = (node) => { const bounds = node.getBoundingClientRect(); return { bottom: bounds.bottom, height: bounds.height, left: bounds.left, right: bounds.right, top: bounds.top, width: bounds.width } }
    return {
      ...base,
      body: { ...rect(body), clientHeight: body.clientHeight, scrollHeight: body.scrollHeight, scrollTop: body.scrollTop },
      chart: chart === null ? zero : rect(chart),
      close: rect(close),
      dock: rect(dock),
      header: rect(header),
      lastDetail: rect(lastDetail),
      selectedPid: document.querySelector('[data-testid="pg-activity-table"] .entity-row[aria-selected="true"] [role="cell"]')?.textContent.trim() ?? null,
    }
  })()`
}

function sparsePostgresGeometry() {
  return `(() => {
    const activity = document.querySelector('[data-testid="pg-entity-layout"]')
    const activityScroll = activity.querySelector('.entity-scroll')
    const workspace = document.querySelector('.workspace')
    const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
    const measured = (root, scroll) => {
      const scrollBox = rect(scroll)
      const style = getComputedStyle(scroll)
      const rows = [...root.querySelectorAll('.entity-row')].map(rect)
      return {
        ...rect(root),
        allRowsFit: rows.every((row) => row.top >= scrollBox.top - .5 && row.bottom <= scrollBox.top + scroll.clientHeight + .5),
        clientHeight: scroll.clientHeight,
        contentSized: root.dataset.contentSized === 'true',
        horizontal: scroll.scrollWidth > scroll.clientWidth,
        overflowX: style.overflowX,
        overflowY: style.overflowY,
        rows,
        scrollBox,
        scrollAxis: scroll.dataset.scrollAxis,
        scrollHeight: scrollBox.height,
        scrollContentHeight: scroll.scrollHeight,
        scrollTop: scroll.scrollTop,
        vertical: /(auto|scroll)/.test(style.overflowY) && scroll.scrollHeight > scroll.clientHeight + 1,
      }
    }
    const activityRect = measured(activity, activityScroll)
    return {
      activity: activityRect,
      workspace: rect(workspace),
    }
  })()`
}

function sparsePostgresSeamGeometry() {
  return `(() => {
    const table = document.querySelector('[data-testid="pg-activity-table"]')
    const scroll = table.querySelector('.entity-scroll')
    const header = table.querySelector('.entity-head [role="columnheader"]:last-child')
    const cell = table.querySelector('.entity-row [role="cell"]:last-child')
    const splitter = document.querySelector('.inspector-splitter')
    const trigger = document.querySelector('.timeline-open-chart')
    const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
    const scrollRect = rect(scroll)
    const splitterRect = rect(splitter)
    const clientRight = scrollRect.left + scroll.clientLeft + scroll.clientWidth
    const style = getComputedStyle(scroll)
    const rows = [...table.querySelectorAll('.entity-row')].map(rect)
    return {
      allRowsFit: rows.every((row) => row.top >= scrollRect.top - .5 && row.bottom <= scrollRect.top + scroll.clientHeight + .5),
      axis: scroll.dataset.scrollAxis,
      cellEndGap: clientRight - rect(cell).right,
      chartToSplitter: splitterRect.left - rect(trigger).right,
      clientHeight: scroll.clientHeight,
      clientRight,
      headerEndGap: clientRight - rect(header).right,
      horizontal: scroll.scrollWidth > scroll.clientWidth + 1,
      overflowX: style.overflowX,
      overflowY: style.overflowY,
      panel: new URL(location.href).searchParams.get('panel'),
      scroll: scrollRect,
      scrollHeight: scroll.scrollHeight,
      scrollLeft: scroll.scrollLeft,
      scrollWidth: scroll.scrollWidth,
      seamGap: splitterRect.left - scrollRect.right,
      splitter: splitterRect,
      vertical: /(auto|scroll)/.test(style.overflowY) && scroll.scrollHeight > scroll.clientHeight + 1,
    }
  })()`
}

function sourceTimelineRecords(historical) {
  const sections = [{ logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001", implementation: "linux", source_family: "system", rows: "1", bytes: "128" }]
  if (historical) sections.push({ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256" })
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    { record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1), source_families: [{ name: "postgresql", configured: false, present: historical, metrics_present: historical }] },
    { record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(AFTER_AT), sections },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "overall_health", ts: String(AT), identity: {}, value: 73 },
    { record: "point", type_id: "0", series: "os_health", ts: String(AT), identity: {}, value: 73 },
    ...systemIndexRecords(String(AT)),
  ]
}

function slowQueryTimelineRecords() {
  return [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    {
      record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(EVENT_SCOPE_AFTER),
      sections: [
        {
          logical_name: "pg_log_slow_queries", physical_name: "pg_log_slow_queries", type_id: "2004001",
          implementation: "postgresql", source_family: "postgresql", rows: "3", bytes: "512",
        },
        {
          logical_name: "os_mountinfo", physical_name: "os_mountinfo", type_id: "1112002",
          implementation: "linux", source_family: "system", rows: "1", bytes: "128",
        },
        {
          logical_name: "pg_locks", physical_name: "pg_locks", type_id: "1011002",
          implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "128",
        },
      ],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "lane", segment_id: SEGMENT, lane: "pg_running", ts: String(AT), value: 1 },
    {
      record: "finding", logical_name: "pg_log_slow_queries", kind: "event", type_id: "2004001",
      field_ordinal: 0, row_ordinal: "3", ts: String(AT),
    },
    {
      record: "finding", logical_name: "pg_log_slow_queries", kind: "event", type_id: "2004001",
      field_ordinal: 0, row_ordinal: "4", ts: String(EVENT_SCOPE_AT),
    },
    {
      record: "finding", logical_name: "pg_log_slow_queries", kind: "known_bad", type_id: "2004001",
      field_ordinal: 0, row_ordinal: "4", ts: String(EVENT_SCOPE_AT),
    },
    {
      record: "finding", logical_name: "os_mountinfo", kind: "known_bad", type_id: "1112002",
      field_ordinal: 9, row_ordinal: "8", ts: String(EVENT_SCOPE_AT),
    },
    {
      record: "finding", logical_name: "pg_log_slow_queries", kind: "event", type_id: "2004001",
      field_ordinal: 0, row_ordinal: "5", ts: String(EVENT_SCOPE_AFTER),
    },
    {
      record: "finding", logical_name: "pg_locks", kind: "known_bad", type_id: "1011002",
      field_ordinal: 2, row_ordinal: "9", ts: String(EVENT_SCOPE_AFTER),
    },
  ]
}

function slowQueryEventRecords(from = HOUR) {
  const minutes = Array.from({ length: 60 }, () => 0)
  const selected = from === EVENT_SCOPE_AT
  if (selected) minutes[0] = 2
  else {
    minutes[Math.floor((AT - HOUR) / 60_000_000)] = 1
    minutes[Math.floor((EVENT_SCOPE_AT - HOUR) / 60_000_000)] = 2
  }
  return [
    { record: "events", representation: "groups", truncated: true },
    {
      record: "event_group",
      key: `slow:${SLOW_PATTERN}`,
      section: "pg_log_slow_queries",
      tier: "notable",
      label: SLOW_PATTERN,
      count: selected ? 2 : 3,
      firstTs: selected ? EVENT_SCOPE_AT : AT,
      lastTs: EVENT_SCOPE_AFTER,
      minutes,
      stat: { kind: "pg.slow", maxMs: 6_290, totalMs: 12_580, thresholdMs: null },
      detail_ref: SLOW_DETAIL_REF,
      representativeTs: selected ? EVENT_SCOPE_AT : AT,
    },
  ]
}

function slowQueryDetailRecords() {
  return [{
    record: "row_detail",
    section: "pg_log_slow_queries",
    at: String(AT),
    fields: {
      sample: {
        stored_text: SLOW_STORED_TEXT,
        full_len: String(SLOW_STORED_TEXT.length),
        truncated: false,
        sha256: null,
      },
    },
  }]
}

function expansionGeometryExpression() {
  return `(() => {
    const chips = [...document.querySelectorAll('[data-testid="event-entry-facts"] > span')].map((chip) => {
      const [label, output] = chip.children
      const labelBox = label.getBoundingClientRect()
      const valueBox = output.getBoundingClientRect()
      return {
        gap: Math.round(valueBox.left - labelBox.right),
        height: Math.round(chip.getBoundingClientRect().height),
        label: label.textContent.trim(),
        sameRow: Math.abs(valueBox.top - labelBox.top) <= 4,
        text: output.textContent.trim(),
      }
    })
    return {
      chips,
      clientWidth: document.documentElement.clientWidth,
      innerWidth: window.innerWidth,
      rawRows: document.querySelectorAll('[data-testid="event-raw-row"]').length,
      scrollWidth: document.documentElement.scrollWidth,
      text: document.querySelector('[data-testid="events-console"]')?.textContent ?? "",
    }
  })()`
}

// At or below 520 px the preview grows by the 44 px cursor row, which is the
// only way to step the cursor without a keyboard.
// The network row is the only ledger cell that prints two readings, so it is
// the only one that can name one series and show two.
function networkLaneRecords(hour) {
  const at = hour + (AT - HOUR)
  return [
    { record: "lane", segment_id: SEGMENT, lane: "net_rx", ts: String(at), value: 884 },
    { record: "lane", segment_id: SEGMENT, lane: "net_tx", ts: String(at), value: 888 },
  ]
}

function previewHeight(width) {
  return width <= 520 ? 67 : 124
}

async function settleLayout(cdp) {
  await cdp.evaluate("document.fonts.ready.then(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))")
}

async function assertCompactTimelineContained(cdp, followingSelector, label) {
  for (const width of [800, 1280]) {
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
    let geometry = null
    for (let attempt = 0; attempt < 3; attempt += 1) {
      await settleLayout(cdp)
      const pointer = await cdp.evaluate(`(() => {
        const plot = document.querySelector('[data-testid="hour-timeline"] .u-over').getBoundingClientRect()
        return { x: plot.right - 1, y: plot.top + plot.height / 2 }
      })()`)
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...pointer })
      await delay(50)
      geometry = await cdp.evaluate(`(() => {
        const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top } }
        const figure = document.querySelector('[data-testid="hour-timeline"]')
        const shell = figure.closest('.timeline-shell')
        const plot = figure.querySelector('.u-over')
        const cursor = figure.querySelector('.u-cursor-x')
        return {
          axes: [...figure.querySelectorAll('.u-axis')].map(bounds),
          cursor: cursor === null ? null : bounds(cursor),
          figure: { ...bounds(figure), height: figure.getBoundingClientRect().height },
          following: bounds(document.querySelector(${JSON.stringify(followingSelector)})),
          plot: bounds(plot),
          rightReserve: figure.getBoundingClientRect().right - plot.getBoundingClientRect().right,
          shell: bounds(shell),
        }
      })()`)
      if (geometry.cursor === null) break
    }
    assert.ok(geometry.figure.height >= 92 && geometry.figure.height <= 96, `${label} ${width}px compact figure: ${JSON.stringify(geometry)}`)
    for (const axis of geometry.axes) {
      assert.ok(axis.left >= geometry.figure.left - 1 && axis.right <= geometry.figure.right + 1
        && axis.top >= geometry.figure.top - 1 && axis.bottom <= geometry.figure.bottom + 1,
      `${label} ${width}px axis containment: ${JSON.stringify(geometry)}`)
    }
    assert.equal(geometry.cursor, null, `${label} ${width}px shared native cursor suppressed: ${JSON.stringify(geometry)}`)
    assert.ok(geometry.rightReserve >= 28, `${label} ${width}px final-label reserve: ${JSON.stringify(geometry)}`)
    assert.ok(geometry.following.top >= geometry.shell.bottom - 1, `${label} ${width}px following-region overlap: ${JSON.stringify(geometry)}`)
    await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 1, y: 1 })
    await delay(20)
  }
}

async function assertDetailRowsDoNotOverlap(cdp, label) {
  for (const locale of ["ru", "en"]) {
    await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${label} ${locale} locale`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
      await settleLayout(cdp)
      const geometry = await cdp.evaluate(`(() => {
        const detail = document.querySelector('[data-testid="pg-detail"]')
        const box = detail.getBoundingClientRect()
        const rows = [...detail.querySelectorAll('dl > .detail-row')].map((row) => {
          const term = row.querySelector('dt').getBoundingClientRect()
          const value = row.querySelector('dd').getBoundingClientRect()
          const horizontal = Math.min(term.right, value.right) - Math.max(term.left, value.left)
          const vertical = Math.min(term.bottom, value.bottom) - Math.max(term.top, value.top)
          return { horizontal, term: { left: term.left, right: term.right }, value: { left: value.left, right: value.right }, vertical }
        })
        return { detail: { left: box.left, right: box.right }, rows, scroll: document.documentElement.scrollWidth, viewport: document.documentElement.clientWidth }
      })()`)
      assert.ok(geometry.rows.length > 0, `${label} ${locale} ${width}px rows`)
      assert.equal(geometry.rows.some(({ horizontal, vertical }) => horizontal > 0.5 && vertical > 0.5), false,
        `${label} ${locale} ${width}px overlap: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.rows.every(({ term, value }) => term.left >= geometry.detail.left - 1 && term.right <= geometry.detail.right + 1
        && value.left >= geometry.detail.left - 1 && value.right <= geometry.detail.right + 1),
      `${label} ${locale} ${width}px containment: ${JSON.stringify(geometry)}`)
      assert.ok(geometry.scroll <= geometry.viewport, `${label} ${locale} ${width}px document overflow: ${JSON.stringify(geometry)}`)
    }
  }
}

async function assertSearchControlContained(cdp, label, selector = '[data-testid="events-console"] [data-search-surface="events"]') {
  for (const locale of ["ru", "en"]) {
    await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${label} ${locale} locale`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width })
      await settleLayout(cdp)
      const closed = await cdp.evaluate(`(() => {
        const root = document.querySelector(${JSON.stringify(selector)})
        const parts = [root.querySelector('form'), root.querySelector('input'), root.querySelector('[aria-label="' + (${JSON.stringify(locale)} === 'ru' ? 'Применить поиск' : 'Apply search') + '"]'), root.querySelector('[aria-expanded]')]
        return {
          bounds: parts.map((part) => { const box = part.getBoundingClientRect(); return { left: box.left, right: box.right } }),
          scroll: document.documentElement.scrollWidth,
          viewport: document.documentElement.clientWidth,
        }
      })()`)
      assert.ok(closed.bounds.every(({ left, right }) => left >= -1 && right <= closed.viewport + 1)
        && closed.scroll <= closed.viewport, `${label} ${locale} ${width}px control: ${JSON.stringify(closed)}`)
      await cdp.evaluate(`document.querySelector(${JSON.stringify(selector)}).querySelector('[aria-expanded]').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="search-help"] [role="dialog"]') !== null`, `${label} ${locale} ${width}px help`)
      const opened = await cdp.evaluate(`(() => {
        const dialog = document.querySelector('[data-testid="search-help"] [role="dialog"]')
        const box = dialog.getBoundingClientRect()
        return { active: dialog.contains(document.activeElement), bottom: box.bottom, left: box.left, right: box.right, top: box.top, viewportHeight: innerHeight, viewportWidth: innerWidth }
      })()`)
      assert.ok(opened.active && opened.left >= -1 && opened.right <= opened.viewportWidth + 1
        && opened.top >= -1 && opened.bottom <= opened.viewportHeight + 1,
      `${label} ${locale} ${width}px help containment: ${JSON.stringify(opened)}`)
      await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
      await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 })
      await cdp.waitFor(`document.querySelector('[data-testid="search-help"]') === null`, `${label} ${locale} ${width}px help close`)
    }
  }
}

async function assertSearchChipHierarchy(cdp, label) {
  const geometry = await cdp.evaluate(`(() => {
    const root = document.querySelector('[data-testid="search-chips"]')
    const predicates = [...root.querySelectorAll('[data-search-predicate]')]
    const syntax = [...root.querySelectorAll('[data-search-syntax]')]
    const bounds = (node) => {
      const rect = node.getBoundingClientRect()
      return { bottom: rect.bottom, height: rect.height, top: rect.top }
    }
    const predicateStyle = getComputedStyle(predicates[0])
    const syntaxStyles = syntax.map((token) => getComputedStyle(token))
    const children = [...root.children]
    const lineCenters = []
    for (const child of children) {
      const rect = child.getBoundingClientRect()
      const center = rect.top + rect.height / 2
      if (!lineCenters.some((candidate) => Math.abs(candidate - center) <= 1)) lineCenters.push(center)
    }
    const predicateHeight = predicates[0].getBoundingClientRect().height
    const rowGap = Number.parseFloat(getComputedStyle(root).rowGap)
    return {
      allChildrenAreElements: [...root.childNodes].every((node) => node.nodeType === Node.ELEMENT_NODE),
      childHeights: children.map((child) => child.getBoundingClientRect().height),
      lineCount: lineCenters.length,
      predicate: {
        color: predicateStyle.color,
        fontSize: Number.parseFloat(predicateStyle.fontSize),
        height: predicateHeight,
      },
      rootHeight: root.getBoundingClientRect().height,
      rowGap,
      syntax: syntax.map((token, index) => ({
        background: syntaxStyles[index].backgroundColor,
        border: [syntaxStyles[index].borderTopWidth, syntaxStyles[index].borderRightWidth, syntaxStyles[index].borderBottomWidth, syntaxStyles[index].borderLeftWidth],
        color: syntaxStyles[index].color,
        fontSize: Number.parseFloat(syntaxStyles[index].fontSize),
        geometry: bounds(token),
        kind: token.getAttribute("data-search-syntax"),
        text: token.textContent,
      })),
      text: root.textContent,
    }
  })()`)
  assert.equal(geometry.allChildrenAreElements, true, `${label}: ${JSON.stringify(geometry)}`)
  assert.match(geometry.text, /^\(tablespace: fast_ssdORtablespace: archive\)AND\(.+OR.+\)$/)
  assert.deepEqual(geometry.syntax.map(({ text }) => text), ["(", "OR", ")", "AND", "(", "OR", ")"], `${label}: ${JSON.stringify(geometry)}`)
  assert.deepEqual(geometry.syntax.map(({ kind }) => kind), ["parenthesis", "connector", "parenthesis", "connector", "parenthesis", "connector", "parenthesis"], `${label}: ${JSON.stringify(geometry)}`)
  assert.equal(geometry.predicate.fontSize, 11, `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ fontSize }) => fontSize < geometry.predicate.fontSize), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ color }) => color !== geometry.predicate.color), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ background, border }) => background === "rgba(0, 0, 0, 0)" && border.every((width) => width === "0px")), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.syntax.every(({ geometry: token }) => Math.abs(token.height - geometry.predicate.height) <= 0.5), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.childHeights.every((height) => height <= geometry.predicate.height + 0.5), `${label}: ${JSON.stringify(geometry)}`)
  assert.ok(geometry.rootHeight <= geometry.lineCount * geometry.predicate.height + Math.max(geometry.lineCount - 1, 0) * geometry.rowGap + 1, `${label}: ${JSON.stringify(geometry)}`)
}

async function assertSearchChipHierarchyMatrix(cdp, label) {
  for (const locale of ["ru", "en"]) {
    await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === ${JSON.stringify(locale)}`, `${label} ${locale} locale`)
    for (const width of [360, 800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: width === 360 ? 640 : 768, mobile: false, width })
      await settleLayout(cdp)
      await assertSearchChipHierarchy(cdp, `${label} ${locale} ${width}px`)
    }
  }
}

test("forensic workstation keeps exact preview and one responsive Inspector", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const [activityLayout, activityTemplate] = activityFixtureSeed()
  const forensicSnapshots = [
    ...snapshotRecords().filter((record) => (record.record === "layout" ? record.layout.type_id : record.type_id) !== "1001004"),
    activityLayout,
    ...Array.from({ length: 12 }, (_, index) => viewportActivityRow(activityTemplate, 3_000 + index, AT, index)),
    ...progressVacuumRecords(),
    ...statementRecords(false),
  ]
  const forensicTimeline = [
    ...timelineRecords(HOUR, true).map((record) => record.record !== "finished_segment" ? record : {
      ...record,
      sections: [...record.sections, {
        logical_name: "pg_stat_progress_vacuum", physical_name: "pg_stat_progress_vacuum", type_id: "1012006",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }],
    }),
    { record: "lane_context", segment_id: SEGMENT, postgresql_interval_seconds: null, environment: 0 },
    { record: "lane", segment_id: SEGMENT, lane: "cpu_busy", ts: String(AT), value: 54 },
    { record: "lane", segment_id: SEGMENT, lane: "pg_running", ts: String(AT), value: 3 },
    { record: "lane", segment_id: SEGMENT, lane: "pg_waiting", ts: String(AT), value: 1 },
  ]
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end(JSON.stringify({ record: "instance_label", database: "artifact_db" }))
      return
    }
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      const section = url.searchParams.get("section")
      if (section === "os_process_summary") return ndjson(response, processSummaryRecords(HOUR, 3, 80))
      if (section === "os_process") return ndjson(response, forensicSnapshots)
      if (section === "pg_stat_progress_vacuum") return ndjson(response, vacuumHourRecords(url))
      return ndjson(response, section === null ? [...forensicTimeline, {
        record: "finding", logical_name: "pg_log_errors", kind: "event", type_id: "1009001",
        field_ordinal: 1, row_ordinal: "1", ts: String(AFTER_AT),
      }] : [])
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, forensicSnapshots)
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("forensic workstation server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    const viewports = [
      { height: 800, kind: "desktop", mobile: false, width: 1280 },
      { height: 900, kind: "tablet", mobile: false, width: 800 },
      { height: 800, kind: "phone", mobile: true, width: 360 },
    ]
    for (const viewport of viewports) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: viewport.height, mobile: viewport.mobile, width: viewport.width })
      await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&lens=cpu` })
      await cdp.waitFor(`document.querySelector('.process-summary-inline > div:first-child strong')?.textContent === "1.5"`, `${viewport.kind} process summary`, 15_000)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"] canvas') !== null && document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, `${viewport.kind} workstation`)
      await settleLayout(cdp)
      const closed = await cdp.evaluate(`(() => {
        const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width } }
        const topbar = document.querySelector('.topbar')
        const top = bounds(topbar)
        const painted = [...topbar.children].filter((node) => getComputedStyle(node).display !== 'none').map(bounds)
        const scroll = document.querySelector('[data-testid="process-table"] .entity-scroll')
        const scrollRect = scroll.getBoundingClientRect()
        const visibleRows = [...document.querySelectorAll('[data-testid="process-table"] .entity-row')].filter((row) => {
          const rect = row.getBoundingClientRect()
          return rect.bottom > scrollRect.top && rect.top < scrollRect.bottom
        }).length
        return {
          clocks: [...document.querySelectorAll('[data-testid="cursor-time"], [data-testid="cursor-row-time"], .snapshot-time')].filter((node) => {
            const rect = node.getBoundingClientRect()
            return getComputedStyle(node).display !== 'none' && rect.width > 0 && rect.height > 0
          }).map((node) => node.textContent.trim()),
          documentOverflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
          inspector: document.querySelector('[data-testid="inspector"]') !== null,
          painted,
          paintedInside: painted.every((rect) => rect.left >= top.left - .5 && rect.right <= top.right + .5 && rect.top >= top.top - .5 && rect.bottom <= top.bottom + .5),
          preview: bounds(document.querySelector('.timeline-preview')),
          scroll: bounds(scroll),
          top,
          visibleRows,
        }
      })()`)
      assert.equal(closed.inspector, false, viewport.kind)
      assert.equal(closed.clocks.length, 1, `${viewport.kind} owns one cursor clock: ${JSON.stringify(closed.clocks)}`)
      assert.match(closed.clocks[0], /^\d{2}:\d{2}:\d{2}$/)
      assert.equal(closed.documentOverflow, false, `${viewport.kind}: ${JSON.stringify(closed)}`)
      assert.equal(closed.paintedInside, true, `${viewport.kind}: ${JSON.stringify(closed)}`)
      assert.ok(Math.abs(closed.preview.height - previewHeight(viewport.width)) <= .5, `${viewport.kind}: ${JSON.stringify(closed.preview)}`)
      for (const locale of ["ru", "en"]) {
        await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
        await cdp.waitFor(`document.documentElement.lang === "${locale}"`, `${viewport.kind} ${locale} clock`)
        assert.equal(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="cursor-time"], [data-testid="cursor-row-time"], .snapshot-time')].filter((node) => { const rect = node.getBoundingClientRect(); return getComputedStyle(node).display !== 'none' && rect.width > 0 && rect.height > 0 }).length`), 1, `${viewport.kind} ${locale} charts-off clock`)
      }

      const cursorRow = await cdp.evaluate(`(() => {
        const row = document.querySelector('[data-testid="cursor-row"]')
        if (row === null || getComputedStyle(row).display === 'none') return { shown: false }
        const reading = row.querySelector('[data-testid="cursor-row-reading"]')
        const buttons = [...row.querySelectorAll('button')].map((button) => {
          const box = button.getBoundingClientRect()
          return { disabled: button.disabled, height: box.height, label: button.getAttribute('aria-label'), width: box.width }
        })
        const fits = (node) => node.scrollWidth <= node.clientWidth
        return {
          buttons,
          height: row.getBoundingClientRect().height,
          readingFits: fits(reading), readingText: reading.textContent,
          shown: true,
        }
      })()`)
      if (viewport.width > 520) {
        assert.equal(cursorRow.shown, false, `${viewport.kind} has the plot and the arrow keys: ${JSON.stringify(cursorRow)}`)
      } else {
        assert.equal(cursorRow.shown, true, `${viewport.kind}: ${JSON.stringify(cursorRow)}`)
        assert.equal(cursorRow.buttons.length, 2, JSON.stringify(cursorRow))
        assert.equal(cursorRow.height, 66, JSON.stringify(cursorRow))
        assert.ok(cursorRow.buttons.every((button) => button.height >= 44 && button.width >= 44), `${viewport.kind} cursor steps stay tappable: ${JSON.stringify(cursorRow)}`)
        assert.equal(cursorRow.readingFits, true, `${viewport.kind} lane reading must not clip: ${JSON.stringify(cursorRow)}`)
        assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="cursor-row-time"]')`), null)
        assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="timeline-preview-reading"]') === null || getComputedStyle(document.querySelector('[data-testid="timeline-preview-reading"]')).display === 'none'`), true, `${viewport.kind} keeps one reading, not a clipped copy`)
        const before = await cdp.evaluate(`new URL(location.href).searchParams.get('at')`)
        await cdp.evaluate(`document.querySelector('[data-testid="cursor-row"] button:first-of-type').click()`)
        await cdp.waitFor(`new URL(location.href).searchParams.get('at') !== ${JSON.stringify(before)}`, `${viewport.kind} cursor stepped back`)
        const stepped = await cdp.evaluate(`new URL(location.href).searchParams.get('at')`)
        assert.ok(Number(stepped) < Number(before), `${viewport.kind} stepped to ${stepped} from ${before}`)
        await cdp.evaluate(`document.querySelector('[data-testid="cursor-row"] button:last-of-type').click()`)
        await cdp.waitFor(`new URL(location.href).searchParams.get('at') === ${JSON.stringify(before)}`, `${viewport.kind} cursor stepped forward`)

        // The health split is the reading both roles route on, and the widest
        // one the formatter can produce. RU spaces its percent sign, so its
        // saturated form is the worst case at any width.
        await cdp.evaluate(`(() => {
          const select = document.querySelector('[data-testid="timeline-preview-metric-select"]')
          select.value = 'health'
          select.dispatchEvent(new Event('change', { bubbles: true }))
        })()`)
        await cdp.waitFor(`document.querySelector('[data-testid="cursor-row-reading"]')?.textContent.includes('Overall')`, `${viewport.kind} health split`)
        await settleLayout(cdp)
        const health = await cdp.evaluate(`(() => {
          const reading = document.querySelector('[data-testid="cursor-row-reading"]')
          const style = getComputedStyle(reading)
          const context = document.createElement('canvas').getContext('2d')
          context.font = style.fontWeight + ' ' + style.fontSize + ' ' + style.fontFamily
          return {
            clipped: reading.scrollWidth > reading.clientWidth,
            room: reading.clientWidth,
            saturated: context.measureText('Overall 100 % · OS 100 % · PostgreSQL 100 %').width,
            text: reading.textContent,
          }
        })()`)
        // Wrapped, the four summary facts cost four lines; scrolled, they cost
        // one and every fact is still reachable.
        const summary = await cdp.evaluate(`(() => {
          const summary = document.querySelector('.process-summary-inline')
          const stamp = document.querySelector('.lensbar > .snapshot-time')
          const facts = [...summary.children]
          return {
            duplicateStamp: stamp !== null && getComputedStyle(stamp).display !== 'none',
            facts: facts.length,
            head: document.querySelector('[data-testid="process-table"] .entity-head').getBoundingClientRect().top,
            height: summary.getBoundingClientRect().height,
            lensbar: document.querySelector('.lensbar').getBoundingClientRect().height,
            reachable: summary.scrollWidth <= summary.clientWidth || summary.scrollWidth <= summary.clientWidth + summary.scrollLeftMax || getComputedStyle(summary).overflowX === 'auto',
          }
        })()`)
        assert.ok(summary.facts >= 4, `${viewport.kind} keeps every summary fact: ${JSON.stringify(summary)}`)
        assert.ok(summary.height <= 32, `${viewport.kind} summary must stay one row: ${JSON.stringify(summary)}`)
        assert.equal(summary.reachable, true, `${viewport.kind} summary must scroll, not clip: ${JSON.stringify(summary)}`)
        assert.equal(summary.duplicateStamp, false, `${viewport.kind} prints the cursor instant once: ${JSON.stringify(summary)}`)
        assert.ok(summary.head <= 400, `${viewport.kind} table head sits at ${summary.head}`)

        assert.match(health.text, /Overall .* OS .* PostgreSQL/, JSON.stringify(health))
        assert.equal(health.clipped, false, `${viewport.kind} health split clips: ${JSON.stringify(health)}`)
        assert.ok(health.saturated <= health.room, `${viewport.kind} saturated health split needs ${health.saturated} of ${health.room}`)
      }

      await cdp.evaluate(`document.querySelector('[data-testid="process-table"] .entity-row').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="inspector"][data-panel="detail"]') !== null`, `${viewport.kind} Detail Inspector`)
      await settleLayout(cdp)
      const opened = await cdp.evaluate(`(() => {
        const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, height: rect.height, left: rect.left, right: rect.right, top: rect.top, width: rect.width } }
        const inspector = bounds(document.querySelector('[data-testid="inspector"]'))
        const scroll = document.querySelector('[data-testid="process-table"] .entity-scroll')
        const scrollRect = scroll.getBoundingClientRect()
        const visibleRows = [...document.querySelectorAll('[data-testid="process-table"] .entity-row')].filter((row) => {
          const rect = row.getBoundingClientRect()
          return rect.bottom > scrollRect.top && rect.top < scrollRect.bottom
        }).length
        const query = new URL(location.href).searchParams
        return { body: bounds(document.querySelector('.inspector-body')), inspector, panel: query.get('panel'), row: query.get('row'), scroll: bounds(scroll), visibleRows }
      })()`)
      assert.notEqual(opened.row, null, viewport.kind)
      assert.equal(opened.panel, null, viewport.kind)
      if (viewport.kind === "desktop") {
        assert.ok(Math.abs(opened.inspector.width - 360) <= 1, JSON.stringify(opened))
        assert.ok(Math.abs(opened.scroll.height - closed.scroll.height) <= 1, JSON.stringify({ closed, opened }))
        assert.equal(opened.visibleRows, closed.visibleRows, JSON.stringify({ closed, opened }))
      } else if (viewport.kind === "tablet") {
        assert.ok(Math.abs(opened.inspector.width - 384) <= 1, JSON.stringify(opened))
        assert.ok(Math.abs(opened.scroll.width - closed.scroll.width) <= 1, JSON.stringify({ closed, opened }))
      } else {
        assert.ok(Math.abs(opened.inspector.width - viewport.width) <= 1, JSON.stringify(opened))
        assert.ok(opened.inspector.height <= 480.5, JSON.stringify(opened))
        assert.ok(opened.inspector.height >= 300 && opened.body.height >= 250, JSON.stringify(opened))
      }
      await cdp.evaluate(`([...document.querySelectorAll('.inspector-tabs button')].find((button) => button.textContent === 'Chart')).click()`)
      await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === 'chart' && document.querySelector('[data-testid="inspector-chart"] .inspector-chart-slot [data-testid="process-history"]') !== null`, `${viewport.kind} entity Chart Inspector`)
      await settleLayout(cdp)
      assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector-chart"] [data-testid="timeline-metric-select"]') === null`), true, `${viewport.kind} entity chart replaces the shared timeline`)
      assert.ok(Math.abs(await cdp.evaluate(`document.querySelector('.timeline-preview').getBoundingClientRect().height`) - previewHeight(viewport.width)) <= .5, `${viewport.kind} preview keeps its figure beside the entity chart`)
      await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="inspector"]') === null`, `${viewport.kind} Inspector closed`)
      await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
      await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === 'chart' && document.querySelector('[data-testid="inspector-chart"] canvas') !== null`, `${viewport.kind} Chart Inspector`)
      await settleLayout(cdp)
      for (const locale of ["ru", "en"]) {
        await cdp.evaluate(`document.querySelector('[data-testid="locale-${locale}"]').click()`)
        await cdp.waitFor(`document.documentElement.lang === "${locale}"`, `${viewport.kind} ${locale} chart clock`)
        assert.equal(await cdp.evaluate(`[...document.querySelectorAll('[data-testid="cursor-time"], [data-testid="cursor-row-time"], .snapshot-time')].filter((node) => { const rect = node.getBoundingClientRect(); return getComputedStyle(node).display !== 'none' && rect.width > 0 && rect.height > 0 }).length`), 1, `${viewport.kind} ${locale} charts-on clock`)
      }
      const chartGeometry = await cdp.evaluate(`(() => {
        const dock = document.querySelector('[data-testid="inspector"]')
        const body = dock.querySelector('.inspector-body')
        const header = dock.querySelector('.inspector-head')
        const title = header.querySelector('strong')
        const rail = body.querySelector('.timeline-rail')
        const picker = body.querySelector('.timeline-metric-picker')
        const select = body.querySelector('[data-testid="timeline-metric-select"]')
        const figure = body.querySelector('.uplot-figure')
        const preview = document.querySelector('.timeline-preview')
        const previewRail = preview.querySelector('.timeline-rail')
        const previewLanes = preview.querySelector('.timeline-lanes')
        const previewPicker = preview.querySelector('.timeline-preview-picker')
        const previewTrigger = preview.querySelector('.timeline-open-chart')
        const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
        const bodyRect = rect(body)
        const headerRect = rect(header)
        const next = [...select.options].find((option) => option.value !== select.value)?.value ?? select.value
        select.value = next
        select.dispatchEvent(new Event('change', { bubbles: true }))
        return {
          body: { ...bodyRect, clientWidth: body.clientWidth, overflowX: getComputedStyle(body).overflowX, scrollbarGutter: getComputedStyle(body).scrollbarGutter, scrollWidth: body.scrollWidth },
          figure: rect(figure),
          header: headerRect,
          headerChildrenInside: [...header.children].every((child) => { const box = child.getBoundingClientRect(); return box.left >= headerRect.left - .5 && box.right <= headerRect.right + .5 && box.top >= headerRect.top - .5 && box.bottom <= headerRect.bottom + .5 }),
          laneButtons: body.querySelectorAll('.lane-label').length,
          optionCount: select.options.length,
          picker: rect(picker),
          preview: {
            ...rect(preview),
            action: rect(previewTrigger),
            actionAtEdge: Math.abs(previewTrigger.getBoundingClientRect().right - previewRail.getBoundingClientRect().right) <= .5,
            labelsInside: getComputedStyle(previewLanes).display === 'none' || [...previewLanes.children].every((child) => { const box = child.getBoundingClientRect(); const parent = previewLanes.getBoundingClientRect(); return box.left >= parent.left - .5 && box.right <= parent.right + .5 }),
            lanesDisplay: getComputedStyle(previewLanes).display,
            ownerCount: [preview, ...preview.querySelectorAll('*')].filter((node) => /(auto|scroll)/.test(getComputedStyle(node).overflowX) && node.scrollWidth > node.clientWidth + 1).length,
            pickerDisplay: getComputedStyle(previewPicker).display,
            selectedAccess: previewLanes.querySelector('[data-primary="true"]')?.getAttribute('title') ?? previewPicker.querySelector('[data-testid="timeline-preview-reading"]')?.getAttribute('title'),
          },
          rail: rect(rail),
          selected: next,
          titleFits: title.scrollWidth <= title.clientWidth + 1 && title.scrollHeight <= title.clientHeight + 1,
        }
      })()`)
      await cdp.waitFor(`document.querySelector('[data-testid="timeline-metric-select"]').value === ${JSON.stringify(chartGeometry.selected)}`, `${viewport.kind} metric selection`)
      assert.equal(chartGeometry.body.overflowX, "hidden", `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.match(chartGeometry.body.scrollbarGutter, /stable/, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.body.scrollWidth <= chartGeometry.body.clientWidth + 1, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.equal(chartGeometry.headerChildrenInside, true, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.equal(chartGeometry.titleFits, true, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.optionCount > 1, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.equal(chartGeometry.laneButtons, 0, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.picker.left >= chartGeometry.body.left - .5 && chartGeometry.picker.right <= chartGeometry.body.left + chartGeometry.body.clientWidth + .5, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(chartGeometry.figure.right <= chartGeometry.body.left + chartGeometry.body.clientWidth + .5, `${viewport.kind}: ${JSON.stringify(chartGeometry)}`)
      assert.ok(Math.abs(chartGeometry.preview.height - 30) <= .5, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.equal(chartGeometry.preview.ownerCount, 0, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.equal(chartGeometry.preview.actionAtEdge, true, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.equal(chartGeometry.preview.labelsInside, true, `${viewport.kind}: ${JSON.stringify(chartGeometry.preview)}`)
      assert.match(chartGeometry.preview.selectedAccess, /\S/)
      if (viewport.kind === "phone") {
        assert.equal(chartGeometry.preview.lanesDisplay, "none", JSON.stringify(chartGeometry.preview))
        // The rail folds below 520px: the plot and its lane picker open at full
        // size from the control bar, which is the only chart affordance there.
        assert.equal(await cdp.evaluate(`getComputedStyle(document.querySelector('.timeline-preview .timeline-rail')).display`), "none")
        assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="mobile-controls"] button').length`), 2)
        assert.equal(await cdp.evaluate(`getComputedStyle(document.querySelector('[data-testid="mobile-controls"]')).display`), "flex")
      } else {
        assert.equal(chartGeometry.preview.lanesDisplay, "flex", JSON.stringify(chartGeometry.preview))
        assert.equal(chartGeometry.preview.pickerDisplay, "none", JSON.stringify(chartGeometry.preview))
        assert.ok(Math.abs(chartGeometry.preview.action.width - 64) <= 1, JSON.stringify(chartGeometry.preview))
      }
      const legend = await cdp.evaluate(`(() => {
        const figure = document.querySelector('[data-testid="inspector-chart"] .uplot-figure')
        const labels = figure.querySelector('.chart-series-labels')
        const current = figure.querySelector('.chart-current')
        const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
        const labelRect = rect(labels)
        const currentRect = rect(current)
        return {
          childrenInside: [...labels.children].every((child) => { const box = child.getBoundingClientRect(); return box.left >= labelRect.left - .5 && box.right <= labelRect.right + .5 }),
          current: currentRect,
          currentInside: currentRect.left >= rect(figure).left - .5 && currentRect.right <= rect(figure).right + .5,
          labels: labelRect,
          overflowX: getComputedStyle(labels).overflowX,
          rowsSeparated: labelRect.bottom <= currentRect.top + .5,
          scrollWidth: labels.scrollWidth,
          clientWidth: labels.clientWidth,
        }
      })()`)
      assert.equal(legend.overflowX, "hidden", `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.ok(legend.scrollWidth <= legend.clientWidth + 1, `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.equal(legend.childrenInside, true, `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.equal(legend.currentInside, true, `${viewport.kind}: ${JSON.stringify(legend)}`)
      assert.equal(legend.rowsSeparated, true, `${viewport.kind}: ${JSON.stringify(legend)}`)
      const fixedHeader = await cdp.evaluate(`(() => { const body = document.querySelector('.inspector-body'); const header = document.querySelector('.inspector-head'); const before = header.getBoundingClientRect().top; body.scrollTop = body.scrollHeight; return { after: header.getBoundingClientRect().top, before } })()`)
      assert.ok(Math.abs(fixedHeader.after - fixedHeader.before) <= .5, `${viewport.kind}: ${JSON.stringify(fixedHeader)}`)
      await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="inspector"]') === null && new URL(location.href).searchParams.get('row') === null && new URL(location.href).searchParams.get('panel') === null`, `${viewport.kind} closed Inspector`)
    }
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=processes&lens=tree` })
    await cdp.waitFor(`document.querySelector('[data-testid="lens-tree"]')?.getAttribute('aria-pressed') === 'true' && document.querySelectorAll('[data-testid="process-table"] .entity-row').length > 10`, "the Process Tree")
    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await cdp.waitFor(`document.documentElement.lang === 'ru'`, "the Russian Process Tree")
    await settleLayout(cdp)
    const treeGeometry = await cdp.evaluate(`(() => {
      const table = document.querySelector('[data-testid="process-table"]')
      const headers = [...table.querySelectorAll('[role="columnheader"]')]
      const names = headers.map((header) => header.querySelector('.entity-sort span')?.textContent.trim() ?? '')
      const pid = headers[names.indexOf('PID')]
      const command = headers[names.indexOf('Command')]
      const user = headers[names.indexOf('User')]
      const state = headers[names.indexOf('STAT')]
      const start = headers[names.indexOf('START')]
      const row = [...table.querySelectorAll('.entity-row')].find((candidate) => candidate.textContent.includes('2686712'))
      const cells = [...row.querySelectorAll('[role="cell"]')]
      const startValue = cells[names.indexOf('START')].querySelector('span')
      const tabs = document.querySelector('.process-workspace > .lensbar > .lens-tabs').getBoundingClientRect()
      const summary = document.querySelector('.process-summary-inline')
      const summaryRect = summary.getBoundingClientRect()
      return {
        divider: getComputedStyle(summary).borderLeftWidth,
        gap: summaryRect.left - tabs.right,
        pidWidth: pid.getBoundingClientRect().width,
        commandWidth: command.getBoundingClientRect().width,
        stateWidth: state.getBoundingClientRect().width,
        start: { clipped: startValue.scrollWidth > startValue.clientWidth + 1, text: startValue.textContent, width: start.getBoundingClientRect().width },
        userWidth: user.getBoundingClientRect().width,
      }
    })()`)
    assert.ok(Math.abs(treeGeometry.pidWidth - 82) <= 1, JSON.stringify(treeGeometry))
    assert.ok(Math.abs(treeGeometry.commandWidth - 620) <= 1, JSON.stringify(treeGeometry))
    assert.ok(Math.abs(treeGeometry.userWidth - 88) <= 1, JSON.stringify(treeGeometry))
    assert.ok(Math.abs(treeGeometry.stateWidth - 62) <= 1, JSON.stringify(treeGeometry))
    assert.ok(Math.abs(treeGeometry.start.width - 92) <= 1, JSON.stringify(treeGeometry))
    assert.equal(treeGeometry.start.clipped, false, JSON.stringify(treeGeometry))
    // ps prints the clock inside the shown day and the civil date before it.
    assert.match(treeGeometry.start.text, /^(?:\d{2}:\d{2}|\d{2}\.\d{2}\.\d{4})$/)
    assert.equal(treeGeometry.divider, "0px", JSON.stringify(treeGeometry))
    assert.ok(treeGeometry.gap >= 7 && treeGeometry.gap <= 11, JSON.stringify(treeGeometry))
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'user:postgres')
      input.dispatchEvent(new Event('input', { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="process-table"] .entity-row').length === 2`, "the filtered Process Tree with its parent")
    const treeLabels = await cdp.evaluate(`[...document.querySelectorAll('[data-testid="process-table"] .entity-row')].map((row) => row.getAttribute('aria-label'))`)
    assert.match(treeLabels[0], /корневой процесс/)
    assert.match(treeLabels[1], /дочерний процесс PID 2686800, уровень 1/)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 800, mobile: false, width: 1280 })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=pg.activity&panel=chart` })
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"]') !== null && document.querySelector('[data-testid="inspector-chart"] canvas') !== null`, "exact adjacent PostgreSQL Chart state")
    await cdp.evaluate(`(() => {
      const style = document.createElement('style')
      style.textContent = '#classic-scrollbar-sentinel::-webkit-scrollbar,.timeline-lanes::-webkit-scrollbar{height:15px;width:15px}'
      document.head.append(style)
      const sentinel = document.createElement('div')
      sentinel.id = 'classic-scrollbar-sentinel'
      sentinel.style.cssText = 'position:fixed;left:-1000px;top:0;width:40px;height:30px;overflow:scroll'
      sentinel.innerHTML = '<div style="width:80px;height:1px"></div>'
      document.body.append(sentinel)
    })()`)
    await settleLayout(cdp)
    const classicPreview = await cdp.evaluate(`(() => {
      const preview = document.querySelector('.timeline-preview')
      const rail = preview.querySelector('.timeline-rail')
      const lanes = preview.querySelector('.timeline-lanes')
      const trigger = preview.querySelector('.timeline-open-chart')
      const sentinel = document.querySelector('#classic-scrollbar-sentinel')
      const rect = (node) => { const box = node.getBoundingClientRect(); return { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top, width: box.width } }
      const laneRect = rect(lanes)
      const railRect = rect(rail)
      return {
        actionAtEdge: Math.abs(rect(trigger).right - railRect.right) <= .5,
        actionWidth: rect(trigger).width,
        classicRail: sentinel.offsetHeight - sentinel.clientHeight,
        height: rect(preview).height,
        labelsInside: [...lanes.children].every((child) => { const box = child.getBoundingClientRect(); return box.left >= laneRect.left - .5 && box.right <= laneRect.right + .5 }),
        laneClientWidth: lanes.clientWidth,
        laneOverflowX: getComputedStyle(lanes).overflowX,
        laneScrollWidth: lanes.scrollWidth,
        ownerCount: [preview, ...preview.querySelectorAll('*')].filter((node) => /(auto|scroll)/.test(getComputedStyle(node).overflowX) && node.scrollWidth > node.clientWidth + 1).length,
        panel: new URL(location.href).searchParams.get('panel'),
        url: location.href,
      }
    })()`)
    assert.equal(classicPreview.classicRail, 15, JSON.stringify(classicPreview))
    assert.equal(classicPreview.panel, "chart", JSON.stringify(classicPreview))
    assert.match(classicPreview.url, /view=pg\.activity/)
    assert.ok(Math.abs(classicPreview.height - 30) <= .5, JSON.stringify(classicPreview))
    assert.equal(classicPreview.laneOverflowX, "hidden", JSON.stringify(classicPreview))
    assert.ok(classicPreview.laneScrollWidth <= classicPreview.laneClientWidth + 1, JSON.stringify(classicPreview))
    assert.equal(classicPreview.ownerCount, 0, JSON.stringify(classicPreview))
    assert.equal(classicPreview.labelsInside, true, JSON.stringify(classicPreview))
    assert.equal(classicPreview.actionAtEdge, true, JSON.stringify(classicPreview))
    assert.ok(Math.abs(classicPreview.actionWidth - 64) <= 1, JSON.stringify(classicPreview))
    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[0].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-disk"]') !== null`, "Host resource table")
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-disk"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-group-chart-disk"] .uplot-figure') !== null`, "the inline Disk chart")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector"]') === null`), true)
    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`document.querySelectorAll('.pg-tabs button').length > 1`, "PostgreSQL tabs")
    await cdp.evaluate(`document.querySelectorAll('.pg-tabs button')[1].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "PostgreSQL Activity table")
    // The recorded vacuum lives on its own tab: one episode row whose phase
    // chip carries the fixed risk of its phase name.
    await cdp.evaluate(`[...document.querySelectorAll('.pg-tabs button')].find((tab) => tab.textContent === 'Vacuum').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-vacuum-table"] .entity-row') !== null`, "PostgreSQL Vacuum table")
    const vacuumRow = await cdp.evaluate(`(() => {
      const chip = [...document.querySelectorAll('[data-testid="pg-vacuum-table"] .vacuum-chip')].find((node) => ["ordinary", "heavy", "dangerous"].includes(node.getAttribute('data-risk')))
      const cells = [...document.querySelectorAll('[data-testid="pg-vacuum-table"] .entity-row [role="cell"]')].map((cell) => cell.textContent)
      return { cells, phase: chip?.textContent ?? null, risk: chip?.getAttribute('data-risk') ?? null, view: new URL(location.href).searchParams.get('view') }
    })()`)
    assert.equal(vacuumRow.view, "pg.vacuum", JSON.stringify(vacuumRow))
    assert.equal(vacuumRow.phase, "vacuuming heap", JSON.stringify(vacuumRow))
    assert.equal(vacuumRow.risk, "heavy", JSON.stringify(vacuumRow))
    // The relation name arrives already resolved on the row: no lookup, no
    // bare OID anywhere in the rendered row.
    assert.ok(vacuumRow.cells.some((cell) => cell.includes("operators.public.bulk_events")), JSON.stringify(vacuumRow))
    assert.equal(vacuumRow.cells.some((cell) => cell.includes("relid=")), false, JSON.stringify(vacuumRow))
    // Progress is a sparkline beside its percent, not a number crammed with
    // the sizes; the raw Inspector block never repeats the OID either.
    assert.ok(await cdp.evaluate(`document.querySelector('[data-testid="pg-vacuum-table"] .vacuum-progress-spark') !== null`))
    await cdp.evaluate(`document.querySelector('[data-testid="pg-vacuum-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null`, "PostgreSQL Vacuum detail")
    const detailText = await cdp.evaluate(`document.querySelector('[data-testid="pg-detail"]').textContent`)
    assert.doesNotMatch(detailText, /OID/)
    await cdp.evaluate(`[...document.querySelectorAll('.pg-tabs button')].find((tab) => tab.textContent === 'Activity').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, "PostgreSQL Activity table again")
    await cdp.evaluate(`(() => {
      const input = document.querySelector('[data-testid="table-filter"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'pid:3000')
      input.dispatchEvent(new Event('input', { bubbles: true }))
      input.form.requestSubmit()
    })()`)
    await cdp.waitFor(`document.querySelectorAll('[data-testid="pg-activity-table"] .entity-row').length === 1 && document.querySelector('[data-testid="pg-entity-layout"]').dataset.contentSized === 'true'`, "filtered sparse PostgreSQL Activity")
    const sparseBefore = await cdp.evaluate(sparsePostgresGeometry())
    assert.equal(sparseBefore.activity.contentSized, true, JSON.stringify(sparseBefore))
    assert.ok(sparseBefore.activity.scrollHeight <= sparseBefore.activity.scrollContentHeight + 15, JSON.stringify(sparseBefore))
    assert.ok(sparseBefore.activity.horizontal, JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.scrollAxis, "horizontal", JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.overflowX, "auto", JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.overflowY, "hidden", JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.vertical, false, JSON.stringify(sparseBefore))
    assert.equal(sparseBefore.activity.allRowsFit, true, JSON.stringify(sparseBefore))
    await cdp.evaluate(`document.querySelector('[data-testid="charts-toggle"]').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === 'chart' && document.querySelector('[data-testid="inspector-chart"] canvas') !== null`, "sparse PostgreSQL Chart Inspector")
    await cdp.evaluate(`(() => { const scroll = document.querySelector('[data-testid="pg-activity-table"] .entity-scroll'); scroll.scrollLeft = scroll.scrollWidth })()`)
    await settleLayout(cdp)
    const sparseSeam = await cdp.evaluate(sparsePostgresSeamGeometry())
    assert.equal(sparseSeam.panel, "chart", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.axis, "horizontal", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.overflowX, "auto", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.overflowY, "hidden", JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.vertical, false, JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.horizontal, true, JSON.stringify(sparseSeam))
    assert.equal(sparseSeam.allRowsFit, true, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.scrollLeft >= sparseSeam.scrollWidth - (sparseSeam.clientRight - sparseSeam.scroll.left) - 1, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.headerEndGap >= 7 && sparseSeam.headerEndGap <= 9, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.cellEndGap >= 7 && sparseSeam.cellEndGap <= 9, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.seamGap >= 6, JSON.stringify(sparseSeam))
    assert.ok(sparseSeam.chartToSplitter >= 6, JSON.stringify(sparseSeam))
    await cdp.evaluate(`document.querySelector('.inspector-close').click()`)
    await cdp.waitFor(`new URL(location.href).searchParams.get('panel') === null && document.querySelector('[data-testid="inspector"]') === null`, "sparse Chart Inspector close")
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="inspector-detail"] [data-testid="pg-detail"]') !== null`, "PostgreSQL detail in shared Inspector")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('[data-testid="inspector"]').length === 1 && document.querySelector('.workspace [data-testid="pg-detail"]') === null`), true)
    const sparseOpened = await cdp.evaluate(sparsePostgresGeometry())
    assert.ok(Math.abs(sparseOpened.activity.height - sparseBefore.activity.height) <= 1, JSON.stringify({ sparseBefore, sparseOpened }))
    await cdp.evaluate(`document.querySelector('.inspector-close').click(); document.querySelectorAll('.source-tabs button')[3].click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-mark"] button') !== null`, "Events threshold marks")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="inspector"]') === null`), true)
    await cdp.evaluate(`document.querySelector('[data-testid="event-mark"] > button').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="event-mark-crossing"]') !== null`, "the grouped crossings of one mark")
    await cdp.evaluate(`document.querySelector('[data-testid="event-mark-crossing"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="events-console"]') === null`, "a threshold mark leaves the Events console")
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

async function assertHoverGeometryStable(cdp, selector, label) {
  const expression = (position) => `(() => {
    const figure = document.querySelector(${JSON.stringify(selector)})
    const pick = (node) => { const box = node.getBoundingClientRect(); return [box.left, box.top, box.width, box.height] }
    const plot = figure.querySelector('.u-over')
    const box = plot.getBoundingClientRect()
    return {
      point: { x: box.left + box.width * ${position}, y: box.top + box.height * 0.45 },
      boxes: {
        figure: pick(figure), caption: pick(figure.querySelector('figcaption')),
        host: pick(figure.querySelector('.uplot-host')), plot: pick(plot), canvas: pick(figure.querySelector('canvas')),
      },
    }
  })()`
  const before = await cdp.evaluate(expression(0.25))
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...before.point })
  await cdp.waitFor(`document.querySelector(${JSON.stringify(selector)})?.querySelector('[data-testid="chart-hover-readout"]') !== null`, `${label} hover readout`)
  const middle = await cdp.evaluate(expression(0.55))
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...middle.point })
  const after = await cdp.evaluate(expression(0.8))
  await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...after.point })
  const moved = await cdp.evaluate(expression(0.8))
  for (const key of Object.keys(before.boxes)) {
    for (const current of [middle.boxes[key], moved.boxes[key]]) {
      before.boxes[key].forEach((value, index) => assert.ok(Math.abs(value - current[index]) <= 0.75, `${label} ${key} moved: ${JSON.stringify({ after: moved.boxes, before: before.boxes })}`))
    }
  }
}

function timelineRecords(hour = HOUR, cgroups = false) {
  const shift = hour - HOUR
  const shifted = (timestamp) => String(timestamp + shift)
  return [
    { record: "hour", from: String(hour), to: String(hour + HOUR_US - 1), available_hours: AVAILABLE_HOURS.map(String) },
    { record: "lane_context", segment_id: SEGMENT, postgresql_interval_seconds: null, environment: cgroups ? 1 : 0 },
    {
      record: "catalog", from: String(hour), to: String(hour + HOUR_US - 1),
      source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }],
    },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(hour), max_ts: shifted(AFTER_AT),
      sections: [{
        logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "pg_stat_statements", physical_name: "pg_stat_statements", type_id: "1002003",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "512",
      }, {
        logical_name: "pg_store_plans", physical_name: "pg_store_plans", type_id: "1004001",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "512",
      }, {
        logical_name: "pg_stat_database", physical_name: "pg_stat_database", type_id: "1005001",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "os_cpu", physical_name: "os_cpu", type_id: "1102001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "128",
      }, {
        logical_name: "os_process", physical_name: "os_process", type_id: "1100001",
        implementation: "linux", source_family: "system", rows: "80", bytes: "16384",
      }, ...(cgroups ? [{
        logical_name: "instance_metadata", physical_name: "instance_metadata", type_id: "1000001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "64",
      }, {
        logical_name: "os_cgroup_context", physical_name: "os_cgroup_context", type_id: "1205001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "128",
      }, {
        logical_name: "os_cgroup_cpu", physical_name: "os_cgroup_cpu", type_id: "1201001",
        implementation: "linux", source_family: "system", rows: "2", bytes: "256",
      }, {
        logical_name: "os_cgroup_memory", physical_name: "os_cgroup_memory", type_id: "1202001",
        implementation: "linux", source_family: "system", rows: "2", bytes: "256",
      }, {
        logical_name: "os_cgroup_io", physical_name: "os_cgroup_io", type_id: "1203002",
        implementation: "linux", source_family: "system", rows: "4", bytes: "512",
      }, {
        logical_name: "os_cgroup_pids", physical_name: "os_cgroup_pids", type_id: "1204001",
        implementation: "linux", source_family: "system", rows: "1", bytes: "128",
      }] : []), {
        logical_name: "pg_stat_user_tables", physical_name: "pg_stat_user_tables", type_id: "1013005",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }, {
        logical_name: "pg_stat_user_indexes", physical_name: "pg_stat_user_indexes", type_id: "1014004",
        implementation: "postgresql", source_family: "postgresql", rows: "1", bytes: "256",
      }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(QUARTER_PREVIOUS), identity: {}, value: 71 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(QUARTER_PREVIOUS), identity: {}, value: 61 },
    { record: "point", type_id: "0", series: "postgres_health", ts: shifted(QUARTER_PREVIOUS), identity: {}, value: 90 },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(QUARTER_NEXT), identity: {}, value: 73 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(QUARTER_NEXT), identity: {}, value: 63 },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(BEFORE_AT), identity: {}, value: null },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(BEFORE_AT), identity: {}, value: null },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(AT), identity: {}, value: 82 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(AT), identity: {}, value: 46 },
    { record: "point", type_id: "0", series: "os_health", ts: shifted(AFTER_AT), identity: {}, value: 84 },
    { record: "point", type_id: "0", series: "overall_health", ts: shifted(AFTER_AT), identity: {}, value: 48 },
    { record: "point", type_id: "0", series: "postgres_health", ts: shifted(AT), identity: {}, value: 64 },
    ...systemIndexRecords(shifted(AT)),
    { record: "lane", segment_id: SEGMENT, lane: "disk_busy", ts: shifted(BEFORE_AT), value: 34 },
    { record: "lane", segment_id: SEGMENT, lane: "disk_busy", ts: shifted(AT), value: 42 },
    // The container scope: lanes of the collector's own cgroup and its pressure.
    ...(cgroups ? [
      ["cg_cpu_share", 20, 25], ["cg_cpu_cores", 0.8, 1], ["cg_cpu_throttle", 0, 0], ["cg_cpu_psi", 0, 1.5],
      ["cg_memory", 30, 40], ["cg_memory_bytes", 1_073_741_824, 1_610_612_736], ["cg_mem_psi", 0, 0], ["cg_oom", null, 0],
      ["cg_io_read", 1_024, 2_048], ["cg_io_write", 2_048, 4_096], ["cg_io_psi", 0, 0],
      ["cg_pids_share", 3.125, 3.125], ["cg_pids", 4, 4],
    ].flatMap(([lane, before, at]) => [
      { record: "lane", segment_id: SEGMENT, lane, ts: shifted(BEFORE_AT), value: before },
      { record: "lane", segment_id: SEGMENT, lane, ts: shifted(AT), value: at },
    ]) : []),
    {
      record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002003",
      field_ordinal: 11, row_ordinal: "91", ts: shifted(AT),
    },
  ]
}

const POSTGRES_SUMMARY_FIELDS = [
  "surface", "active_count", "active_pct", "mean_exec_ms", "buffer_read_pct", "rollback_pct", "temp_bytes_per_transaction", "seq_scan_pct",
]

function postgresSummaryRecords(hour) {
  const shift = hour - HOUR
  const records = [
    [BEFORE_AT, 1, { active_count: 1, active_pct: 20, mean_exec_ms: 5, buffer_read_pct: 40 }],
    [BEFORE_AT, 2, { active_count: 1, active_pct: 20, mean_exec_ms: 5, buffer_read_pct: 40 }],
    [BEFORE_AT, 3, { rollback_pct: 10, temp_bytes_per_transaction: 1_024, buffer_read_pct: 40 }],
    [BEFORE_AT, 4, { seq_scan_pct: 25, buffer_read_pct: 40 }],
    [BEFORE_AT, 5, { buffer_read_pct: 40 }],
    [AT, 1, { active_count: 3, active_pct: 60, mean_exec_ms: 15, buffer_read_pct: 20 }],
    [AT, 2, { active_count: 3, active_pct: 60, mean_exec_ms: 15, buffer_read_pct: 20 }],
    [AT, 3, { rollback_pct: 20, temp_bytes_per_transaction: 2_048, buffer_read_pct: 20 }],
    [AT, 4, { seq_scan_pct: 40, buffer_read_pct: 20 }],
    [AT, 5, { buffer_read_pct: 20 }],
  ]
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    layout("postgresql-summary", "postgresql_summary", POSTGRES_SUMMARY_FIELDS),
    ...records.map(([timestamp, surface, facts]) => row(
      "postgresql-summary",
      `${timestamp}-${surface}`,
      POSTGRES_SUMMARY_FIELDS.map((field) => field === "surface" ? surface : facts[field] ?? null),
      timestamp + shift,
    )),
  ]
}

function snapshotTargetTimelineRecords() {
  return timelineRecords().map((record) => record.record !== "finished_segment"
    ? record
    : {
        ...record,
        sections: [...record.sections, {
          logical_name: "os_meminfo", physical_name: "os_meminfo", type_id: "1104001",
          implementation: "linux", source_family: "system", rows: "1", bytes: "128",
        }, {
          logical_name: "os_diskstats", physical_name: "os_diskstats", type_id: "1108001",
          implementation: "linux", source_family: "system", rows: "1", bytes: "256",
        }],
      })
}

function systemIndexRecords(timestamp) {
  return [
    ["os_cpu_busy_percent", 42],
    ["os_mem_available_percent", 58],
    ["os_oom_kills", 0],
    ["os_min_filesystem_free_percent", 61],
    ["os_device_count", 3],
    ["os_device_active_io", 1],
    ["os_filesystem_count", 8],
    ["os_interface_count", 4],
    ["os_network_rx", 2_400_000],
    ["os_network_tx", 1_700_000],
    ["os_network_errors", 0],
    ["os_network_drops", 0],
  ].map(([series, value]) => ({ record: "point", type_id: "0", series, ts: timestamp, identity: {}, value }))
}

function systemSnapshotRecords(cgroupContext = false, at = AT) {
  const cpuColumns = ["ts", "cpu_id", "user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal", "scope"]
  const cgroupPath = at === BEFORE_AT ? "/collector-before" : "/collector"
  return [
    { record: "layout", rates: ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"], layout: { type_id: "1102001", logical_name: "os_cpu", columns: cpuColumns.map((name) => ({ name })) } },
    row("1102001", "cpu-all", [String(at), -1, 20, 5, 10, 50, 5, 2, 3, 5, 0], at),
    row("1102001", "cpu-0", [String(at), 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    row("1102001", "cpu-1", [String(at), 1, 0, 0, 0, 0, 0, 0, 0, 0, 0], at),
    layout("1113001", "os_topology", ["ts", "cpu_id", "model_name", "mhz_max", "core_id", "socket_id", "numa_node", "scope"]),
    row("1113001", "topology-0", [String(at), 0, "Artifact CPU", null, 0, 0, 0, 0], at),
    layout("1103001", "os_stat", ["ts", "ctxt", "procs_running", "procs_blocked"]),
    row("1103001", "1", [String(at), 1_234_567, 3, 1], at),
    layout("1105001", "os_loadavg", ["ts", "load1", "load5", "load15", "running", "total"]),
    row("1105001", "2", [String(at), 1.25, 1.1, 0.9, 3, 214], at),
    layout("1104001", "os_meminfo", ["ts", "mem_available", "mem_total", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim", "swap_free", "swap_total"]),
    row("1104001", "3", [String(at), 8_388_608, 16_777_216, 1_048_576, 4_194_304, 131_072, 5_242_880, 524_288, 262_144, 1_048_576, 2_097_152], at),
    layout("1107001", "os_psi", ["ts", "resource", "some_avg10"]),
    row("1107001", "4", [String(at), 0, 2.5], at),
    row("1107001", "5", [String(at), 1, 1.2], at),
    row("1107001", "6", [String(at), 2, 0.4], at),
    layout("1108001", "os_diskstats", ["ts", "major", "minor", "device", "reads", "writes", "read_sectors", "write_sectors", "read_time_ms", "write_time_ms", "io_time_ms", "io_weighted_time_ms", "io_in_progress", "scope"]),
    row("1108001", "7", [String(at), 8, 0, "device_target_A", 100, 50, 200, 150, 1_000, 500, 2_000, 2_500, 1, 0], at),
    ...(cgroupContext ? [
      layout("1000001", "instance_metadata", ["environment"]),
      row("1000001", "metadata", [1], at),
      layout("1205001", "os_cgroup_context", [
        "ts", "cgroup_version", "cpu_path", "memory_path", "io_path", "cpuset_cpus",
        "effective_cpu_quota_usec", "effective_cpu_period_usec", "effective_memory_max", "scope",
      ]),
      row("1205001", "context", [String(at), 2, cgroupPath, cgroupPath, cgroupPath, 2, -1, 100_000, null, 3], at),
    ] : []),
  ]
}

function cgroupSnapshotRecords(url) {
  const section = url.searchParams.get("section")
  const fields = url.searchParams.getAll("field")
  const at = Number(url.searchParams.get("at") ?? AT)
  const path = url.searchParams.get("where.cgroup_path") ?? (at === BEFORE_AT ? "/collector-before" : "/collector")
  const definitions = {
    os_cgroup_cpu: {
      typeId: "1201001",
      values: { ts: at, cgroup_path: path, usage_usec: 1_500_000, user_usec: 1_000_000, system_usec: 400_000, throttled_usec: 0, nr_throttled: 0, quota_usec: 400_000, period_usec: 100_000, scope: 3 },
    },
    os_cgroup_memory: {
      typeId: "1202001",
      values: { ts: at, cgroup_path: path, current: 1024, max: 4096, anon: 512, file: 256, kernel: 128, slab: 64, low_events: 0, high_events: 0, max_events: 0, oom_events: 0, oom_kill: 0, scope: 3 },
    },
    os_cgroup_io: {
      typeId: "1203002",
      values: { ts: at, cgroup_path: path, major: 8, minor: 0, rbytes: 1024, wbytes: 2048, rios: 2, wios: 3, scope: 3 },
    },
    os_cgroup_pids: {
      typeId: "1204001",
      values: { ts: at, cgroup_path: path, current: 4, max: 128, scope: 3 },
    },
  }
  const definition = definitions[section]
  if (definition === undefined) return []
  const values = section === "os_cgroup_io"
    ? [definition.values, { ...definition.values, major: 253, minor: 0, rbytes: 4096, wbytes: 8192, rios: 5, wios: 7 }]
    : [definition.values]
  return [
    layout(definition.typeId, section, fields),
    ...values.map((stored, index) => row(definition.typeId, `${section}-${index}`, fields.map((field) => stored[field] ?? null), at)),
  ]
}

function layout(typeId, logicalName, columns) {
  return { record: "layout", rates: [], layout: { type_id: typeId, logical_name: logicalName, columns: columns.map((name) => ({ name })) } }
}

function row(typeId, ordinal, values, timestamp = AT) {
  return { record: "row", segment_id: SEGMENT, type_id: typeId, ordinal, timestamp: String(timestamp), values }
}

function relationRecords(url, mode) {
  const indexes = url.searchParams.getAll("section").includes("pg_stat_user_indexes")
  const logicalName = indexes ? "pg_stat_user_indexes" : "pg_stat_user_tables"
  const group = url.searchParams.get("group") ?? "object"
  const state = url.searchParams.getAll("field").includes("invalid_count")
  const requestedFields = url.searchParams.getAll("field")
  const sized = requestedFields.includes("reltuples") || requestedFields.includes("main_fork_bytes")
  const page = url.searchParams.get("cursor")
  const count = mode === "long" ? page === null ? 200 : 5 : mode === "short" ? 3 : 1
  const offset = page === null ? 0 : 200
  const baseKey = group === "database"
    ? { datid: "42", datname: "artifact_db" }
    : group === "schema"
      ? { datid: "42", datname: "artifact_db", schemaname: "public" }
      : group === "tablespace"
        ? { tablespace_oid: "1663" }
      : indexes
        ? { datid: "42", datname: "artifact_db", schemaname: "public", relid: "73", relname: "artifact_table", indexrelid: "74", indexrelname: "artifact_index" }
        : { datid: "42", datname: "artifact_db", schemaname: "public", relid: "73", relname: "artifact_table" }
  const columns = indexes
    ? state
      ? group === "object"
        ? [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("indisvalid", "bool", "none"), wire("indisready", "bool", "none"), wire("indisunique", "bool", "none"), wire("indisprimary", "bool", "none")]
        : [...(group === "tablespace" ? [wire("tablespace", "text", "none")] : []), wire("index_count"), wire("invalid_count"), wire("unready_count"), wire("unique_count"), wire("primary_count"), wire("exclusion_count")]
      : sized
        ? [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("main_fork_bytes", "number", "bytes"), wire("idx_blks_read", "number", "per_second"), wire("idx_blks_hit", "number", "per_second"), wire("buffer_hit_pct", "number", "percent")]
        : [wire("tablespace", "text", "none"), wire("amname", "text", "none"), wire("idx_scan", "number", "per_second")]
    : sized
      ? [wire("tablespace", "text", "none"), wire("main_fork_bytes", "number", "bytes"), wire("toast_bytes", "number", "bytes"), wire("reltuples"), wire("toast_n_live_tup"), wire("toast_n_dead_tup")]
      : group === "tablespace"
        ? [wire("tablespace", "text", "none"), wire("table_count"), wire("seq_scan", "number", "per_second")]
        : [wire("tablespace", "text", "none"), wire("seq_scan", "number", "per_second")]
  const baseValues = indexes
    ? state
      ? group === "object"
        ? { tablespace: "pg_default", amname: "btree", indisvalid: true, indisready: true, indisunique: true, indisprimary: true }
        : { ...(group === "tablespace" ? { tablespace: "fast_ssd" } : {}), index_count: 363, invalid_count: 0, unready_count: 0, unique_count: 223, primary_count: 111, exclusion_count: 0 }
      : sized
        ? { tablespace: "pg_default", amname: "btree", main_fork_bytes: 524_288, idx_blks_read: 2, idx_blks_hit: 14, buffer_hit_pct: 87.5 }
        : { tablespace: "pg_default", amname: "btree", idx_scan: 3 }
    : sized
      ? { tablespace: "pg_default", main_fork_bytes: 1_048_576, toast_bytes: 131_072, reltuples: "9007199254740993", toast_n_live_tup: "713456", toast_n_dead_tup: "12876" }
      : { tablespace: group === "tablespace" ? "fast_ssd" : "pg_default", ...(group === "tablespace" ? { table_count: 17 } : {}), seq_scan: 3 }
  const rows = Array.from({ length: count }, (_, local) => {
    const index = offset + local
    const key = group !== "object" ? baseKey : indexes
      ? { ...baseKey, indexrelid: String(74 + index), indexrelname: index === 0 ? "artifact_index" : `artifact_index_${index}` }
      : { ...baseKey, relid: String(73 + index), relname: index === 0 ? "artifact_table" : `artifact_table_${index}` }
    return {
      record: "relation", logical_name: logicalName, group, key, values: baseValues,
      sample_from: String(AT - 5_000_000), sample_to: String(AT),
      source: group === "object" ? { segment_id: SEGMENT, type_id: indexes ? "1014004" : "1013005", ordinal: String((indexes ? 8 : 7) + index), timestamp: String(AT) } : null,
    }
  })
  const hasMore = mode === "long" && page === null
  return [
    {
      record: "relation_layout", logical_name: logicalName, group, columns,
    },
    ...rows,
    {
      record: "snapshot_page", logical_name: logicalName, group,
      eligible: String(mode === "long" ? 205 : count), returned: String(count), has_more: hasMore, truncated: false, next_cursor: hasMore ? "viewport-page-two" : null,
      page_size: 200, order_by: url.searchParams.getAll("by"), order_direction: url.searchParams.get("order") ?? "desc",
      from: String(AT - 5_000_000), to: String(AT),
    },
  ]
}

function aggregateRelationHistoryRecords(url) {
  const logicalName = url.searchParams.get("section")
  const group = url.searchParams.get("group")
  const fields = url.searchParams.getAll("field")
  const values = (offset) => Object.fromEntries(fields.map((field) => [field, ({
    index_count: 363,
    invalid_count: offset,
    unready_count: 0,
    unique_count: 223 + offset,
    primary_count: 111,
    exclusion_count: 0,
  })[field] ?? 0]))
  return [
    { record: "hour", from: String(HOUR), to: String(AT), available_hours: [String(HOUR)] },
    { record: "relation_layout", logical_name: logicalName, group, columns: fields.map((field) => wire(field)) },
    { record: "series_segment", segment: { id: SEGMENT } },
    {
      record: "relation", logical_name: logicalName, group,
      key: group === "tablespace" ? { tablespace_oid: "1663" } : { datid: "42", datname: "artifact_db" }, values: values(0),
      sample_from: String(BEFORE_AT - 5_000_000), sample_to: String(BEFORE_AT), source: null,
    },
    {
      record: "relation", logical_name: logicalName, group,
      key: group === "tablespace" ? { tablespace_oid: "1663" } : { datid: "42", datname: "artifact_db" }, values: values(1),
      sample_from: String(BEFORE_AT), sample_to: String(AT), source: null,
    },
  ]
}

function wire(name, kind = "number", unit = "count") {
  return { name, kind, unit, nullable: true }
}

function exactIndexRecords() {
  const columns = ["ts", "datid", "datname", "schemaname", "relid", "relname", "indexrelid", "indexrelname", "indexdef", "idx_scan"]
  return [{ record: "layout", rates: ["idx_scan"], layout: { type_id: "1014004", logical_name: "pg_stat_user_indexes", columns: columns.map((name) => ({ name })) } }, {
    record: "row", segment_id: SEGMENT, type_id: "1014004", ordinal: "8", timestamp: String(AT),
    values: [String(AT), "42", "artifact_db", "public", "73", "artifact_table", "74", "artifact_index", "CREATE UNIQUE INDEX artifact_index ON public.artifact_table USING btree (id)", 15],
  }]
}

function snapshotRecords() {
  const processColumns = [
    "ts", "pid", "comm", "cmdline", "ppid", "uid", "euid", "gid", "egid", "num_threads", "tty", "exit_signal",
    "state", "starttime", "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "curcpu", "nice", "prio", "rtprio", "policy",
    "rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt", "read_bytes", "write_bytes", "syscr", "syscw", "rchar", "wchar", "cancelled_write_bytes",
    "user", "effective_user",
  ]
  const columns = [
    "ts", "pid", "leader_pid", "datid", "datname", "usename", "application_name", "client_addr", "backend_type",
    "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age", "backend_xmin_age",
    "backend_start", "xact_start", "query_start", "state_change",
  ]
  const processValues = (pid, index) => [
    String(AT), pid, pid === 2_686_712 ? "postgres" : `worker-${index}`, pid === 2_686_712 ? "postgres: artifact_db artifact_role 192.0.2.72" : null,
    pid === 2_686_712 ? 2_686_800 : 1, pid === 2_686_712 ? 26 : 1000, pid === 2_686_712 ? 27 : 1000, 1000, 1000, 2 + index % 4, 0, 17, index % 3 === 0 ? 82 : 83,
    String(Date.UTC(2022, 5, 16, 4, 30, index % 60) * 1_000),
    1000 + index, 300 + index, 5_000_000 + index, 12 + index, 50 + index, 3 + index, index % 8, -5, 15, 0, 0,
    1024 + index, 4096 + index, index % 3, 20 + index, 2 + index, 4096 + index, 8192 + index, 4 + index, 5 + index, 16_384 + index, 32_768 + index, 0,
    pid === 2_686_712 ? "postgres" : "app", pid === 2_686_712 ? "postgres-worker" : "app",
  ]
  return [
    layout("1100001", "os_process", processColumns),
    ...Array.from({ length: 80 }, (_, index) => {
      const pid = index === 27 ? 2_686_712 : 2_686_800 + index
      return row("1100001", String(index), processValues(pid, index), AT)
    }),
    {
      record: "layout", rates: [],
      layout: { type_id: "1001004", logical_name: "pg_stat_activity", columns: columns.map((name) => ({ name })) },
    },
    {
      record: "row", segment_id: SEGMENT, type_id: "1001004", ordinal: "73", timestamp: String(AT),
      values: [
        String(AT), 4242, null, 20, "operators", "kronika", "artifact-test", "127.0.0.1", "client backend",
        "active", null, null, "select artifact_wire_contract", "991", null, "7",
        String(AT - 60_000_000), String(AT - 30_000_000), String(AT - 5_000_000), String(AT - 1_000_000),
      ],
    },
    {
      record: "row", segment_id: SEGMENT, type_id: "1001004", ordinal: "74", timestamp: String(AT - 2_000_000),
      values: [
        String(AT - 2_000_000), 2_686_712, null, 21, "artifact_db", "artifact_role", "offset-activity", "192.0.2.72", "client backend",
        "active", "IO", "DataFileRead", "select activity_for_2686712", "992", "3", "8",
        String(AT - 120_000_000), String(AT - 45_000_000), String(AT - 7_000_000), String(AT - 2_500_000),
      ],
    },
  ]
}

function processReviewTimelineRecords() {
  return snapshotTargetTimelineRecords().map((record) => record.record !== "finished_segment"
    ? record
    : {
        ...record,
        sections: [...record.sections, {
          logical_name: "instance_metadata", physical_name: "instance_metadata", type_id: "1000001",
          implementation: "linux", source_family: "system", rows: "1", bytes: "64",
        }],
      })
}

function snapshotRecordsAt(timestamp) {
  const records = snapshotRecords()
  const columns = new Map(records
    .filter((record) => record.record === "layout")
    .map((record) => [record.layout.type_id, record.layout.columns.map(({ name }) => name)]))
  return records.map((record) => {
    if (record.record !== "row") return record
    const names = columns.get(record.type_id) ?? []
    const ts = names.indexOf("ts")
    return {
      ...record,
      timestamp: String(timestamp),
      values: ts < 0 ? record.values : record.values.map((value, index) => index === ts ? String(timestamp) : value),
    }
  })
}

function processSnapshotRecordsAt(timestamp) {
  return snapshotRecordsAt(timestamp).filter((record) => (record.record === "layout" ? record.layout.type_id : record.type_id) === "1100001")
}

function instanceMetadataRecords(timestamp, ticksPerSecond = 100) {
  return [
    layout("1000001", "instance_metadata", ["ts", "postgresql_interval_seconds", "clock_ticks_per_sec", "environment"]),
    row("1000001", "metadata", [String(timestamp), 30, ticksPerSecond, 1], timestamp),
  ]
}

function processHistoryRecords(url) {
  const fields = url.searchParams.getAll("field")
  const records = snapshotRecords()
  const sourceLayout = records.find((record) => record.record === "layout" && record.layout.type_id === "1100001")
  const sourceRow = records.find((record) => record.record === "row" && record.type_id === "1100001")
  const sourceFields = sourceLayout.layout.columns.map(({ name }) => name)
  const source = Object.fromEntries(sourceFields.map((field, index) => [field, sourceRow.values[index]]))
  const counterStep = {
    blkdelay_ticks: 7, cancelled_write_bytes: 512, majflt: 2, minflt: 19, nivcsw: 4, nvcsw: 13,
    rchar: 4096, read_bytes: 2048, rundelay_ns: 900_000, stime: 80, syscr: 5, syscw: 6,
    utime: 170, wchar: 8192, write_bytes: 3072,
  }
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    layout("1100001", "os_process", fields),
    ...[BEFORE_AT, AT, AFTER_AT].map((timestamp, index) => row(
      "1100001",
      `history-${index}`,
      fields.map((field) => {
        if (field === "pid") return Number(url.searchParams.get("where.pid") ?? source.pid)
        const base = source[field]
        const step = counterStep[field]
        return step === undefined || typeof base !== "number" ? base ?? null : base + step * index
      }),
      timestamp,
    )),
  ]
}

function progressVacuumRecords() {
  const columns = [
    "ts", "pid", "datid", "datname", "relid", "schemaname", "relname", "is_autovacuum", "phase", "heap_blks_total",
    "heap_blks_scanned", "heap_blks_vacuumed", "index_vacuum_count", "max_dead_tuple_bytes", "dead_tuple_bytes",
    "num_dead_item_ids", "indexes_total", "indexes_processed", "delay_time",
  ]
  return [layout("1012006", "pg_stat_progress_vacuum", columns), row("1012006", "1", [
    String(AT), 4343, 20, "operators", 73, "public", "bulk_events", true, "vacuuming heap", 8000, 3200, 1200, 1, 67_108_864, 4_194_304, 2400, 5, 2, 17.5,
  ], AT)]
}

// Mirrors the server's real rule (query.rs output_names): an empty `field`
// list answers with every column the segment's own layout defines, not a
// fixed union across every PG-version shape. VacuumView relies on exactly
// this — asking by name for a column an older layout never defines is a
// hard error server-side, not a null fill, so the hour fetch must omit the
// field list entirely.
const VACUUM_V3_COLUMNS = [
  "ts", "pid", "datid", "datname", "relid", "schemaname", "relname", "is_autovacuum", "phase", "heap_blks_total",
  "heap_blks_scanned", "heap_blks_vacuumed", "index_vacuum_count", "max_dead_tuple_bytes", "dead_tuple_bytes",
  "num_dead_item_ids", "indexes_total", "indexes_processed", "delay_time",
]

function vacuumHourRecords(url) {
  const requested = url.searchParams.getAll("field")
  if (requested.some((field) => !VACUUM_V3_COLUMNS.includes(field))) {
    return [{ record: "error", error: "bad_parameter" }]
  }
  const columns = requested.length > 0 ? requested : VACUUM_V3_COLUMNS.filter((field) => field !== "ts")
  const sample = {
    datid: 20, datname: "operators", dead_tuple_bytes: 4_194_304, delay_time: 17.5, heap_blks_scanned: 3200,
    heap_blks_total: 8000, heap_blks_vacuumed: 1200, index_vacuum_count: 1, indexes_processed: 2, indexes_total: 5,
    is_autovacuum: true, max_dead_tuple_bytes: 67_108_864, num_dead_item_ids: 2400, phase: "vacuuming heap", pid: 4343,
    relid: 73, relname: "bulk_events", schemaname: "public",
  }
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    layout("1012006", "pg_stat_progress_vacuum", columns),
    row("1012006", "1", columns.map((field) => sample[field] ?? null), AT),
  ]
}

function activityHistoryRecords(url) {
  const fields = url.searchParams.getAll("field")
  const samples = [
    { pid: 4242, query_start: String(BEFORE_AT - 5_000_000), state: "active", timestamp: BEFORE_AT, xact_start: String(BEFORE_AT - 12_000_000) },
    { pid: 4242, query_start: String(AT - 5_000_000), state: "active", timestamp: AT, xact_start: String(AT - 30_000_000) },
  ]
  return [
    { record: "series_segment", segment: { id: SEGMENT } },
    layout("1001004", "pg_stat_activity", fields),
    ...samples.map((sample, index) => row("1001004", String(70 + index), fields.map((field) => sample[field] ?? null), sample.timestamp)),
  ]
}

function statementRecords(page, eligible = 1, hasMore = false, rowCount = eligible) {
  const columns = ["ts", "queryid", "userid", "dbid", "toplevel", "datname", "usename", "query", "calls", "rows", "total_exec_time"]
  return [
    {
      record: "layout", rates: ["calls", "rows", "total_exec_time"],
      layout: { type_id: "1002003", logical_name: "pg_stat_statements", columns: columns.map((name) => ({ name })) },
    },
    ...Array.from({ length: rowCount }, (_, index) => ({
      record: "row", segment_id: SEGMENT, type_id: "1002003", ordinal: String(91 + index), timestamp: String(AT),
      values: [String(AT), String(9_007_199_254_740_991n - BigInt(index)), 10, 20, true, "operators", "reporter", index === 0 ? "select artifact_exact_context" : `select artifact_page_${index}`, 2 + index, 1, 7.5 + index],
    })),
    ...(page ? [{
      record: "snapshot_page", logical_name: "pg_stat_statements", eligible: String(eligible), returned: String(rowCount),
      has_more: hasMore, truncated: hasMore, next_cursor: hasMore ? "next-statement-page" : null, page_size: 200,
      order_by: ["total_exec_time", "calls"], order_direction: "desc", from: String(AT - 5_000_000), to: String(AT),
    }] : []),
  ]
}

const VADV_TEXT_PLAN = [
  "Merge Join  (cost=0.85..81.42 rows=12 width=64)",
  "  Merge Cond: (orders.customer_id = customers.id)",
  "  ->  Seq Scan on orders  (cost=0.00..20.00 rows=400 width=32)",
  "        Filter: (status = 'open'::text)",
  "  ->  Index Scan using customers_pkey on customers  (cost=0.42..8.44 rows=1 width=32)",
  "        Index Cond: (id > 0)",
].join("\n")

const INLINE_QUERY_PRIMARY = [
  "  SELECT jobs.id, jobs.payload",
  "  FROM jobs",
  "  WHERE jobs.state = 'ready'",
  ...Array.from({ length: 70 }, (_, index) => `    AND jobs.partition_${index} = ${index}`),
  "  ORDER BY jobs.created_at",
].join("\n")
function planRecords() {
  const columns = ["ts", "userid", "dbid", "queryid", "planid", "queryid_stat_statements", "datname", "usename", "plan", "calls", "total_time", "rows"]
  return [
    { record: "layout", rates: ["calls", "total_time", "rows"], layout: { type_id: "1004001", logical_name: "pg_store_plans", columns: columns.map((name) => ({ name })) } },
    { record: "row", segment_id: SEGMENT, type_id: "1004001", ordinal: "201", timestamp: String(AT), values: [String(AT), 10, 20, 0, 77, 42, "operators", "reporter", VADV_TEXT_PLAN, 3, 12, 4] },
    { record: "snapshot_page", logical_name: "pg_store_plans", eligible: "1", returned: "1", has_more: false, truncated: false, next_cursor: null, page_size: 200, order_by: ["total_time", "calls"], order_direction: "desc", from: String(AT - 5_000_000), to: String(AT) },
  ]
}

function planStatementRecords() {
  return statementRecords(true).map((record) => record.record !== "row" ? record : {
    ...record,
    values: record.values.map((stored, index) => index === 1 ? "42" : index === 7 ? "select from plan_navigation" : stored),
  })
}

function inlinePlanQueryRecords() {
  const columns = ["query"]
  return [
    { record: "layout", layout: { type_id: "1002003", logical_name: "pg_stat_statements", columns: columns.map((name) => ({ name })) } },
    { record: "row", segment_id: SEGMENT, type_id: "1002003", ordinal: "301", timestamp: String(AT), values: [INLINE_QUERY_PRIMARY] },
    { record: "snapshot_page", logical_name: "pg_stat_statements", eligible: "1", returned: "1", has_more: false, truncated: false, next_cursor: null, page_size: 1, order_by: [], order_direction: "desc", from: null, to: String(AT) },
  ]
}

function emptyPlanQueryRecords() {
  return [{
    record: "snapshot_page", logical_name: "pg_stat_statements", eligible: "0", returned: "0", has_more: false,
    truncated: false, next_cursor: null, page_size: 1, order_by: [], order_direction: "desc", from: null, to: null,
  }]
}

function targetedActivityRecords(query, timestamp) {
  return snapshotRecords().map((record) => record.record !== "row"
    ? record
    : {
        ...record,
        timestamp: String(timestamp),
        values: record.values.map((value, index) => index === 0 ? String(timestamp) : index === 12 ? query : value),
      })
}

function targetedStatementRecords(query, eligible) {
  return statementRecords(true, eligible, false, 1).map((record) => record.record === "row"
    ? { ...record, values: record.values.map((value, index) => index === 7 ? query : value) }
    : record)
}

function targetedRelationRecords(url, label, eligible) {
  return relationRecords(url, "single").map((record) => {
    if (record.record === "snapshot_page") return { ...record, eligible: String(eligible) }
    if (record.record !== "relation") return record
    const key = record.group === "database"
      ? { ...record.key, datname: label }
      : record.group === "schema"
        ? { ...record.key, schemaname: label }
        : record.group === "tablespace"
          ? record.key
        : { ...record.key, relname: label }
    return record.group === "tablespace" ? { ...record, key, values: { ...record.values, tablespace: label } } : { ...record, key }
  })
}


function answerHeatmap(url, response) {
  assert.equal(url.searchParams.has("label"), false)
  const from = Number(url.searchParams.get("from") ?? "0")
  const to = Number(url.searchParams.get("to") ?? "0")
  const columns = Number(url.searchParams.get("columns") ?? "60")
  const section = url.searchParams.get("section") ?? ""
  const labels = {
    pg_stat_database: ["datname"],
    pg_stat_statements: ["datname", "usename"],
    pg_stat_user_indexes: ["datname", "schemaname", "relname", "indexrelname"],
    pg_stat_user_tables: ["datname", "schemaname", "relname"],
    pg_store_plans: ["datname", "usename"],
  }[section] ?? []
  const labelValues = labels.map(() => "demo")
  const span = to - from + 1
  const intervals = Array.from({ length: columns }, (_at, index) => ({
    start: String(from + Math.floor((index * span) / columns)),
    end: String(from + Math.floor(((index + 1) * span) / columns) - 1),
  }))
  const cells = Array.from({ length: columns }, (_at, index) => index < 3 ? (index + 1) * 0.5 : null)
  const identities = section === "pg_store_plans"
    ? [
        { identity: ["10", "20", "42", "77", "42"], total: 120, typeId: "1004001" },
        { identity: ["10", "20", "43", "78", "43"], total: 60, typeId: "1004001" },
      ]
    : [
        { identity: ["101", "10", "5", "true"], total: 120, typeId: "1002006" },
        { identity: ["102", "10", "5", "true"], total: 60, typeId: "1002006" },
      ]
  // Only Statements rankings may be scoped; the workload scope drops the collector's own query id 102.
  const scope = url.searchParams.get("scope")
  assert.ok(scope === null || (scope === "workload" && section === "pg_stat_statements"), `${section} ${scope}`)
  const ranked = scope === "workload" ? identities.filter(({ identity }) => identity[0] !== "102") : identities
  return ndjson(response, [
    {
      record: "heatmap", from: String(from), to: String(to), section,
      fields: url.searchParams.getAll("field"), class: "cumulative", summary: "sum", labels,
      top: ranked.length, entity_count: ranked.length + 1, others_count: 1, out_of_order: "0", intervals,
    },
    ...ranked.map(({ identity, total, typeId }) => ({ record: "heatmap_row", type_id: typeId, identity, labels: labelValues, total, cells })),
    { record: "heatmap_band", band: "totals", total: 200, cells },
    { record: "heatmap_band", band: "others", total: 20, cells },
  ])
}

function answerInstanceLabel(response) {
  response.writeHead(200, { "Content-Type": "application/json" })
  response.end(JSON.stringify({ record: "instance_label", database: "artifact_db" }))
}

function ndjson(response, records) {
  response.writeHead(200, {
    "Cache-Control": "no-store",
    "Content-Type": "application/x-ndjson; charset=utf-8",
  })
  response.end(records.map((record) => JSON.stringify(record)).join("\n") + (records.length === 0 ? "" : "\n"))
}

function brokenNdjson(response) {
  response.writeHead(200, {
    "Cache-Control": "no-store",
    "Content-Type": "application/x-ndjson; charset=utf-8",
  })
  response.end("{")
}

function requestRecord(request, url) {
  return {
    authorization: request.headers.authorization ?? null,
    cookie: request.headers.cookie ?? null,
    marker: request.headers["x-kronika-ui"] ?? null,
    method: request.method ?? "GET",
    path: url.pathname,
    query: url.search,
  }
}

function answerSession(request, response, state) {
  const headers = { "Cache-Control": "no-store" }
  if (request.method === "GET") {
    response.writeHead(browserIsAuthenticated(request, state) ? 204 : 401, headers)
    response.end()
    return
  }
  if (request.method === "POST") {
    if (request.headers.authorization !== "Basic YXJ0aWZhY3Q6d2lyZQ==") {
      response.writeHead(401, headers)
      response.end()
      return
    }
    state.valid = true
    response.writeHead(204, {
      ...headers,
      "Set-Cookie": `${SESSION_COOKIE}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000`,
    })
    response.end()
    return
  }
  if (request.method === "DELETE") {
    state.valid = false
    response.writeHead(204, {
      ...headers,
      "Set-Cookie": "kronika_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
    })
    response.end()
    return
  }
  response.writeHead(405, { ...headers, Allow: "GET, POST, DELETE" })
  response.end()
}

function browserIsAuthenticated(request, state) {
  return state.valid
    && request.headers.cookie?.split(";").map((value) => value.trim()).includes(SESSION_COOKIE) === true
    && request.headers["x-kronika-ui"] === "1"
}

function unauthorized(response) {
  response.writeHead(401, { "Cache-Control": "no-store", "Content-Type": "application/json" })
  response.end('{"error":"unauthorized"}')
}

function trackPage(socket, origin, result) {
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data)
    if (message.method === "Runtime.exceptionThrown") {
      result.errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text)
    }
    if (message.method === "Runtime.consoleAPICalled" && ["assert", "error"].includes(message.params.type)) {
      result.errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "))
    }
    if (message.method === "Log.entryAdded" && message.params.entry.level === "error") {
      if (!expectedUnauthorizedLog(message.params.entry.text)) result.errors.push(message.params.entry.text)
    }
    if (message.method === "Network.loadingFailed"
      && message.params.canceled !== true
      && message.params.errorText !== "net::ERR_ABORTED") {
      result.errors.push(message.params.errorText)
    }
    if (message.method === "Network.responseReceived") {
      const response = message.params.response
      const url = new URL(response.url)
      result.responses.push({
        challenge: response.headers["www-authenticate"] ?? response.headers["WWW-Authenticate"] ?? null,
        path: url.pathname,
        status: response.status,
      })
      if (response.status >= 400) {
        const expected = response.status === 401 && url.origin === origin
          && (url.pathname === "/auth/session" || url.pathname.startsWith("/api/"))
        if (!expected) result.errors.push(`${response.status}:${response.url}`)
      }
    }
    if (message.method === "Network.requestWillBeSent") {
      const requested = message.params.request.url
      if (/^https?:/.test(requested) && new URL(requested).origin !== origin) result.external.push(requested)
    }
  })
}

function enablePage(cdp) {
  return Promise.all([
    cdp.send("Page.enable"),
    cdp.send("Runtime.enable"),
    cdp.send("Network.enable"),
    cdp.send("Log.enable"),
  ])
}

async function submitLogin(cdp) {
  await cdp.evaluate(`(() => {
    const set = (name, value) => {
      const input = document.querySelector('[name="' + name + '"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, value)
      input.dispatchEvent(new Event("input", { bubbles: true }))
    }
    set("username", "artifact")
    set("password", "wire")
    document.querySelector("form").requestSubmit()
  })()`)
}

function assertBootstrapBeforeApi(requests, restored) {
  const bootstrap = requests.findIndex(({ method, path }) => method === "GET" && path === "/auth/session")
  const firstApi = requests.findIndex(({ path }) => path.startsWith("/api/"))
  assert.ok(bootstrap >= 0, JSON.stringify(requests, null, 2))
  if (!restored) {
    assert.equal(firstApi, -1, JSON.stringify(requests, null, 2))
    return
  }
  assert.ok(firstApi > bootstrap, JSON.stringify(requests, null, 2))
  assert.equal(requests.slice(0, bootstrap).some(({ path }) => path.startsWith("/api/")), false)
  assert.equal(requests.some(({ method, path }) => method === "POST" && path === "/auth/session"), false)
}

function expectedUnauthorizedLog(message) {
  return /^Failed to load resource: the server responded with a status of 401 \(Unauthorized\)$/.test(message)
}

async function waitForRequests(predicate) {
  const started = Date.now()
  while (Date.now() - started < 5_000) {
    if (predicate()) return
    await delay(20)
  }
  throw new Error("timed out waiting for request")
}

function launchBrowser(profile, arguments_ = []) {
  const executable = browserExecutable()
  const browser = spawn(executable, [
    "--headless",
    // Headless Chromium without input devices reports `pointer: none` and
    // `hover: none`, which the interface reads as a coarse pointer. Start from
    // desktop semantics; touch emulation still switches to coarse where a test asks.
    "--blink-settings=primaryPointerType=4,primaryHoverType=2,availablePointerTypes=4,availableHoverTypes=2",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-dev-shm-usage",
    "--disable-extensions",
    "--disable-gpu",
    "--metrics-recording-only",
    "--no-first-run",
    "--no-sandbox",
    "--remote-debugging-address=127.0.0.1",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    ...arguments_,
    "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] })
  browser.diagnostics = ""
  browser.stderr.setEncoding("utf8")
  browser.stderr.on("data", (chunk) => { browser.diagnostics = `${browser.diagnostics}${chunk}`.slice(-12_000) })
  return browser
}

function browserExecutable() {
  const candidates = [
    process.env.CHROME_BIN,
    "chromium-browser",
    "chromium",
    "google-chrome-stable",
    "google-chrome",
  ].filter((candidate) => candidate !== undefined && candidate !== "")
  for (const candidate of candidates) {
    const result = spawnSync(candidate, ["--version"], { stdio: "ignore" })
    if (result.status === 0) return candidate
  }
  throw new Error(`no Chromium executable found (tried ${candidates.join(", ")})`)
}

async function browserDebugPort(profile, browser) {
  const activePort = join(profile, "DevToolsActivePort")
  const started = Date.now()
  while (Date.now() - started < 10_000) {
    if (browser.exitCode !== null) throw new Error(`Chromium exited before startup:\n${browser.diagnostics}`)
    try {
      const [port] = (await readFile(activePort, "utf8")).split("\n")
      if (/^\d+$/.test(port ?? "")) return port
    } catch (reason) {
      if (reason?.code !== "ENOENT") throw reason
    }
    await delay(40)
  }
  throw new Error(`timed out starting Chromium:\n${browser.diagnostics}`)
}

async function pageSocket(port, targetId) {
  const started = Date.now()
  while (Date.now() - started < 5_000) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((response) => response.json())
      const target = targets.find((candidate) => (
        candidate.type === "page" && (targetId === undefined || candidate.id === targetId)
      ))
      if (target !== undefined) {
        const socket = new WebSocket(target.webSocketDebuggerUrl)
        await new Promise((resolve, reject) => {
          socket.addEventListener("open", resolve, { once: true })
          socket.addEventListener("error", reject, { once: true })
        })
        return socket
      }
    } catch {}
    await delay(40)
  }
  throw new Error("Chromium did not expose a page target")
}

function cdpSession(socket) {
  let sequence = 0
  const pending = new Map()
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data)
    if (message.id === undefined) return
    const request = pending.get(message.id)
    if (request === undefined) return
    pending.delete(message.id)
    if (message.error === undefined) request.resolve(message.result)
    else request.reject(new Error(`${request.method}: ${message.error.message}`))
  })
  const send = (method, params = {}) => {
    const id = ++sequence
    return new Promise((resolve, reject) => {
      pending.set(id, { method, reject, resolve })
      socket.send(JSON.stringify({ id, method, params }))
    })
  }
  const evaluate = async (expression) => {
    const response = await send("Runtime.evaluate", {
      awaitPromise: true,
      expression,
      returnByValue: true,
      userGesture: true,
    })
    if (response.exceptionDetails !== undefined) {
      throw new Error(response.exceptionDetails.exception?.description ?? response.exceptionDetails.text)
    }
    return response.result.value
  }
  const waitFor = async (expression, description, timeout = 10_000) => {
    const started = Date.now()
    while (Date.now() - started < timeout) {
      if (await evaluate(expression)) return
      await delay(40)
    }
    throw new Error(`timed out waiting for ${description}`)
  }
  return { evaluate, send, waitFor }
}

async function stopBrowser(browser) {
  if (browser.exitCode !== null) return
  browser.kill("SIGTERM")
  await Promise.race([
    new Promise((resolve) => browser.once("exit", resolve)),
    delay(2_000).then(() => { if (browser.exitCode === null) browser.kill("SIGKILL") }),
  ])
}

async function removeBrowserProfile(profile) {
  await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 })
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}

test("mixed-cadence shared cursor uses one recorded domain for pointer and both keyboard paths", { timeout: 90_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const page = { errors: [], external: [], responses: [] }
  const base = HOUR + 1_800_000_000
  const activityOne = base + 1
  const activityTwo = base + 4_000
  const five = base + 5_000_000
  const ten = base + 10_000_000
  let snapshotRequests = 0
  const activityRecords = snapshotRecords().filter((record) => record.record === "layout"
    ? record.layout.logical_name === "pg_stat_activity"
    : record.record === "row" && record.type_id === "1001004")
    .map((record, index) => {
      if (record.record !== "row") return record
      const timestamp = index === 1 ? activityOne : activityTwo
      return { ...record, timestamp: String(timestamp), values: [String(timestamp), ...record.values.slice(1)] }
    })
  const timeline = [
    { record: "hour", from: String(HOUR), to: String(HOUR + HOUR_US - 1), available_hours: [String(HOUR)] },
    { record: "catalog", from: String(HOUR), to: String(HOUR + HOUR_US - 1), source_families: [{ name: "postgresql", configured: true, present: true, metrics_present: true }] },
    {
      record: "finished_segment", id: SEGMENT, min_ts: String(HOUR), max_ts: String(HOUR + HOUR_US - 1),
      sections: [{ logical_name: "pg_stat_activity", physical_name: "pg_stat_activity", type_id: "1001004", implementation: "postgresql", source_family: "postgresql", rows: "2", bytes: "256" }],
    },
    { record: "index", segment: { id: SEGMENT }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "os_health", ts: String(base), identity: {}, value: 80 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(base), identity: {}, value: 75 },
    ...[-30_000_000, 0, 30_000_000].map((offset, index) => ({ record: "lane", segment_id: SEGMENT, lane: "pg_waiting", ts: String(base + offset), value: index + 1 })),
    { record: "lane_context", segment_id: SEGMENT, postgresql_interval_seconds: null, environment: 0 },
    ...[0, 5_000_000, 10_000_000, 15_000_000].map((offset, index) => ({ record: "lane", segment_id: SEGMENT, lane: "cpu_busy", ts: String(base + offset), value: 20 + index })),
  ]
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") return ndjson(response, url.searchParams.has("section") ? [] : timeline)
    if (url.pathname === "/api/instance-label") {
      response.writeHead(200, { "Content-Type": "application/json" })
      response.end(JSON.stringify({ record: "instance_label", database: "artifact_db" }))
      return
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      snapshotRequests += 1
      return ndjson(response, activityRecords)
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("mixed-cadence browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    const waitAt = async (timestamp, label) => {
      await cdp.waitFor(`new URL(location.href).searchParams.get("at") === "${timestamp}"`, label, 15_000)
      await cdp.waitFor(
        `document.querySelector('[data-testid="cursor-behind"]') === null && document.querySelector('[data-testid="hour-timeline"]')?.dataset.navigationCount === "8"`,
        `${label} snapshot`,
        15_000,
      )
      assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"]')?.dataset.selectedTimestamp`), String(timestamp))
    }
    for (const width of [800, 1280]) {
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
      await cdp.send("Page.navigate", { url: `${origin}/?at=${ten}&view=pg.activity` })
      await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-row') !== null`, `${width}px Activity rows`, 15_000)
      if (await cdp.evaluate(`document.documentElement.lang !== "ru"`)) {
        await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
        await cdp.waitFor(`document.documentElement.lang === "ru"`, `${width}px RU locale`)
      }
      await waitAt(ten, `${width}px initial cursor`)
      const geometry = await cdp.evaluate(`(() => {
        const figure = document.querySelector('[data-testid="hour-timeline"]')
        const plot = figure.querySelector('.u-over')
        const tabs = document.querySelector('.pg-tabs')
        const figureBox = figure.getBoundingClientRect()
        const plotBox = plot.getBoundingClientRect()
        const tabsBox = tabs.getBoundingClientRect()
        return {
          count: Number(figure.dataset.navigationCount),
          figureBottom: figureBox.bottom,
          figureLeft: figureBox.left,
          figureRight: figureBox.right,
          plotBottom: plotBox.bottom,
          plotLeft: plotBox.left,
          plotRight: plotBox.right,
          tabsTop: tabsBox.top,
        }
      })()`)
      assert.equal(geometry.count, 8, `${width}:${JSON.stringify(geometry)}`)
      assert.ok(geometry.figureBottom <= geometry.tabsTop + 0.75, `${width}:${JSON.stringify(geometry)}`)
      assert.ok(geometry.plotLeft >= geometry.figureLeft && geometry.plotRight <= geometry.figureRight && geometry.plotBottom <= geometry.figureBottom, `${width}:${JSON.stringify(geometry)}`)

      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowLeft' }))`)
      await waitAt(five, `${width}px global five-second cursor`)
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowLeft' }))`)
      await waitAt(activityTwo, `${width}px second recorded Activity cursor`)
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'ArrowLeft' }))`)
      await waitAt(activityOne, `${width}px first recorded Activity cursor`)

      await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] input.chart-navigator').focus()`)
      await cdp.send("Input.dispatchKeyEvent", { code: "ArrowRight", key: "ArrowRight", nativeVirtualKeyCode: 39, type: "keyDown", windowsVirtualKeyCode: 39 })
      await cdp.send("Input.dispatchKeyEvent", { code: "ArrowRight", key: "ArrowRight", nativeVirtualKeyCode: 39, type: "keyUp", windowsVirtualKeyCode: 39 })
      await waitAt(activityTwo, `${width}px chart keyboard cursor`)

      const points = await cdp.evaluate(`(() => {
        const figure = document.querySelector('[data-testid="hour-timeline"]')
        const host = figure.querySelector('.uplot-host').getBoundingClientRect()
        const plot = figure.querySelector('.u-over').getBoundingClientRect()
        const extendedEnd = ${HOUR + HOUR_US} + ${HOUR_US} * 38 / Math.max(1, host.width - 84)
        figure.querySelector('.uplot').dataset.gestureIdentity = 'same'
        return [${base - 30_000_000}, ${base}, ${five}, ${ten}].map((timestamp) => ({
          timestamp,
          x: plot.left + (timestamp - ${HOUR}) / (extendedEnd - ${HOUR}) * plot.width,
          y: plot.top + plot.height / 2,
        }))
      })()`)
      const requestsBeforeTravel = snapshotRequests
      const clocks = []
      for (const point of points) {
        await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: point.x, y: point.y })
        await cdp.waitFor(`document.querySelector('[data-testid="hour-timeline"]')?.dataset.selectedTimestamp === "${point.timestamp}"`, `${width}px preview ${point.timestamp}`)
        clocks.push(await cdp.evaluate(`document.querySelector('[data-testid="cursor-time"]')?.textContent`))
      }
      assert.equal(await cdp.evaluate(`new URL(location.href).searchParams.get("at")`), String(activityTwo), `${width}px pointer travel leaves URL committed`)
      assert.equal(snapshotRequests, requestsBeforeTravel, `${width}px pointer travel emits no snapshots`)
      assert.ok(new Set(clocks).size >= 3, `${width}px clock follows pointer: ${JSON.stringify(clocks)}`)
      assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="hour-timeline"] .uplot')?.dataset.gestureIdentity`), "same", `${width}px plot survives pointer travel`)
      const point = points.at(-1)
      await cdp.send("Input.dispatchMouseEvent", { button: "left", buttons: 1, clickCount: 1, type: "mousePressed", x: point.x, y: point.y })
      await cdp.send("Input.dispatchMouseEvent", { button: "left", buttons: 0, clickCount: 1, type: "mouseReleased", x: point.x, y: point.y })
      await waitAt(ten, `${width}px pointer cursor`)
      assert.equal(snapshotRequests, requestsBeforeTravel + 1, `${width}px pointer release emits one snapshot`)
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

test("narrow controls stay contained and help never changes selection", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  let historyFailure = false
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      if (url.searchParams.has("section") && historyFailure) return ndjson(response, [{ record: "error", error: "history unavailable" }])
      return ndjson(response, url.searchParams.has("section") ? [] : [
        ...timelineRecords(HOUR),
        { record: "lane_context", segment_id: SEGMENT, postgresql_interval_seconds: null, environment: 0 },
        { record: "lane", segment_id: SEGMENT, lane: "cpu_busy", ts: String(AT), value: 42 },
      ])
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) {
      return ndjson(response, url.searchParams.getAll("section").includes("pg_stat_activity") ? snapshotRecords() : systemSnapshotRecords())
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("narrow browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 480 })
    await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"]') !== null`, "the host ledger", 15_000)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the narrow System view", 15_000)
    await settleLayout(cdp)

    const closed = await cdp.evaluate(`(() => {
      const actions = document.querySelector('.top-actions').getBoundingClientRect()
      return {
        actions: { left: actions.left, right: actions.right, width: actions.width },
        client: document.documentElement.clientWidth,
        coarse: matchMedia('(pointer: coarse)').matches,
        nested: document.querySelectorAll('button button').length,
        scroll: document.documentElement.scrollWidth,
        topbarScroll: document.querySelector('.topbar').scrollWidth,
      }
    })()`)
    assert.equal(closed.coarse, true)
    assert.equal(closed.nested, 0)
    assert.ok(closed.actions.left >= 0 && closed.actions.right <= closed.client, JSON.stringify(closed))
    assert.ok(closed.scroll <= closed.client && closed.topbarScroll <= closed.client, JSON.stringify(closed))

    await cdp.evaluate(`document.querySelector('[data-testid="locale-ru"]').click()`)
    await settleLayout(cdp)
    const russianActions = await cdp.evaluate(`(() => {
      const actions = document.querySelector('.top-actions').getBoundingClientRect()
      return {
        client: document.documentElement.clientWidth,
        left: actions.left,
        right: actions.right,
        scroll: document.documentElement.scrollWidth,
        topbarScroll: document.querySelector('.topbar').scrollWidth,
      }
    })()`)
    assert.ok(russianActions.left >= 0 && russianActions.right <= russianActions.client, JSON.stringify(russianActions))
    assert.ok(russianActions.scroll <= russianActions.client && russianActions.topbarScroll <= russianActions.client, JSON.stringify(russianActions))
    await cdp.evaluate(`document.querySelector('[data-testid="locale-en"]').click()`)
    await settleLayout(cdp)

    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "the narrow hour popover")
    await settleLayout(cdp)
    const popover = await cdp.evaluate(`(() => {
      const rect = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
      return { client: document.documentElement.clientWidth, left: rect.left, right: rect.right, scroll: document.documentElement.scrollWidth }
    })()`)
    assert.ok(popover.left >= 0 && popover.right <= popover.client && popover.scroll <= popover.client, JSON.stringify(popover))
    await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' }))`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "Escape closing the hour popover")
    assert.equal(await cdp.evaluate(`document.activeElement?.dataset.testid`), "hour-picker-trigger")

    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the CPU chips")
    await cdp.evaluate(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]').click()`)
    const selectedMetric = await cdp.evaluate(`document.querySelector('[data-testid="system-group-chart-cpu"] button[aria-pressed="true"]')?.dataset.testid`)
    await cdp.evaluate(`document.querySelector('.use-header-cell .help-dot').click()`)
    await cdp.waitFor(`document.querySelector('[role="tooltip"]') !== null`, "the USE methodology help")
    assert.equal(await cdp.evaluate(`document.querySelector('[data-testid="system-group-chart-cpu"] button[aria-pressed="true"]')?.dataset.testid`), selectedMetric)
    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)

    const selectedLane = await cdp.evaluate(`document.querySelector('.lane-select[aria-pressed="true"]')?.textContent`)
    await cdp.evaluate(`(() => {
      const other = [...document.querySelectorAll('.lane-label')].find((lane) => lane.querySelector('.lane-select').getAttribute('aria-pressed') !== 'true')
      other.querySelector('.help-dot').click()
    })()`)
    await cdp.waitFor(`document.querySelector('[role="tooltip"]') !== null`, "the timeline lane help")
    assert.equal(await cdp.evaluate(`document.querySelector('.lane-select[aria-pressed="true"]')?.textContent`), selectedLane)
    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)

    await cdp.evaluate(`document.querySelectorAll('.source-tabs button')[2].click()`)
    await cdp.waitFor(`document.querySelector('.pg-tabs') !== null`, "the PostgreSQL tabs", 15_000)
    await cdp.evaluate(`([...document.querySelectorAll('.pg-tabs button')].find((button) => button.textContent.includes('Activity')) ?? document.querySelectorAll('.pg-tabs button')[1]).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-activity-table"] .entity-header-cell > .label-help .help-dot') !== null`, "the Activity header help", 15_000)
    const headerHelp = await cdp.evaluate(`(() => {
      const button = document.querySelector('[data-testid="pg-activity-table"] .entity-header-cell > .label-help .help-dot')
      button.scrollIntoView({ block: "nearest", inline: "nearest" })
      const rect = button.getBoundingClientRect()
      const style = getComputedStyle(button.closest('.label-help'))
      const after = getComputedStyle(button, "::after")
      const x = rect.left + rect.width / 2
      const y = rect.top + rect.height / 2
      // The mark stays small; the touch target grows invisibly via ::after,
      // and every side of it has to survive the cell and the scroll port.
      const reach = (dx, dy) => {
        let grown = 0
        for (let step = 1; step <= 15; step += 1) {
          if (document.elementFromPoint(x + dx * step, y + dy * step) !== button) break
          grown = step
        }
        return grown
      }
      button.click()
      return {
        afterContent: after.content,
        afterInset: after.inset,
        height: rect.height,
        opacity: style.opacity,
        pointerEvents: style.pointerEvents,
        reach: { down: reach(0, 1), left: reach(-1, 0), right: reach(1, 0), up: reach(0, -1) },
        width: rect.width,
      }
    })()`)
    assert.deepEqual(
      [headerHelp.afterContent, headerHelp.afterInset, headerHelp.height, headerHelp.opacity, headerHelp.pointerEvents, headerHelp.width],
      ['""', "-11px", 14, "1", "auto", 14],
      JSON.stringify(headerHelp),
    )
    // 36x36 around a 14px mark, reached from every side. Flush against the
    // table's scroll port the right half used to be clipped to 6px; the mark
    // now steps in and the port keeps scroll padding, and nothing of the
    // neighbouring column is taken.
    for (const [side, reach] of Object.entries(headerHelp.reach)) {
      assert.ok(reach >= 14 && reach <= 15, `${side} touch reach: ${JSON.stringify(headerHelp)}`)
    }
    assert.ok(
      headerHelp.width + headerHelp.reach.left + headerHelp.reach.right >= 42
        && headerHelp.height + headerHelp.reach.up + headerHelp.reach.down >= 42,
      `complete touch target: ${JSON.stringify(headerHelp)}`,
    )
    await cdp.waitFor(`document.querySelector('[role="tooltip"]') !== null`, "the first-touch table help")
    assert.equal(await cdp.evaluate(`document.querySelectorAll('button button').length`), 0)

    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)
    historyFailure = true
    await cdp.evaluate(`document.querySelector('[data-testid="pg-activity-table"] .entity-row').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="pg-detail"]') !== null`, "the Activity detail")
    await cdp.evaluate(`([...document.querySelectorAll('.inspector-tabs button')].find((button) => button.textContent === 'Chart')).click()`)
    await cdp.waitFor(`document.querySelector('.inspector-chart-slot [data-testid="series-status"][data-status="error"]') !== null`, "the Activity history error")
    assert.equal(await cdp.evaluate(`document.querySelector('.inspector-chart-slot [data-testid="series-status"][data-status="error"]').textContent`), "Could not load history")
    const failedHistory = await cdp.evaluate(`(() => {
      const detail = document.querySelector('.inspector-chart-slot')
      const status = detail.querySelector('[data-testid="series-status"][data-status="error"]').getBoundingClientRect()
      return { chart: detail.querySelector('.uplot-host') !== null, statusHeight: status.height, text: detail.querySelector('.series-chart')?.textContent ?? '' }
    })()`)
    assert.equal(failedHistory.chart, true, JSON.stringify(failedHistory))
    assert.ok(failedHistory.statusHeight <= 40, JSON.stringify(failedHistory))
    historyFailure = false

    await cdp.evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width: 960 })
    await cdp.evaluate(`([...document.querySelectorAll('.source-tabs button')].find((button) => button.textContent.trim() === 'Host')).click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="use-toggle-cpu"]') !== null`, "the cpu ledger row", 15_000)
    await cdp.evaluate(`(() => { const toggle = document.querySelector('[data-testid="use-toggle-cpu"]'); if (toggle && toggle.getAttribute("aria-expanded") !== "true") toggle.click() })()`)
    await cdp.waitFor(`document.querySelector('[data-testid="system-metric-cpu_used_cores"]') !== null`, "the 960px System view", 15_000)
    await settleLayout(cdp)
    await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "the 960px hour popover")
    const standard = await cdp.evaluate(`(() => {
      const actions = document.querySelector('.top-actions').getBoundingClientRect()
      const popover = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
      const client = document.documentElement.clientWidth
      return { actionsLeft: actions.left, actionsRight: actions.right, client, popoverLeft: popover.left, popoverRight: popover.right, scroll: document.documentElement.scrollWidth }
    })()`)
    assert.ok(standard.actionsLeft >= 0 && standard.actionsRight <= standard.client, JSON.stringify(standard))
    assert.ok(standard.popoverLeft >= 0 && standard.popoverRight <= standard.client && standard.scroll <= standard.client, JSON.stringify(standard))
    for (const width of [521, 580, 600, 760, 761]) {
      await cdp.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, key: 'Escape' }))`)
      await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 900, mobile: false, width })
      await settleLayout(cdp)
      await cdp.evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
      await cdp.waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, `the ${width}px hour popover`)
      const intermediate = await cdp.evaluate(`(() => {
        const rect = document.querySelector('[data-testid="hour-popover"]').getBoundingClientRect()
        const client = document.documentElement.clientWidth
        return { client, left: rect.left, right: rect.right, scroll: document.documentElement.scrollWidth }
      })()`)
      assert.ok(intermediate.left >= 0 && intermediate.right <= intermediate.client && intermediate.scroll <= intermediate.client, `${width}:${JSON.stringify(intermediate)}`)
    }
    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})

// A narrow-width rule declared before the base value it must beat loses the
// cascade and does nothing. Both of these shipped that way and passed review,
// because the responsive suite matches the text of the stylesheet rather than
// what the browser resolves. Assert the resolved geometry instead.
test("phone width keeps narrow rules winning and nothing reserving height", { timeout: 60_000 }, async () => {
  const html = gunzipSync(await readFile(ARTIFACT))
  const authState = { valid: true }
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1")
    if (url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
      response.end(html)
      return
    }
    if (url.pathname === "/auth/session") return answerSession(request, response, authState)
    if (url.pathname === "/api/instance-label") return answerInstanceLabel(response)
    if (url.pathname.startsWith("/api/") && !browserIsAuthenticated(request, authState)) return unauthorized(response)
    if (url.pathname === "/api/heatmap") return answerHeatmap(url, response)
    if (url.pathname === "/api/catalog") return ndjson(response, [])
    if (url.pathname === "/api/hour") {
      return ndjson(response, url.searchParams.has("section") ? [] : [
        ...timelineRecords(HOUR),
        { record: "lane_context", segment_id: SEGMENT, postgresql_interval_seconds: null, environment: 0 },
        { record: "lane", segment_id: SEGMENT, lane: "cpu_busy", ts: String(AT), value: 42 },
      ])
    }
    if (url.pathname === `/api/segments/${SEGMENT}/snapshot`) return ndjson(response, systemSnapshotRecords())
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", resolve)
  })
  const address = server.address()
  if (address === null || typeof address === "string") throw new Error("cascade browser server has no TCP address")
  const origin = `http://127.0.0.1:${address.port}`
  const profile = await mkdtemp(join(tmpdir(), "b-"))
  const browser = launchBrowser(profile)
  const page = { errors: [], external: [], responses: [] }
  let socket
  try {
    const debugPort = await browserDebugPort(profile, browser)
    socket = await pageSocket(debugPort)
    const cdp = cdpSession(socket)
    trackPage(socket, origin, page)
    await enablePage(cdp)
    await cdp.send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height: 915, mobile: true, width: 412 })
    await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 })
    await cdp.send("Network.setCookie", { name: "kronika_session", url: origin, value: SESSION_COOKIE.slice(SESSION_COOKIE.indexOf("=") + 1) })
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=host.overview` })
    await cdp.waitFor(`document.querySelector('[data-testid="use-table"] .use-row') !== null`, "a host ledger row", 15_000)
    await settleLayout(cdp)

    const resolved = await cdp.evaluate(`(() => {
      const row = document.querySelector('[data-testid="use-table"] .use-row')
      const tracks = getComputedStyle(row).gridTemplateColumns.split(" ")
      const name = row.querySelector('.use-resource')
      const cell = row.querySelector('.use-cell')
      return {
        client: document.documentElement.clientWidth,
        first: tracks[0], tracks: tracks.length,
        nameSpans: name === null ? null : Math.round(name.getBoundingClientRect().width),
        nameBottom: name === null ? null : Math.round(name.getBoundingClientRect().bottom),
        cellTop: cell === null ? null : Math.round(cell.getBoundingClientRect().top),
        rowWidth: Math.round(row.getBoundingClientRect().width),
      }
    })()`)
    // At or below 520px four columns left a reading 88px for a label, a number
    // and its help mark. The resource name takes the whole row and the three
    // readings share the width under it.
    assert.equal(resolved.client, 412, JSON.stringify(resolved))
    assert.equal(resolved.tracks, 3, JSON.stringify(resolved))
    assert.equal(resolved.nameSpans, resolved.rowWidth, JSON.stringify(resolved))
    assert.ok(resolved.cellTop >= resolved.nameBottom - .5, JSON.stringify(resolved))

    // A phone has no room for a reserved list height or a strip that stacks
    // its readings; both pushed real rows below the fold.
    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=events` })
    await cdp.waitFor(`document.querySelector('[data-testid="events-console"]') !== null`, "the events console", 15_000)
    await settleLayout(cdp)
    const console412 = await cdp.evaluate(`(() => {
      const shell = document.querySelector('[data-testid="events-console"]')
      const reserving = [...shell.querySelectorAll("div")].filter((el) => getComputedStyle(el).minHeight === "390px")
      return { reserving: reserving.length, divs: shell.querySelectorAll("div").length }
    })()`)
    assert.ok(console412.divs > 0, JSON.stringify(console412))
    assert.equal(console412.reserving, 0, JSON.stringify(console412))

    await cdp.send("Page.navigate", { url: `${origin}/?at=${AT}&view=processes` })
    await cdp.waitFor(`document.querySelector('.process-summary-inline') !== null`, "the process summary strip", 15_000)
    await settleLayout(cdp)
    const strip = await cdp.evaluate(`(() => {
      const el = document.querySelector('.process-summary-inline')
      return { overflowX: getComputedStyle(el).overflowX, wrap: getComputedStyle(el).flexWrap }
    })()`)
    assert.equal(strip.wrap, "nowrap", JSON.stringify(strip))
    assert.equal(strip.overflowX, "auto", JSON.stringify(strip))

    assert.deepEqual(page.errors, [])
    assert.deepEqual(page.external, [])
  } finally {
    socket?.close()
    await stopBrowser(browser)
    await new Promise((resolve) => server.close(resolve))
    await removeBrowserProfile(profile)
  }
})
