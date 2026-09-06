import assert from "node:assert/strict"

const [debugPort, pageUrl] = process.argv.slice(2)
if (debugPort === undefined || pageUrl === undefined) {
  throw new Error("usage: node tests/browser-corrections.mjs DEBUG_PORT PAGE_URL")
}
const expectedOrigin = new URL(pageUrl).origin
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
  if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.text)
  if (message.method === "Runtime.consoleAPICalled" && ["error", "assert"].includes(message.params.type)) {
    errors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? "").join(" "))
  }
  if (message.method === "Log.entryAdded" && message.params.entry.level === "error") errors.push(message.params.entry.text)
  if (message.method === "Network.loadingFailed") errors.push(message.params.errorText)
  if (message.method === "Network.responseReceived" && message.params.response.status >= 400) {
    errors.push(`${message.params.response.status}:${message.params.response.url}`)
  }
  if (message.method === "Network.requestWillBeSent") {
    const url = message.params.request.url
    if (!url.startsWith("data:") && !url.startsWith("blob:") && new URL(url).origin !== expectedOrigin) external.push(url)
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
  throw new Error(`timed out waiting for ${description}`)
}

async function viewport(width, height) {
  await send("Emulation.setDeviceMetricsOverride", { deviceScaleFactor: 1, height, mobile: false, width })
  await new Promise((resolve) => setTimeout(resolve, 100))
}

await Promise.all([send("Page.enable"), send("Runtime.enable"), send("Network.enable"), send("Log.enable")])
await send("Page.bringToFront")
await viewport(1366, 768)
await send("Page.navigate", { url: pageUrl })
await waitFor(`document.readyState === "complete" && document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "the application")
await evaluate(`document.fonts.ready.then(() => true)`)

assert.equal(await evaluate(`document.querySelector('[data-testid="hour-picker"]').querySelectorAll('input, select').length`), 0)
const initialHourLabel = await evaluate(`document.querySelector('[data-testid="hour-picker-trigger"] strong').textContent.trim()`)
await evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
await waitFor(`document.querySelectorAll('[data-testid="hour-cell"]').length === 2`, "the exact catalogued hours")
const nextInstant = await evaluate(`[...document.querySelectorAll('[data-testid="hour-cell"]')].find((cell) => cell.getAttribute('aria-pressed') !== 'true').dataset.instant`)
await evaluate(`document.querySelector('[data-testid="hour-cell"][aria-pressed="true"]').dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'ArrowRight' }))`)
assert.equal(await evaluate(`document.activeElement?.dataset.instant`), nextInstant)
await evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))`)
await waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "Escape close")
assert.equal(await evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)

await evaluate(`document.querySelector('[data-testid="locale-ru"]').click(); document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
await waitFor(`document.querySelector('[data-testid="timezone-value"]')?.textContent.length > 0`, "time zone label")
await evaluate(`document.querySelector('[data-testid="hour-cell"][data-instant="${nextInstant}"]').click()`)
await waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong').textContent.trim() !== ${JSON.stringify(initialHourLabel)}`, "cell selection")
const nextHourLabel = await evaluate(`document.querySelector('[data-testid="hour-picker-trigger"] strong').textContent.trim()`)
assert.equal(await evaluate(`document.querySelector('[data-testid="hour-popover"]') === null`), true)
assert.equal(await evaluate(`document.activeElement === document.querySelector('[data-testid="hour-picker-trigger"]')`), true)
await evaluate(`document.querySelector('[data-testid="hour-previous"]').click()`)
await waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong').textContent.trim() === ${JSON.stringify(initialHourLabel)}`, "previous navigation")
await evaluate(`document.querySelector('[data-testid="hour-next"]').click()`)
await waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong').textContent.trim() === ${JSON.stringify(nextHourLabel)}`, "next navigation")
await evaluate(`document.querySelector('[data-testid="hour-previous"]').click(); document.querySelector('[data-testid="locale-en"]').click()`)
await waitFor(`document.querySelector('[data-testid="hour-picker-trigger"] strong').textContent.trim() === ${JSON.stringify(initialHourLabel)}`, "restored hour")

await evaluate(`document.querySelector('[data-testid="hour-picker-trigger"]').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' })); document.querySelector('[data-testid="hour-picker-trigger"]').click()`)
await waitFor(`document.querySelector('[data-testid="hour-popover"]') !== null`, "touch open")
await evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)
await waitFor(`document.querySelector('[data-testid="hour-popover"]') === null`, "touch outside close")

const help = `.lane-label .help-dot`
await evaluate(`document.querySelector('${help}').dispatchEvent(new MouseEvent('mouseover', { bubbles: true }))`)
await waitFor(`document.querySelector('[role="tooltip"]') !== null`, "hover tooltip")
let overlay = await evaluate(`(() => {
  const node = document.querySelector('[role="tooltip"]')
  const rect = node.getBoundingClientRect()
  return { bottom: rect.bottom, left: rect.left, parent: node.parentElement === document.body, position: getComputedStyle(node).position, right: rect.right, top: rect.top, z: Number(getComputedStyle(node).zIndex) }
})()`)
assert.equal(overlay.parent, true)
assert.equal(overlay.position, "fixed")
assert.ok(overlay.left >= 0 && overlay.right <= 1366 && overlay.top >= 0 && overlay.bottom <= 768)
assert.ok(overlay.z > 100)
await evaluate(`document.querySelector('${help}').dispatchEvent(new MouseEvent('mouseout', { bubbles: true })); document.querySelector('[role="tooltip"]').dispatchEvent(new MouseEvent('mouseover', { bubbles: true }))`)
await new Promise((resolve) => setTimeout(resolve, 120))
assert.equal(await evaluate(`document.querySelector('[role="tooltip"]') !== null`), true)
await evaluate(`document.querySelector('[role="tooltip"]').dispatchEvent(new MouseEvent('mouseout', { bubbles: true }))`)
await waitFor(`document.querySelector('[role="tooltip"]') === null`, "hover leave close")

await evaluate(`document.activeElement?.blur(); document.querySelector('${help}').focus()`)
await waitFor(`document.querySelector('[role="tooltip"]') !== null`, "focus tooltip")
await evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' }))`)
await waitFor(`document.querySelector('[role="tooltip"]') === null`, "tooltip Escape close")
await evaluate(`document.querySelector('${help}').click()`)
await waitFor(`document.querySelector('[role="tooltip"]') !== null`, "click tooltip")
await evaluate(`document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))`)
await waitFor(`document.querySelector('[role="tooltip"]') === null`, "tooltip outside close")

await viewport(320, 360)
await evaluate(`(() => {
  const wrapper = document.querySelector('${help}').closest('.label-help')
  Object.assign(wrapper.style, { bottom: '2px', position: 'fixed', right: '2px', zIndex: '200' })
  document.querySelector('${help}').dispatchEvent(new MouseEvent('mouseover', { bubbles: true }))
})()`)
await waitFor(`document.querySelector('[role="tooltip"]')?.dataset.placement === 'above'`, "tooltip collision flip")
overlay = await evaluate(`(() => { const rect = document.querySelector('[role="tooltip"]').getBoundingClientRect(); return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top } })()`)
assert.ok(overlay.left >= 0 && overlay.right <= 320 && overlay.top >= 0 && overlay.bottom <= 360)
const narrowChart = await evaluate(`(() => {
  const figure = document.querySelector('[data-testid="hour-timeline"]')
  const host = figure.querySelector('.uplot-host')
  const plot = figure.querySelector('.u-over')
  const canvas = figure.querySelector('canvas')
  const bounds = (node) => { const rect = node.getBoundingClientRect(); return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top, width: rect.width } }
  return {
    canvas: bounds(canvas),
    canvasAriaHidden: canvas.getAttribute('aria-hidden'),
    host: bounds(host),
    hostLabel: host.getAttribute('aria-label'),
    navigator: figure.querySelector('input.chart-navigator[type="range"]') !== null,
    plot: bounds(plot),
    scrollWidth: document.documentElement.scrollWidth,
  }
})()`)
assert.equal(narrowChart.canvasAriaHidden, "true")
assert.equal(narrowChart.navigator, true)
assert.ok(narrowChart.hostLabel.length > 0)
assert.ok(narrowChart.plot.width > 100)
assert.ok(narrowChart.canvas.left >= narrowChart.host.left - 1 && narrowChart.canvas.right <= narrowChart.host.right + 1)
assert.ok(narrowChart.scrollWidth <= 320)

await viewport(1366, 768)
await send("Page.reload", { ignoreCache: true })
await waitFor(`document.querySelector('[data-testid="hour-picker-trigger"]') !== null`, "the reloaded application")
await evaluate(`document.fonts.ready.then(() => true); document.querySelector('[data-testid="hour-picker-trigger"]').click(); document.querySelector('${help}').dispatchEvent(new MouseEvent('mouseover', { bubbles: true }))`)
await waitFor(`document.querySelector('[role="tooltip"]') !== null && document.querySelector('[data-testid="hour-popover"]') !== null`, "the combined checkpoint")
const chart = await evaluate(`(() => {
  const figure = document.querySelector('[data-testid="hour-timeline"]')
  const canvas = figure.querySelector('canvas')
  const host = figure.querySelector('.uplot-host')
  const navigator = figure.querySelector('input.chart-navigator[type="range"]')
  return {
    canvasAriaHidden: canvas.getAttribute('aria-hidden'),
    canvasCount: figure.querySelectorAll('.uplot-host canvas').length,
    hostLabel: host.getAttribute('aria-label'),
    hostRole: host.getAttribute('role'),
    navigatorLabel: navigator.getAttribute('aria-label'),
    navigatorMaximum: Number(navigator.max),
    summary: figure.querySelector('.chart-summary').textContent,
  }
})()`)
assert.equal(chart.canvasAriaHidden, "true")
assert.equal(chart.canvasCount, 1)
assert.equal(chart.hostRole, "img")
assert.ok(chart.hostLabel.length > 0)
assert.ok(chart.navigatorLabel.length > 0)
assert.ok(chart.navigatorMaximum >= 0)
assert.ok(chart.summary.length > 0)

assert.deepEqual(errors, [])
assert.deepEqual(external, [])
socket.close()
process.stdout.write(`${JSON.stringify({ chart, externalRequests: external.length, pageErrors: errors.length }, null, 2)}\n`)
