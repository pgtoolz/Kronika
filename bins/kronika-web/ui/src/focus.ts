export const FOCUSABLE = 'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])'

const ACTIVE_PORTAL = [
  "[data-kronika-active-surface]",
  '[role="dialog"]',
  '[aria-modal="true"]',
  '[data-testid="search-help"]',
  '[data-testid="activity-overlay"]',
  '[data-testid="hour-popover"]',
  '[data-testid="manual-copy-fallback"]',
  "dialog[open]",
].join(",")

export interface TabKeyEvent {
  readonly key: string
  readonly shiftKey: boolean
  readonly defaultPrevented: boolean
  readonly target: EventTarget | null
  preventDefault(): void
}

function asElement(value: unknown): Element | null {
  return typeof Element !== "undefined" && value instanceof Element ? value : null
}

function asHTMLElement(value: unknown): HTMLElement | null {
  return typeof HTMLElement !== "undefined" && value instanceof HTMLElement ? value : null
}

function bodyRoots(): readonly Element[] {
  return typeof document === "undefined" ? [] : [document.body]
}

/** Focusable controls in the supplied visual surfaces, in their DOM order. */
export function visibleFocusable(roots: readonly (Element | null | undefined)[]): HTMLElement[] {
  const seen = new Set<HTMLElement>()
  const result: HTMLElement[] = []
  for (const root of roots) {
    if (root === null || root === undefined) continue
    const self = asHTMLElement(root)
    const candidates = self !== null && root.matches(FOCUSABLE)
      ? [self, ...root.querySelectorAll<HTMLElement>(FOCUSABLE)]
      : [...root.querySelectorAll<HTMLElement>(FOCUSABLE)]
    for (const node of candidates) {
      if (seen.has(node) || node.hasAttribute("disabled")) continue
      try {
        if (node.matches(":disabled")) continue
      } catch {
        // A synthetic node without full selector support is still checked below.
      }
      if (node.closest('[inert], [aria-hidden="true"]') !== null) continue
      if (node.offsetParent === null && node.getClientRects().length === 0) continue
      try {
        const style = getComputedStyle(node)
        if (style.display === "none" || style.visibility === "hidden") continue
      } catch {
        // Keep the node if computed style is unavailable.
      }
      seen.add(node)
      result.push(node)
    }
  }
  return result
}

export function containsNode(roots: readonly (Element | null | undefined)[], node: Node | null): boolean {
  if (node === null) return false
  return roots.some((root) => root !== null && root !== undefined && root.contains(node))
}

/** Body-level overlays that may still own focus while another modal is open. */
export function activePortalRoots(exclude: readonly (Element | null | undefined)[] = []): HTMLElement[] {
  if (typeof document === "undefined") return []
  return [...document.body.children].flatMap((child) => {
    const element = asHTMLElement(child)
    if (element === null || element.id === "root") return []
    if (["SCRIPT", "STYLE", "TEMPLATE"].includes(element.tagName)) return []
    if (containsNode(exclude, element)) return []
    // The modal's own portal wrapper is not an independent active surface:
    // its dialog is one of the excluded surfaces.
    if (exclude.some((surface) => surface !== null && surface !== undefined && element.contains(surface))) return []
    return element.matches(ACTIVE_PORTAL) || element.querySelector(ACTIVE_PORTAL) !== null ? [element] : []
  })
}

export function isActivePortalNode(
  node: Node | null,
  exclude: readonly (Element | null | undefined)[] = [],
): boolean {
  const element = asElement(node)
  if (element === null) return false
  // An inert background dialog is not an active surface.
  if (element.closest("[inert]") !== null) return false
  const target = element instanceof Node ? element : null
  if (target === null) return false
  return containsNode(activePortalRoots(exclude), target)
}

interface IsolatedElement {
  readonly element: HTMLElement
  readonly inert: string | null
  readonly hidden: string | null
}

/**
 * Hide the background from assistive technology while a modal is open.
 * Ancestors of the surface stay interactive, so portals nested in the modal
 * are not disabled; body-level active portals are left alone. `inert` is
 * opt-in: it also blocks pointer events, which would break programmatic
 * background controls used by the app (and its tests) while a dialog is open.
 * The returned function restores the original attributes.
 */
export function isolateBackground(
  surfaces: readonly (Element | null | undefined)[],
  options: { readonly inert?: boolean } = {},
): () => void {
  if (typeof document === "undefined") return () => {}
  const protectedNodes = new Set<Element>()
  const surfaceNodes = new Set<Element>()
  for (const surface of surfaces) {
    if (surface === null || surface === undefined) continue
    surfaceNodes.add(surface)
    let node: Element | null = surface
    while (node !== null) {
      protectedNodes.add(node)
      if (node === document.body) break
      node = node.parentElement
    }
  }
  const isolated: IsolatedElement[] = []
  for (const ancestor of protectedNodes) {
    if (surfaceNodes.has(ancestor)) continue
    for (const rawChild of [...ancestor.children]) {
      const child = asHTMLElement(rawChild)
      if (child === null || protectedNodes.has(child)) continue
      if (["SCRIPT", "STYLE", "TEMPLATE"].includes(child.tagName)) continue
      if (isActivePortalNode(child, surfaces)) continue
      isolated.push({
        element: child,
        inert: child.getAttribute("inert"),
        hidden: child.getAttribute("aria-hidden"),
      })
      if (options.inert === true) child.setAttribute("inert", "")
      child.setAttribute("aria-hidden", "true")
    }
  }
  return () => {
    for (const { element, inert, hidden } of isolated) {
      if (inert === null) element.removeAttribute("inert")
      else element.setAttribute("inert", inert)
      if (hidden === null) element.removeAttribute("aria-hidden")
      else element.setAttribute("aria-hidden", hidden)
    }
  }
}

/**
 * Keep focus inside the supplied surfaces. A `focusin` that lands on the
 * background (including WebKit's BODY fallback) is pulled back to the first
 * visible control of the modal. Active body-level portals are allowed.
 */
export function holdFocus(
  roots: readonly (Element | null | undefined)[],
  portals: () => readonly (Element | null | undefined)[] = () => [],
): () => void {
  if (typeof document === "undefined") return () => {}
  const guard = (event: FocusEvent) => {
    const node = event.target
    if (!(node instanceof Node)) return
    if (containsNode(roots, node) || containsNode(portals(), node)) return
    if (isActivePortalNode(node, roots)) return
    focusSurface(roots)
  }
  document.addEventListener("focusin", guard, true)
  return () => document.removeEventListener("focusin", guard, true)
}

export function focusNode(node: Element | null | undefined): boolean {
  const element = asHTMLElement(node)
  if (element === null || !element.isConnected || element.closest("[inert]") !== null) return false
  try {
    element.focus({ preventScroll: true })
    return document.activeElement === element
  } catch {
    return false
  }
}

export function focusSurface(
  roots: readonly (Element | null | undefined)[],
  containers: readonly (Element | null | undefined)[] = [],
): boolean {
  for (const node of visibleFocusable(roots)) if (focusNode(node)) return true
  for (const container of containers) if (focusNode(container)) return true
  return false
}

/**
 * Explicit Tab cycling is more reliable than native order across portals,
 * Safari Full Keyboard Access settings, and WebKit's button-focus behavior.
 * Returns true when the event was consumed.
 */
export function cycleTabFocus(
  event: TabKeyEvent,
  roots: readonly (Element | null | undefined)[],
): boolean {
  if (event.key !== "Tab" || event.defaultPrevented) return false
  const focusable = visibleFocusable(roots)
  if (focusable.length === 0) return false
  const active = typeof document === "undefined" ? null : asHTMLElement(document.activeElement)
  const fromEvent = asHTMLElement(event.target)
  const current = active !== null && focusable.includes(active)
    ? active
    : fromEvent !== null && focusable.includes(fromEvent) ? fromEvent : null
  const index = current === null ? -1 : focusable.indexOf(current)
  const next = event.shiftKey
    ? index <= 0 ? focusable.length - 1 : index - 1
    : index < 0 || index === focusable.length - 1 ? 0 : index + 1
  event.preventDefault()
  focusNode(focusable[next])
  return true
}

let trackedUsers = 0
let trackedRelease: (() => void) | null = null
let lastInteracted: HTMLElement | null = null

/** Remember the last real control the user touched, even when WebKit leaves activeElement on BODY. */
export function trackOpeners(): () => void {
  if (typeof document === "undefined") return () => {}
  if (trackedRelease === null) {
    const remember = (event: Event) => {
      const target = asElement(event.target)
      if (target === null) return
      const candidate = target.closest<HTMLElement>(FOCUSABLE)
      if (candidate === null || candidate === document.body || candidate.closest("[inert]") !== null) return
      lastInteracted = candidate
    }
    document.addEventListener("pointerdown", remember, true)
    document.addEventListener("keydown", remember, true)
    trackedRelease = () => {
      document.removeEventListener("pointerdown", remember, true)
      document.removeEventListener("keydown", remember, true)
      lastInteracted = null
    }
  }
  trackedUsers += 1
  return () => {
    trackedUsers -= 1
    if (trackedUsers === 0 && trackedRelease !== null) {
      const release = trackedRelease
      trackedRelease = null
      release()
    }
  }
}

function lastInteractedElement(): HTMLElement | null {
  return lastInteracted !== null && lastInteracted.isConnected ? lastInteracted : null
}

/** Prefer an explicit initiator, then the last real interaction, then activeElement. */
export function rememberOpener(
  explicit: Element | null | undefined,
  excludedRoots: readonly (Element | null | undefined)[] = [],
): HTMLElement | null {
  if (typeof document === "undefined") return null
  const candidates: readonly (Element | null)[] = [
    explicit ?? null,
    lastInteractedElement(),
    document.activeElement,
    ...visibleFocusable(bodyRoots()),
  ]
  for (const candidate of candidates) {
    const element = asHTMLElement(candidate)
    if (element === null || element === document.body || !element.isConnected) continue
    if (containsNode(excludedRoots, element) || element.closest("[inert]") !== null) continue
    return element
  }
  return null
}

/**
 * Restore focus after isolation has been released. The explicit initiator gets
 * a synchronous attempt plus a microtask retry before any body-level fallback
 * is used, because WebKit can ignore a focus() call made outside a click.
 */
export function restoreFocus(
  candidates: readonly (Element | null | undefined)[],
  fallbackRoots: readonly (Element | null | undefined)[] = bodyRoots(),
  excludedRoots: readonly (Element | null | undefined)[] = [],
): void {
  const explicit = candidates.filter((candidate) => candidate !== null && candidate !== undefined)
  const attempt = (list: readonly (Element | null | undefined)[]): boolean => {
    for (const candidate of list) {
      const element = asHTMLElement(candidate)
      if (element === null || containsNode(excludedRoots, element)) continue
      if (focusNode(element)) return true
    }
    return false
  }
  if (attempt(explicit)) return
  const retry = () => {
    if (attempt(explicit)) return
    const fallback = visibleFocusable(fallbackRoots).filter((node) => !containsNode(excludedRoots, node))
    attempt([...explicit, ...fallback])
  }
  if (typeof queueMicrotask === "function") queueMicrotask(retry)
  else retry()
}
