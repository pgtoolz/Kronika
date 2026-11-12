/**
 * Visible-area geometry shared by every portalled surface.
 *
 * Safari's layout viewport (`window.innerHeight`, `100vh`) does not shrink when
 * the on-screen keyboard opens, and `100dvh` only tracks the collapsing browser
 * bars — it says nothing about a scrolled or zoomed visual viewport.
 * `window.visualViewport` is the only source for the rectangle the user can
 * actually see and touch. Anchors from `getBoundingClientRect()` and the
 * `position: fixed` surfaces we place are both expressed in layout-viewport
 * coordinates, so the visible rectangle is returned as an offset plus a size in
 * that same coordinate space.
 *
 * With `viewport-fit=cover` the layout viewport also includes the notch and the
 * Home Indicator. The four `--kronika-safe-*` custom properties carry the
 * `env()` insets; `env()` is substituted inside custom properties at
 * computed-value time in Chrome and WebKit, so both CSS and script read the
 * same numbers. The helpers are pure where possible so they can be tested
 * without a DOM.
 *
 * `--kronika-visible-*` is the safe-area-intersected visible rectangle. It is
 * for `position: fixed` overlays only: applied as normal-flow padding to the
 * app shell it would reflow the dashboard on every scroll/zoom frame.
 */

export interface VisibleArea {
  readonly height: number
  readonly left: number
  readonly top: number
  readonly width: number
}

export interface SafeInsets {
  readonly bottom: number
  readonly left: number
  readonly right: number
  readonly top: number
}

export interface VisualViewportMetrics {
  readonly height: number
  readonly offsetLeft: number
  readonly offsetTop: number
  readonly width: number
}

export interface LayoutViewport {
  readonly height: number
  readonly width: number
}

export const NO_INSETS: SafeInsets = { bottom: 0, left: 0, right: 0, top: 0 }

const VISIBLE_PROPERTIES = {
  bottom: "--kronika-visible-bottom",
  height: "--kronika-visible-height",
  left: "--kronika-visible-left",
  right: "--kronika-visible-right",
  top: "--kronika-visible-top",
  width: "--kronika-visible-width",
} as const

const SAFE_PROPERTIES = {
  bottom: "--kronika-safe-bottom",
  left: "--kronika-safe-left",
  right: "--kronika-safe-right",
  top: "--kronika-safe-top",
} as const

/** The visual viewport in layout-viewport coordinates, or the layout viewport. */
export function visibleArea(
  layout: { readonly height: number; readonly width: number },
  visual: VisualViewportMetrics | null = null,
): VisibleArea {
  return {
    height: Math.max(0, visual === null ? layout.height : visual.height),
    left: visual?.offsetLeft ?? 0,
    top: visual?.offsetTop ?? 0,
    width: Math.max(0, visual === null ? layout.width : visual.width),
  }
}

/**
 * Intersect the visible rectangle with the safe rectangle. Intersecting (rather
 * than blindly shrinking each side) matters when the visual viewport is already
 * scrolled below the notch: the top inset then no longer applies to its top
 * edge, while the bottom inset still applies to the bottom one.
 */
export function placementBox(
  layout: { readonly height: number; readonly width: number },
  visual: VisualViewportMetrics | null = null,
  insets: SafeInsets = NO_INSETS,
): VisibleArea {
  const seen = visibleArea(layout, visual)
  const left = Math.max(seen.left, insets.left)
  const top = Math.max(seen.top, insets.top)
  const right = Math.min(seen.left + seen.width, layout.width - insets.right)
  const bottom = Math.min(seen.top + seen.height, layout.height - insets.bottom)
  return {
    height: Math.max(0, bottom - top),
    left,
    top,
    width: Math.max(0, right - left),
  }
}

/** CSS edge offsets for the intersection box, measured from the layout edges. */
export function viewportCssProperties(
  layout: LayoutViewport,
  box: VisibleArea,
): Readonly<Record<string, string>> {
  return {
    [VISIBLE_PROPERTIES.top]: `${box.top}px`,
    [VISIBLE_PROPERTIES.right]: `${Math.max(0, layout.width - box.left - box.width)}px`,
    [VISIBLE_PROPERTIES.bottom]: `${Math.max(0, layout.height - box.top - box.height)}px`,
    [VISIBLE_PROPERTIES.left]: `${box.left}px`,
    [VISIBLE_PROPERTIES.height]: `${box.height}px`,
    [VISIBLE_PROPERTIES.width]: `${box.width}px`,
  }
}

/** Read the four safe-area insets from the computed root custom properties. */
export function readSafeInsets(root?: Element | null): SafeInsets {
  if (typeof document === "undefined" || typeof getComputedStyle !== "function") return NO_INSETS
  const element = root === undefined ? document.documentElement : root
  if (element === null) return NO_INSETS
  const style = getComputedStyle(element)
  const pixels = (name: string): number => {
    const value = Number.parseFloat(style.getPropertyValue(name))
    return Number.isFinite(value) ? value : 0
  }
  return {
    bottom: pixels(SAFE_PROPERTIES.bottom),
    left: pixels(SAFE_PROPERTIES.left),
    right: pixels(SAFE_PROPERTIES.right),
    top: pixels(SAFE_PROPERTIES.top),
  }
}

/** The layout viewport in the same coordinate space as `position: fixed`. */
export function layoutViewport(): LayoutViewport | null {
  if (typeof window === "undefined" || typeof document === "undefined") return null
  return {
    height: window.innerHeight,
    width: document.documentElement.clientWidth,
  }
}

/** `visualViewport` in layout-viewport coordinates, or null when unavailable. */
export function visualViewportMetrics(): VisualViewportMetrics | null {
  if (typeof window === "undefined") return null
  const viewport = window.visualViewport
  if (viewport === null || viewport === undefined) return null
  return {
    height: viewport.height,
    offsetLeft: viewport.offsetLeft,
    offsetTop: viewport.offsetTop,
    width: viewport.width,
  }
}

/** The box an open surface must stay inside, right now. */
export function currentPlacementBox(): VisibleArea {
  const layout = layoutViewport()
  if (layout === null) return { height: 0, left: 0, top: 0, width: 0 }
  return placementBox(layout, visualViewportMetrics(), readSafeInsets())
}

/**
 * Safari changes the visible area on bar collapse/expand, keyboard show/hide,
 * rotation and pinch-zoom without necessarily firing `window.resize`. The
 * `visualViewport` events cover all of those; the window events are kept for
 * engines where the visual viewport alone is not enough.
 */
export function observeViewport(update: () => void): () => void {
  if (typeof window === "undefined") return () => {}
  const viewport = window.visualViewport
  window.addEventListener("resize", update)
  window.addEventListener("orientationchange", update)
  viewport?.addEventListener("resize", update)
  viewport?.addEventListener("scroll", update)
  return () => {
    window.removeEventListener("resize", update)
    window.removeEventListener("orientationchange", update)
    viewport?.removeEventListener("resize", update)
    viewport?.removeEventListener("scroll", update)
  }
}

/**
 * Publish the same intersection box to CSS. Fixed surfaces cannot read
 * `visualViewport` from a stylesheet, and `dvh` does not follow the keyboard or
 * pinch-zoom offsets. The root properties are edges from the layout viewport;
 * CSS therefore applies them exactly once, instead of adding safe-area padding
 * again inside every nested surface.
 */
export function installViewportCss(): () => void {
  if (typeof window === "undefined" || typeof document === "undefined") return () => {}
  const root = document.documentElement
  let applied: Readonly<Record<string, string>> = {}
  const update = () => {
    const layout = layoutViewport()
    if (layout === null) return
    const properties = viewportCssProperties(layout, currentPlacementBox())
    // visualViewport `scroll` fires on every pan/zoom frame. Rewriting unchanged
    // custom properties would invalidate styles for the whole document each time
    // and can feed back into the layout, so only the deltas are written.
    for (const [name, value] of Object.entries(properties)) {
      if (applied[name] === value) continue
      root.style.setProperty(name, value)
    }
    applied = properties
  }
  update()
  return observeViewport(update)
}
