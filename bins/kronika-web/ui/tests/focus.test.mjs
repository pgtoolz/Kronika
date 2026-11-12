import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { FOCUSABLE, cycleTabFocus, isolateBackground, restoreFocus, visibleFocusable } from "../src/focus.ts"

class FakeElement {
  constructor(tag, attributes = {}) {
    this.tagName = tag.toUpperCase()
    this.attributes = new Map(Object.entries(attributes))
    this.childNodes = []
    this.parentElement = null
    this.id = attributes.id ?? ""
    this.offsetParent = {}
    this.rects = [{}]
    this.display = "block"
    this.visibility = "visible"
    this.isConnected = true
    this.focusCalls = []
    this.ignoreFocus = false
    this.focusable = this.tagName === "BUTTON" || this.tagName === "INPUT"
    this.overlay = false
  }

  get children() { return this.childNodes }
  append(...nodes) {
    for (const node of nodes) {
      node.parentElement = this
      this.childNodes.push(node)
    }
    return this
  }
  hasAttribute(name) { return this.attributes.has(name) }
  getAttribute(name) { return this.attributes.get(name) ?? null }
  setAttribute(name, value) { this.attributes.set(name, value) }
  removeAttribute(name) { this.attributes.delete(name) }
  matches(selector) {
    if (selector === FOCUSABLE) return this.focusable && !this.attributes.has("disabled")
    if (selector === ":disabled") return this.attributes.has("disabled")
    if (selector.includes("[inert]") || selector.includes("aria-hidden")) {
      return this.ancestors().some((node) => node.attributes.has("inert") || node.attributes.get("aria-hidden") === "true")
    }
    if (selector.includes('role="dialog"') || selector.includes('aria-modal') || selector.includes("hour-popover")) return this.overlay
    return false
  }
  ancestors() {
    const result = []
    for (let node = this; node !== null; node = node.parentElement) result.push(node)
    return result
  }
  descendants() {
    return this.childNodes.flatMap((child) => [child, ...child.descendants()])
  }
  querySelectorAll(selector) {
    if (selector !== FOCUSABLE) return []
    return this.descendants().filter((node) => node.matches(selector))
  }
  querySelector(selector) {
    return this.descendants().find((node) => node.matches(selector)) ?? null
  }
  closest(selector) {
    return this.ancestors().find((node) => node.matches(selector)) ?? null
  }
  contains(node) {
    return node !== null && node !== undefined && node.ancestors().includes(this)
  }
  getClientRects() { return this.rects }
  focus(options) {
    this.focusCalls.push(options)
    if (!this.ignoreFocus && globalThis.document !== undefined) globalThis.document.activeElement = this
  }
}

function button(attributes = {}) { return new FakeElement("button", attributes) }

function installDom() {
  const body = new FakeElement("body")
  globalThis.Element = FakeElement
  globalThis.HTMLElement = FakeElement
  globalThis.Node = FakeElement
  globalThis.document = { activeElement: body, addEventListener() {}, body, removeEventListener() {} }
  globalThis.getComputedStyle = (node) => ({ display: node.display, visibility: node.visibility })
  return body
}

function keyEvent(key, target, shiftKey = false) {
  return { defaultPrevented: false, key, preventDefault() { this.defaultPrevented = true }, shiftKey, target }
}

test("explicit Tab cycling works when WebKit leaves activeElement on BODY", () => {
  const body = installDom()
  const dialog = new FakeElement("section", { role: "dialog" })
  const first = button({ "data-testid": "first" })
  const second = button({ "data-testid": "second" })
  const third = button({ "data-testid": "third" })
  dialog.append(first, second, third)
  body.append(dialog)

  // A WebKit button click does not necessarily move focus at all.
  document.activeElement = body
  const forward = keyEvent("Tab", first)
  assert.equal(cycleTabFocus(forward, [dialog]), true)
  assert.equal(forward.defaultPrevented, true)
  assert.equal(document.activeElement, second)
  assert.deepEqual(second.focusCalls, [{ preventScroll: true }])

  // Wrapping is explicit, not dependent on native Tab reaching the last node.
  document.activeElement = third
  assert.equal(cycleTabFocus(keyEvent("Tab", third), [dialog]), true)
  assert.equal(document.activeElement, first)

  document.activeElement = first
  assert.equal(cycleTabFocus(keyEvent("Tab", first, true), [dialog]), true)
  assert.equal(document.activeElement, third)

  const other = keyEvent("Escape", first)
  assert.equal(cycleTabFocus(other, [dialog]), false)
  assert.equal(other.defaultPrevented, false)
})

test("hidden, disabled, inert and aria-hidden controls never receive focus", () => {
  const body = installDom()
  const dialog = new FakeElement("section", { role: "dialog" })
  const first = button()
  const disabled = button({ disabled: "" })
  const hidden = button()
  hidden.offsetParent = null
  hidden.rects = []
  const displayNone = button()
  displayNone.display = "none"
  const behindInert = button()
  const inertWrap = new FakeElement("div", { inert: "" })
  inertWrap.append(behindInert)
  const last = button()
  dialog.append(first, disabled, hidden, displayNone, inertWrap, last)
  body.append(dialog)

  assert.deepEqual(visibleFocusable([dialog]), [first, last])
  document.activeElement = body
  const event = keyEvent("Tab", first)
  assert.equal(cycleTabFocus(event, [dialog]), true)
  assert.equal(document.activeElement, last)
})

test("background isolation hides the page but preserves active portals and never sets inert by default", () => {
  const body = installDom()
  const root = new FakeElement("div", { id: "root" })
  const scrim = new FakeElement("div", { "data-testid": "export-scrim" })
  const dialog = new FakeElement("section", { role: "dialog" })
  scrim.append(dialog)
  const popover = new FakeElement("div", { "data-testid": "hour-popover" })
  popover.overlay = true
  const inside = button()
  dialog.append(inside)
  body.append(root, scrim, popover)

  const release = isolateBackground([dialog])
  assert.equal(root.getAttribute("aria-hidden"), "true")
  assert.equal(root.hasAttribute("inert"), false, "pointer-blocking inert would break programmatic background controls")
  assert.equal(popover.getAttribute("aria-hidden"), null, "an active portal stays reachable")
  assert.equal(scrim.getAttribute("aria-hidden"), null, "the modal's own portal wrapper is not background")
  assert.equal(inside.getAttribute("aria-hidden"), null, "the modal's children stay exposed")
  release()
  assert.equal(root.getAttribute("aria-hidden"), null)
})

test("focus restoration retries the explicit initiator instead of falling through to an arbitrary control", async () => {
  const body = installDom()
  const opener = button({ "data-testid": "export-trigger" })
  const other = button({ "data-testid": "charts-toggle" })
  body.append(opener, other)
  document.activeElement = body
  opener.ignoreFocus = true

  restoreFocus([opener])
  assert.equal(document.activeElement, body, "the first attempt was ignored by the engine")
  opener.ignoreFocus = false
  await Promise.resolve()
  assert.equal(document.activeElement, opener)
  assert.equal(opener.focusCalls.length, 2)
  assert.deepEqual(opener.focusCalls[0], { preventScroll: true })
  assert.equal(other.focusCalls.length, 0, "no arbitrary fallback while the explicit opener works")
})

test("R1-02 wires explicit initiators, visible-control cycling and isolation into every modal", async () => {
  const read = (name) => readFile(new URL(`../src/${name}`, import.meta.url), "utf8")
  const [app, exporter, inspector, mcp, focus, picker] = await Promise.all([
    read("app.tsx"), read("export-dialog.tsx"), read("inspector.tsx"),
    read("mcp-connect.tsx"), read("focus.ts"), read("hour-picker.tsx"),
  ])

  // A global interaction tracker supplies the initiator when WebKit ignores clicks.
  assert.match(app, /useEffect\(\(\) => trackOpeners\(\), \[\]\)/)
  assert.match(app, /const mcpTrigger = useRef<HTMLButtonElement>\(null\)/)
  assert.match(app, /ref=\{mcpTrigger\}/)
  assert.match(app, /initiator=\{exportTrigger\}/)
  assert.match(app, /initiator=\{mcpTrigger\}/)

  // The export dialog and mobile Inspector cycle Tab over visible controls,
  // isolate the background, hold focus, and restore it on unmount.
  for (const [name, source] of [["export-dialog.tsx", exporter], ["inspector.tsx", inspector]]) {
    assert.match(source, /cycleTabFocus\(/, name)
    assert.match(source, /isolateBackground\(/, name)
    assert.match(source, /holdFocus\(/, name)
    assert.match(source, /restoreFocus\(/, name)
    assert.match(source, /activePortalRoots\(/, name)
    assert.doesNotMatch(source, /document\.activeElement === first/, name)
    assert.doesNotMatch(source, /document\.activeElement === last/, name)
  }
  assert.match(exporter, /rememberOpener\(initiator\.current/)
  assert.doesNotMatch(exporter, /const FOCUSABLE =/)
  assert.match(inspector, /rememberOpener\(undefined/)
  assert.match(inspector, /aria-modal=\{modal \|\| undefined\}/, "only the compact Inspector announces modality")
  assert.match(inspector, /if \(!modal \|\| surface === null\) return/, "desktop Inspector stays non-modal")
  assert.match(inspector, /if \(event\.key !== "Tab" \|\| !modal\) return/)
  assert.doesNotMatch(inspector, /requestAnimationFrame/)

  // MCP uses the explicit trigger, not activeElement, and retries restoration.
  assert.match(mcp, /rememberOpener\(initiator\.current/)
  assert.match(mcp, /restoreFocus\(\[target\]\)/)
  assert.match(mcp, /onKeyDownCapture=\{\(event\) => \{ cycleTabFocus/)
  assert.doesNotMatch(mcp, /const active = document\.activeElement/)
  assert.doesNotMatch(mcp, /requestAnimationFrame/)
  assert.doesNotMatch(mcp, /querySelectorAll/)

  // The shared helper is the single implementation of the focus contract.
  for (const name of ["visibleFocusable", "containsNode", "activePortalRoots", "isolateBackground", "holdFocus", "cycleTabFocus", "trackOpeners", "rememberOpener", "restoreFocus"]) {
    assert.match(focus, new RegExp(`export function ${name}\\(`), name)
  }
  assert.match(focus, /element\.focus\(\{ preventScroll: true \}\)/)
  assert.match(focus, /options\.inert === true/)
  assert.match(focus, /queueMicrotask\(retry\)/)
  assert.match(focus, /element\.id === "root"/)
  assert.match(picker, /from "\.\/focus"/)
})
