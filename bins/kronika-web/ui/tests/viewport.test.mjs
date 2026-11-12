import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import {
  NO_INSETS,
  currentPlacementBox,
  installViewportCss,
  layoutViewport,
  observeViewport,
  placementBox,
  readSafeInsets,
  viewportCssProperties,
  visibleArea,
  visualViewportMetrics,
} from "../src/viewport.ts"

const source = (name) => readFile(new URL(`../src/${name}`, import.meta.url), "utf8")

/**
 * A minimal browser shim for the viewport helpers. `visualViewport` fires
 * `scroll` continuously on iOS pan/zoom, so the tests drive those listeners
 * explicitly and record every `root.style.setProperty` call.
 */
function withBrowser(options, action) {
  const {
    height = 900,
    width = 390,
    visual = null,
    insets = { bottom: 0, left: 0, right: 0, top: 0 },
  } = options
  const safeValues = {
    "--kronika-safe-bottom": `${insets.bottom}px`,
    "--kronika-safe-left": `${insets.left}px`,
    "--kronika-safe-right": `${insets.right}px`,
    "--kronika-safe-top": `${insets.top}px`,
  }
  const listeners = { window: {}, viewport: {} }
  const writes = []
  const register = (kind) => (type, fn) => {
    ;(listeners[kind][type] ??= new Set()).add(fn)
  }
  const unregister = (kind) => (type, fn) => {
    listeners[kind][type]?.delete(fn)
  }
  const vv = visual === null ? null : {
    height: visual.height,
    width: visual.width,
    offsetLeft: visual.offsetLeft ?? 0,
    offsetTop: visual.offsetTop ?? 0,
    addEventListener: register("viewport"),
    removeEventListener: unregister("viewport"),
  }
  const fakeWindow = {
    innerHeight: height,
    innerWidth: width,
    visualViewport: vv,
    addEventListener: register("window"),
    removeEventListener: unregister("window"),
  }
  const fakeDocument = {
    documentElement: {
      clientHeight: height,
      clientWidth: width,
      style: { setProperty: (name, value) => writes.push([name, value]) },
    },
  }
  const previous = {
    window: Object.getOwnPropertyDescriptor(globalThis, "window"),
    document: Object.getOwnPropertyDescriptor(globalThis, "document"),
    getComputedStyle: Object.getOwnPropertyDescriptor(globalThis, "getComputedStyle"),
  }
  const fire = (kind, type) => {
    for (const fn of listeners[kind][type] ?? []) fn()
  }
  try {
    Object.defineProperty(globalThis, "window", { configurable: true, value: fakeWindow })
    Object.defineProperty(globalThis, "document", { configurable: true, value: fakeDocument })
    Object.defineProperty(globalThis, "getComputedStyle", {
      configurable: true,
      value: () => ({ getPropertyValue: (name) => safeValues[name] ?? "" }),
    })
    return action({ fakeWindow, listeners, writes, vv, fire })
  } finally {
    for (const [name, descriptor] of Object.entries(previous)) {
      if (descriptor === undefined) Reflect.deleteProperty(globalThis, name)
      else Object.defineProperty(globalThis, name, descriptor)
    }
  }
}

test("the visible area is the layout viewport when there is no visualViewport", () => {
  assert.deepEqual(visibleArea({ height: 900, width: 1440 }), {
    height: 900, left: 0, top: 0, width: 1440,
  })
  assert.deepEqual(
    visibleArea({ height: 900, width: 1440 }, { height: 400, offsetLeft: 12, offsetTop: 300, width: 1000 }),
    { height: 400, left: 12, top: 300, width: 1000 },
  )
})

test("a keyboard-shrunk and scrolled visual viewport is clamped to the safe edges", () => {
  // iPhone portrait: the keyboard leaves 420px visible, scrolled 240px down,
  // a 47px status bar, a 34px Home Indicator and 0 horizontal insets.
  const layout = { height: 900, width: 390 }
  const visual = { height: 420, offsetLeft: 0, offsetTop: 240, width: 390 }
  assert.deepEqual(placementBox(layout, visual, { bottom: 34, left: 0, right: 0, top: 47 }), {
    height: 420, left: 0, top: 240, width: 390,
  })
  // Landscape: a 44px notch on the left/right is subtracted once.
  assert.deepEqual(placementBox({ height: 390, width: 844 }, null, { bottom: 21, left: 44, right: 44, top: 0 }), {
    height: 369, left: 44, top: 0, width: 756,
  })
})

test("the placement box is the intersection, never a doubled inset", () => {
  const layout = { height: 800, width: 400 }
  // Visual viewport top is already below the notch, so the top inset does not
  // subtract again; the safe right edge still trims the visible rectangle.
  const box = placementBox(
    layout,
    { height: 300, offsetLeft: 0, offsetTop: 200, width: 360 },
    { bottom: 34, left: 0, right: 44, top: 47 },
  )
  assert.deepEqual(box, { height: 300, left: 0, top: 200, width: 356 })
  assert.ok(box.top >= 47 && box.top + box.height <= layout.height - 34)
})

test("an impossible box collapses instead of producing negative geometry", () => {
  const box = placementBox({ height: 320, width: 320 }, null, { bottom: 200, left: 200, right: 200, top: 200 })
  assert.deepEqual(box, { height: 0, left: 200, top: 200, width: 0 })
  assert.ok(Object.values(box).every((value) => value >= 0))
})

test("CSS properties are edge offsets from the layout viewport, not a second size", () => {
  const layout = { height: 900, width: 390 }
  const keyboard = placementBox(
    layout,
    { height: 420, offsetLeft: 0, offsetTop: 240, width: 390 },
    { bottom: 34, left: 0, right: 0, top: 47 },
  )
  assert.deepEqual(viewportCssProperties(layout, keyboard), {
    "--kronika-visible-top": "240px",
    "--kronika-visible-right": "0px",
    "--kronika-visible-bottom": "240px",
    "--kronika-visible-left": "0px",
    "--kronika-visible-height": "420px",
    "--kronika-visible-width": "390px",
  })
  // Zoomed and panned: offsets on all four edges, all non-negative.
  const zoomed = placementBox(
    layout,
    { height: 300, offsetLeft: 20, offsetTop: 200, width: 360 },
    { bottom: 34, left: 44, right: 44, top: 47 },
  )
  assert.deepEqual(viewportCssProperties(layout, zoomed), {
    "--kronika-visible-top": "200px",
    "--kronika-visible-right": "44px",
    "--kronika-visible-bottom": "400px",
    "--kronika-visible-left": "44px",
    "--kronika-visible-height": "300px",
    "--kronika-visible-width": "302px",
  })
  // An over-wide box can never publish a negative right/bottom edge.
  const clamped = viewportCssProperties(layout, { height: 100, left: 0, top: 0, width: 500 })
  assert.equal(clamped["--kronika-visible-right"], "0px")
  assert.equal(clamped["--kronika-visible-bottom"], "800px")
})

test("safe insets are read from the root custom properties, with a DOM-free default", () => {
  assert.equal(readSafeInsets(), NO_INSETS)
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document")
  const previousComputed = Object.getOwnPropertyDescriptor(globalThis, "getComputedStyle")
  try {
    const values = {
      "--kronika-safe-bottom": "34px",
      "--kronika-safe-left": "0px",
      "--kronika-safe-right": "44px",
      "--kronika-safe-top": "47px",
    }
    Object.defineProperty(globalThis, "document", { configurable: true, value: { documentElement: {} } })
    Object.defineProperty(globalThis, "getComputedStyle", {
      configurable: true,
      value: () => ({ getPropertyValue: (name) => values[name] ?? "" }),
    })
    assert.deepEqual(readSafeInsets(), { bottom: 34, left: 0, right: 44, top: 47 })
  } finally {
    if (previousDocument === undefined) Reflect.deleteProperty(globalThis, "document")
    else Object.defineProperty(globalThis, "document", previousDocument)
    if (previousComputed === undefined) Reflect.deleteProperty(globalThis, "getComputedStyle")
    else Object.defineProperty(globalThis, "getComputedStyle", previousComputed)
  }
})

test("layoutViewport and visualViewportMetrics share the fixed-position coordinate space", () => {
  assert.equal(layoutViewport(), null)
  assert.equal(visualViewportMetrics(), null)
  withBrowser(
    { height: 844, width: 390, visual: { height: 420, offsetLeft: 12, offsetTop: 240, width: 370 } },
    ({ fakeWindow }) => {
      assert.deepEqual(layoutViewport(), { height: 844, width: 390 })
      assert.deepEqual(visualViewportMetrics(), {
        height: 420, offsetLeft: 12, offsetTop: 240, width: 370,
      })
      fakeWindow.visualViewport = null
      assert.equal(visualViewportMetrics(), null)
    },
  )
})

test("installViewportCss publishes the box once and writes only changed properties", () => {
  withBrowser(
    {
      height: 900,
      width: 390,
      visual: { height: 420, offsetLeft: 0, offsetTop: 240, width: 390 },
      insets: { bottom: 34, left: 0, right: 0, top: 47 },
    },
    ({ listeners, writes, vv, fire }) => {
      const stop = installViewportCss()
      assert.deepEqual(Object.fromEntries(writes), {
        "--kronika-visible-top": "240px",
        "--kronika-visible-right": "0px",
        "--kronika-visible-bottom": "240px",
        "--kronika-visible-left": "0px",
        "--kronika-visible-height": "420px",
        "--kronika-visible-width": "390px",
      })
      assert.equal(writes.length, 6)

      // A pan/zoom frame with unchanged geometry must not invalidate styles.
      fire("viewport", "scroll")
      fire("window", "resize")
      assert.equal(writes.length, 6)

      // The keyboard grows: only top/bottom/height change.
      vv.height = 300
      vv.offsetTop = 200
      fire("viewport", "resize")
      assert.deepEqual(writes.slice(6), [
        ["--kronika-visible-top", "200px"],
        ["--kronika-visible-bottom", "400px"],
        ["--kronika-visible-height", "300px"],
      ])

      // Rotation fires both window and visualViewport events; the second is a no-op.
      fire("window", "orientationchange")
      assert.equal(writes.length, 9)

      stop()
      for (const kind of ["window", "viewport"]) {
        for (const handlers of Object.values(listeners[kind])) assert.equal(handlers.size, 0)
      }
      fire("viewport", "scroll")
      assert.equal(writes.length, 9)
    },
  )
})

test("DOM-free helpers stay harmless outside a browser", () => {
  assert.deepEqual(currentPlacementBox(), { height: 0, left: 0, top: 0, width: 0 })
  assert.doesNotThrow(() => observeViewport(() => {})())
  assert.doesNotThrow(() => installViewportCss()())
})

test("the viewport meta opts into safe-area insets without disabling zoom", async () => {
  const html = await source("index.html")
  assert.match(html, /name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"/)
  assert.doesNotMatch(html, /user-scalable\s*=\s*no|maximum-scale/)
  assert.match(html, /name="color-scheme" content="dark light"/)
})

test("the app installs the CSS visible box before React renders", async () => {
  const app = await source("app.tsx")
  assert.match(app, /import \{ installViewportCss \} from "\.\/viewport"/)
  assert.match(app, /^installViewportCss\(\)$/m)
  assert.ok(
    app.indexOf("installViewportCss()") < app.indexOf("createRoot(root)"),
    "the root properties must exist before the first surface mounts",
  )
})

test("CSS declares env() once, aliases it, and pads every fixed surface once", async () => {
  const css = await source("styles.css")
  const root = css.match(/:root \{\s*--kronika-safe-top:[\s\S]*?\}/)?.[0] ?? ""
  for (const edge of ["top", "right", "bottom", "left"]) {
    assert.match(root, new RegExp(`--kronika-safe-${edge}: env\\(safe-area-inset-${edge}, 0px\\)`))
    assert.match(root, new RegExp(`--kronika-visible-${edge}: var\\(--kronika-safe-${edge}\\)`))
  }
  // Every env() read lives on :root. The four alias reads plus the four shell
  // padding reads are the only direct safe-inset consumers; fixed overlays see
  // --kronika-visible-* instead.
  assert.equal((css.match(/env\(safe-area-inset-/g) ?? []).length, 4)
  assert.equal((root.match(/env\(safe-area-inset-/g) ?? []).length, 4)
  assert.equal((root.match(/var\(--kronika-safe-/g) ?? []).length, 4)
  const shell = css.match(/\.app-shell \{[\s\S]*?\}/)?.[0] ?? ""
  assert.equal((shell.match(/var\(--kronika-safe-/g) ?? []).length, 4)
  assert.doesNotMatch(shell, /--kronika-visible/)
  assert.ok((css.match(/var\(--kronika-visible-/g) ?? []).length > 20)
  const searchHelp = css.match(/\[data-testid="search-help"\] \{[\s\S]*?\}/)?.[0] ?? ""
  assert.equal((searchHelp.match(/var\(--kronika-visible-/g) ?? []).length, 4)

  for (const selector of [
    '[data-testid="search-help"]',
    ".activity-overlay",
    '[data-testid="help-panel"]',
    ".mcp-dialog",
    '[data-testid="manual-copy-fallback"]',
    '[data-testid="demo-notice"]',
    ".mobile-panel-head",
    ".mobile-search-open [data-search-surface]",
    ".inspector.inspector-maximized",
    ".export-scrim",
  ]) {
    assert.ok(css.includes(selector), `${selector} must have visible-area rules`)
  }
  // Nested Inspector surfaces must not repeat the insets (no double padding).
  assert.doesNotMatch(css, /\.inspector-head \{[^}]*kronika-(safe|visible)/)
  assert.doesNotMatch(css, /\.inspector-body \{[^}]*kronika-(safe|visible)/)
  // On phones the scrim yields the bottom edge to the dialog exactly once.
  assert.match(css, /@media \(max-width: 520px\) \{[\s\S]*?padding-bottom: 0;[\s\S]*?padding-bottom: var\(--kronika-visible-bottom\)/)
})

test("the shell and every dvh height keep a vh fallback first", async () => {
  const css = await source("styles.css")
  const shell = css.match(/\.app-shell \{[\s\S]*?\}/)?.[0] ?? ""
  assert.ok(shell.indexOf("height: 100vh") < shell.indexOf("height: 100dvh"), shell)
  assert.match(shell, /padding: var\(--kronika-safe-top\)/)
  // The shell must never reflow on visualViewport scroll/zoom frames.
  assert.doesNotMatch(shell, /--kronika-visible/)
  for (const rule of [
    "max-height: min(85vh, 640px); max-height: min(85dvh, 640px)",
    "height: calc(100vh - 250px);",
    "height: calc(100dvh - 250px);",
    "height: min(480px, calc(100vh - 82px));",
    "height: min(480px, calc(100dvh - 82px));",
    "max-height: calc(100vh - 20px);",
    "max-height: calc(100dvh - 20px);",
  ]) {
    assert.ok(css.includes(rule), `missing fallback progression: ${rule}`)
  }
  // The published visible height is a consumed contract, not dead output.
  assert.match(css, /--kronika-visible-height: 100vh;/)
  assert.match(css, /--kronika-visible-height: 100dvh;/)
  assert.match(css, /--kronika-visible-width: 100vw;/)
  for (const bound of [20, 16]) {
    assert.match(css, new RegExp(`max-height: calc\\(var\\(--kronika-visible-height\\) - ${bound}px\\)`))
  }
})

test("the MCP backdrop does not depend on inherited custom properties", async () => {
  const css = await source("styles.css")
  const backdrops = [...css.matchAll(/^\.mcp-dialog::backdrop.*$/gm)].map((match) => match[0])
  assert.equal(backdrops.length, 1)
  assert.doesNotMatch(backdrops[0], /var\(/)
  assert.match(css, /\[data-theme="light"\] \.mcp-dialog::backdrop \{ background: rgba\(/)
})

test("both script-placed portals use the shared visible-area box", async () => {
  const [picker, help, viewport] = await Promise.all([
    source("hour-picker.tsx"),
    source("help.tsx"),
    source("viewport.ts"),
  ])
  for (const [name, text] of [["hour-picker.tsx", picker], ["help.tsx", help]]) {
    assert.match(text, /currentPlacementBox\(\)/, `${name} must read the visible area`)
    assert.match(text, /observeViewport\(/, `${name} must react to visual viewport changes`)
    assert.doesNotMatch(text, /window\.innerHeight|window\.innerWidth/, `${name} must not use the layout viewport`)
    assert.match(text, /max-h-\[calc\(100dvh_-_/)
  }
  assert.match(picker, /window\.addEventListener\("scroll", place, true\)/)
  assert.match(help, /window\.addEventListener\("scroll", update, true\)/)
  assert.match(viewport, /const viewport = window\.visualViewport/)
  assert.match(viewport, /viewport\?\.addEventListener\("resize"/)
  assert.match(viewport, /viewport\?\.addEventListener\("scroll"/)
  assert.match(viewport, /addEventListener\("orientationchange"/)
  assert.match(viewport, /installViewportCss/)
  assert.match(viewport, /applied\[name\] === value\) continue/)
})
