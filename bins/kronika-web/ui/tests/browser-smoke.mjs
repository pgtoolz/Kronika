import assert from "node:assert/strict"

const [debugPort, pageUrl] = process.argv.slice(2)
if (debugPort === undefined || pageUrl === undefined) {
  throw new Error("usage: node tests/browser-smoke.mjs DEBUG_PORT PAGE_URL")
}

const requestedUrl = new URL(pageUrl)
const origin = requestedUrl.origin
const reportMode = requestedUrl.protocol === "file:"
const targets = await fetch(`http://127.0.0.1:${debugPort}/json/list`).then((response) => response.json())
const target = targets.find((candidate) => candidate.type === "page")
if (target === undefined) throw new Error("Chromium did not expose a page target")

const socket = new WebSocket(target.webSocketDebuggerUrl)
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true })
  socket.addEventListener("error", reject, { once: true })
})
let sequence = 0
const pending = new Map()
socket.addEventListener("close", () => {
  for (const request of pending.values()) request.reject(new Error("Chromium closed the page connection"))
  pending.clear()
})
const errors = []
const external = []
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data)
  if (message.id !== undefined) {
    const request = pending.get(message.id)
    if (request === undefined) return
    pending.delete(message.id)
    if (message.error === undefined) request.resolve(message.result)
    else request.reject(new Error(`${request.method}: ${message.error.message}`))
    return
  }
  if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text)
  if (message.method === "Runtime.consoleAPICalled" && ["error", "assert"].includes(message.params.type)) {
    errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "))
  }
  if (message.method === "Log.entryAdded" && message.params.entry.level === "error") errors.push(message.params.entry.text)
  if (message.method === "Network.loadingFailed" && message.params.canceled !== true && message.params.errorText !== "net::ERR_ABORTED") errors.push(message.params.errorText)
  if (message.method === "Network.responseReceived" && message.params.response.status >= 400) {
    const response = message.params.response
    const url = new URL(response.url)
    const unsignedBootstrap = response.status === 401 && url.origin === origin && url.pathname === "/auth/session"
    if (!unsignedBootstrap) errors.push(`${response.status}:${response.url}`)
  }
  if (message.method === "Network.requestWillBeSent") {
    const rawUrl = message.params.request.url
    const url = new URL(rawUrl)
    const reportDocument = reportMode
      && url.protocol === "file:"
      && url.pathname === requestedUrl.pathname
    if (!rawUrl.startsWith("data:") && !rawUrl.startsWith("blob:")
        && (reportMode ? !reportDocument : url.origin !== origin)) external.push(rawUrl)
  }
})

function send(method, params = {}) {
  const id = ++sequence
  return new Promise((resolve, reject) => {
    pending.set(id, { method, reject, resolve })
    socket.send(JSON.stringify({ id, method, params }))
  })
}

async function evaluate(expression) {
  const response = await send("Runtime.evaluate", { awaitPromise: true, expression, returnByValue: true, userGesture: true })
  if (response.exceptionDetails !== undefined) throw new Error(response.exceptionDetails.text)
  return response.result.value
}

async function waitFor(expression, description, timeout = 10_000) {
  const started = Date.now()
  while (Date.now() - started < timeout) {
    if (await evaluate(expression)) return
    await new Promise((resolve) => setTimeout(resolve, 40))
  }
  throw new Error(`timed out waiting for ${description}: ${JSON.stringify(errors)}`)
}

await Promise.all([send("Page.enable"), send("Runtime.enable"), send("Network.enable"), send("Log.enable")])
errors.length = 0
external.length = 0
await send("Page.navigate", { url: pageUrl })
await waitFor(`document.readyState === "complete" && (document.querySelector('[data-testid="login-card"]') !== null || document.querySelector('[data-testid="hour-picker-trigger"]') !== null)`, "login or application")
if (await evaluate(`document.querySelector('[data-testid="login-card"]') !== null`)) {
  await evaluate(`(() => {
    const set = (name, stored) => {
      const input = document.querySelector('[name="' + name + '"]')
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, stored)
      input.dispatchEvent(new Event("input", { bubbles: true }))
    }
    set("username", "smoke")
    set("password", "smoke")
    document.querySelector("form").requestSubmit()
  })()`)
}
await waitFor(`document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "application")
if (reportMode) {
  await waitFor(`(() => {
    const at = Number(new URL(location.href).searchParams.get("at"))
    return Number.isSafeInteger(at) && at > 0
      && document.querySelectorAll(".process-table .entity-row").length > 0
  })()`, "canonical report address", 30_000)
  const beforeReload = await evaluate(`location.href`)
  const reportState = await evaluate(`(() => ({
    export: document.querySelector('[data-testid="export-trigger"]') !== null,
    login: document.querySelector('[data-testid="login-card"]') !== null,
    mcp: document.querySelector('[data-testid="mcp-trigger"]') !== null,
    refresh: document.querySelector('[data-testid="refresh-action"]') !== null,
    logout: document.querySelector('[data-testid="logout-action"]') !== null,
    pathname: location.pathname,
  }))()`)
  assert.equal(reportState.export, false)
  assert.equal(reportState.login, false)
  assert.equal(reportState.mcp, false)
  assert.equal(reportState.refresh, false)
  assert.equal(reportState.logout, false)
  assert.equal(reportState.pathname, requestedUrl.pathname)
  await send("Page.reload")
  await waitFor(`document.readyState === "complete" && document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "reloaded report", 30_000)
  await waitFor(`location.href === ${JSON.stringify(beforeReload)}`, "reloaded report address", 30_000)
  assert.equal(await evaluate(`location.href`), beforeReload)
  assert.equal(await evaluate(`location.pathname`), requestedUrl.pathname)

  const visibleRange = await evaluate(`(() => {
    const runtime = globalThis.__KRONIKA_REPORT_RUNTIME__
    const from = Number(runtime?.visibleFrom)
    const toExclusive = Number(runtime?.visibleToExclusive)
    return Number.isSafeInteger(from) && Number.isSafeInteger(toExclusive) && from < toExclusive
      ? { from, toExclusive }
      : null
  })()`)
  if (visibleRange !== null) {
    const invalidAt = visibleRange.toExclusive + 3_600_000_000
    const previousTimeOrigin = await evaluate(`performance.timeOrigin`)
    const invalidUrl = await evaluate(`(() => {
      const url = new URL(location.href)
      url.searchParams.set("at", ${JSON.stringify(String(invalidAt))})
      return url.href
    })()`)
    await send("Page.navigate", { url: invalidUrl })
    await waitFor(`performance.timeOrigin !== ${previousTimeOrigin} && document.readyState === "complete"`, "out-of-range report reload", 30_000)
    await waitFor(`(() => {
      const at = Number(new URL(location.href).searchParams.get("at"))
      return at >= ${visibleRange.from} && at < ${visibleRange.toExclusive}
        && document.querySelectorAll(".process-table .entity-row").length > 0
    })()`, "out-of-range initial report address recovery", 30_000)
    assert.notEqual(await evaluate(`new URL(location.href).searchParams.get("at")`), String(invalidAt))

    await evaluate(`(() => {
      const url = new URL(location.href)
      url.searchParams.set("at", ${JSON.stringify(String(invalidAt))})
      history.pushState({}, "", url)
      dispatchEvent(new PopStateEvent("popstate"))
    })()`)
    await waitFor(`(() => {
      const at = Number(new URL(location.href).searchParams.get("at"))
      return at >= ${visibleRange.from} && at < ${visibleRange.toExclusive}
        && document.querySelectorAll(".process-table .entity-row").length > 0
    })()`, "out-of-range report address recovery", 30_000)
    assert.notEqual(await evaluate(`new URL(location.href).searchParams.get("at")`), String(invalidAt))

    await evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
    await waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "bounded report hour picker")
    const pickerHours = await evaluate(`[...document.querySelectorAll('[data-testid="hour-cell"]')].map((node) => Number(node.dataset.instant))`)
    assert.ok(pickerHours.length > 0)
    assert.equal(pickerHours.includes(invalidAt), false)
    assert.equal(pickerHours.every((hour) => hour < visibleRange.toExclusive && hour + 3_600_000_000 > visibleRange.from), true)
    await evaluate(`dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`)
  }

  await evaluate(`document.querySelectorAll('.source-tabs button')[3].click()`)
  await waitFor(`document.querySelector('[data-testid="events-console"]') !== null`, "report Events console", 30_000)
  await waitFor(`(() => {
    const console = document.querySelector('[data-testid="events-console"]')
    return console !== null && console.querySelector('[data-loading="true"]') === null
      && console.querySelector(':scope > header [role]') !== null
  })()`, "settled report Events", 30_000)
  const eventState = await evaluate(`(() => {
    const console = document.querySelector('[data-testid="events-console"]')
    return {
      failed: console.querySelector(':scope > header [role="alert"]') !== null,
      groups: console.querySelectorAll('[data-testid="event-entry"]').length,
      status: console.querySelector(':scope > header [role]')?.textContent ?? "",
    }
  })()`)
  assert.equal(eventState.failed, false, JSON.stringify(eventState))
}
await evaluate(`document.querySelector('[data-testid="process-tab"]').click()`)
await waitFor(`document.querySelectorAll(".process-table .entity-row").length > 0`, "process rows", reportMode ? 30_000 : 10_000)
await evaluate(`document.querySelector(".process-table .entity-row").click()`)
let result
if (reportMode) {
  // A standalone segment need not have a predecessor-derived process rate,
  // so its real fixture can open Detail without drawing a history line.
  await waitFor(`document.querySelector('[data-testid="process-dock"]') !== null`, "process detail", 30_000)
  result = await evaluate(`(() => ({
    command: document.querySelector('[data-testid="process-cmdline"]')?.textContent ?? "",
    detailRows: document.querySelectorAll('[data-testid="process-dock"] dl > div').length,
    pathname: location.pathname,
    rows: document.querySelectorAll(".process-table .entity-row").length,
  }))()`)
  assert.ok(result.command.length > 0)
  assert.ok(result.detailRows > 0)
  assert.equal(result.pathname, requestedUrl.pathname)
  assert.ok(result.rows > 0)
} else {
  await waitFor(`document.querySelector('[data-testid="process-history"] .uplot-host canvas') !== null`, "process history")
  result = await evaluate(`(() => ({
    backingRatio: (() => {
      const canvas = document.querySelector('[data-testid="process-history"] .uplot-host canvas')
      return canvas.width / canvas.getBoundingClientRect().width
    })(),
    canvasAriaHidden: document.querySelector('[data-testid="process-history"] .uplot-host canvas').getAttribute("aria-hidden"),
    canvases: document.querySelectorAll('[data-testid="process-history"] .uplot-host canvas').length,
    chartHeight: document.querySelector('[data-testid="process-history"] .uplot-figure').getBoundingClientRect().height,
    hostLabel: document.querySelector('[data-testid="process-history"] .uplot-host').getAttribute("aria-label"),
    hostRole: document.querySelector('[data-testid="process-history"] .uplot-host').getAttribute("role"),
    navigators: document.querySelectorAll('[data-testid="process-history"] input.chart-navigator[type="range"]').length,
    rows: document.querySelectorAll(".process-table .entity-row").length,
    summaries: document.querySelectorAll('[data-testid="process-history"] .chart-summary').length,
  }))()`)
  assert.equal(result.chartHeight, 200)
  assert.equal(result.canvasAriaHidden, "true")
  assert.equal(result.canvases, 1)
  assert.equal(result.hostRole, "img")
  assert.ok(result.hostLabel.length > 0)
  assert.equal(result.navigators, 1)
  assert.ok(result.rows > 0)
  assert.equal(result.summaries, 1)
  assert.ok(result.backingRatio >= 1)
}
assert.deepEqual(errors, [])
assert.deepEqual(external, [])
socket.close()
process.stdout.write(`${JSON.stringify({ ...result, externalRequests: external.length, pageErrors: errors.length })}\n`)
