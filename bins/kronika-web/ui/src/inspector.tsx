import { Maximize2, Minimize2 } from "lucide-react"
import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type MutableRefObject, type PointerEvent as ReactPointerEvent, type ReactNode } from "react"
import { createPortal } from "react-dom"

import { INSPECTOR_PANELS, type InspectorPanel } from "./address"
import { activePortalRoots, cycleTabFocus, focusNode, holdFocus, isolateBackground, rememberOpener, restoreFocus } from "./focus"
import type { Translate } from "./help"
import { persistentStorage } from "./storage"

const MIN_WIDTH = 320
const MAX_WIDTH = 520
const DEFAULT_WIDTH = 360
const WIDTH_KEY = "kronika.inspector-width"
const MOBILE_MODAL = "(max-width: 1000px)"

/** The desktop Inspector is a peer panel; only the compact layout is modal. */
function useModalMode(): boolean {
  const [modal, setModal] = useState(() => window.matchMedia(MOBILE_MODAL).matches)
  useEffect(() => {
    const update = () => setModal(window.matchMedia(MOBILE_MODAL).matches)
    update()
    window.addEventListener("resize", update)
    return () => window.removeEventListener("resize", update)
  }, [])
  return modal
}

/// A panel beyond Detail and Chart, named after the recorded section it shows.
export type RelationPanel = Exclude<InspectorPanel, "chart" | "detail" | null>

interface RelationTab {
  readonly id: RelationPanel
  readonly label: string
}

interface PortalController {
  readonly target: HTMLElement | null
  readonly chartTarget: HTMLElement | null
  readonly register: (identity: string, dismiss: () => void, title: string, autoOpen: boolean) => () => void
  readonly registerChart: (identity: string) => () => void
  readonly registerRelated: (identity: string, id: RelationPanel, label: string) => () => void
  readonly attachRelated: (id: RelationPanel, node: HTMLElement) => () => void
  readonly relatedTabs: readonly RelationTab[]
  readonly relatedTargets: ReadonlyMap<string, HTMLElement>
}

const InspectorPortalContext = createContext<PortalController | null>(null)

export function InspectorPortalProvider({ chartTarget, children, dismissRef, onChartAvailable, onOpen, onTitle, target }: {
  readonly chartTarget: HTMLElement | null
  readonly children: ReactNode
  readonly dismissRef: MutableRefObject<(() => void) | null>
  readonly onChartAvailable: (available: boolean) => void
  readonly onOpen: () => void
  readonly onTitle: (title: string | null) => void
  readonly target: HTMLElement | null
}) {
  const active = useRef<string | null>(null)
  const activeChart = useRef<string | null>(null)
  const callbacks = useRef({ dismissRef, onChartAvailable, onOpen, onTitle })
  callbacks.current = { dismissRef, onChartAvailable, onOpen, onTitle }
  const register = useMemo(() => (identity: string, dismiss: () => void, title: string, autoOpen: boolean) => {
    active.current = identity
    callbacks.current.dismissRef.current = dismiss
    callbacks.current.onTitle(title)
    if (autoOpen) callbacks.current.onOpen()
    return () => {
      if (active.current !== identity) return
      active.current = null
      callbacks.current.dismissRef.current = null
      callbacks.current.onTitle(null)
    }
  }, [])
  const registerChart = useMemo(() => (identity: string) => {
    activeChart.current = identity
    callbacks.current.onChartAvailable(true)
    return () => {
      if (activeChart.current !== identity) return
      activeChart.current = null
      callbacks.current.onChartAvailable(false)
    }
  }, [])
  // Relation tabs are owned by the view that has the rows, not by the shell.
  // A tab is kept in the closed panel order so the strip does not reorder as
  // owners mount, and an owner that remounts replaces its own tab in place.
  const [relatedTabs, setRelatedTabs] = useState<readonly RelationTab[]>([])
  const [relatedTargets, setRelatedTargets] = useState<ReadonlyMap<string, HTMLElement>>(new Map())
  const relatedOwners = useRef(new Map<string, string>())
  const registerRelated = useMemo(() => (identity: string, id: RelationPanel, label: string) => {
    relatedOwners.current.set(id, identity)
    setRelatedTabs((current) => panelOrder([...current.filter((tab) => tab.id !== id), { id, label }]))
    return () => {
      if (relatedOwners.current.get(id) !== identity) return
      relatedOwners.current.delete(id)
      setRelatedTabs((current) => current.filter((tab) => tab.id !== id))
    }
  }, [])
  const attachRelated = useMemo(() => (id: RelationPanel, node: HTMLElement) => {
    setRelatedTargets((current) => new Map(current).set(id, node))
    return () => setRelatedTargets((current) => {
      const next = new Map(current)
      next.delete(id)
      return next
    })
  }, [])
  return <InspectorPortalContext.Provider value={{ attachRelated, chartTarget, register, registerChart, registerRelated, relatedTabs, relatedTargets, target }}>{children}</InspectorPortalContext.Provider>
}

function panelOrder(tabs: readonly RelationTab[]): readonly RelationTab[] {
  return [...tabs].sort((left, right) => INSPECTOR_PANELS.indexOf(left.id) - INSPECTOR_PANELS.indexOf(right.id))
}

export function InspectorPortal({ autoOpen = true, children, identity, onClose, title }: {
  readonly autoOpen?: boolean | undefined
  readonly children: ReactNode
  readonly identity: string
  readonly onClose: () => void
  readonly title: string
}) {
  const controller = useContext(InspectorPortalContext)
  const dismiss = useRef(onClose)
  dismiss.current = onClose
  useLayoutEffect(() => controller?.register(identity, () => dismiss.current(), title, autoOpen), [autoOpen, controller?.register, identity, title])
  return controller === null || controller.target === null ? null : createPortal(children, controller.target)
}

// A detail's metric-history section lives on the Inspector's Chart tab, not in
// the fact list. Registration is unconditional so the Inspector knows an
// entity chart exists before the tab is opened; the children render only once
// the tab provides its slot.
export function InspectorChartPortal({ children, identity }: {
  readonly children: ReactNode
  readonly identity: string
}) {
  const controller = useContext(InspectorPortalContext)
  useLayoutEffect(() => controller?.registerChart(identity), [controller?.registerChart, identity])
  return controller === null || controller.chartTarget === null ? null : createPortal(children, controller.chartTarget)
}

// A relation panel: rows of another recorded section carrying the selection's
// identity. Registration is unconditional so the strip knows the tab exists
// before it is opened; the children render once the open tab provides a slot.
export function InspectorRelatedPortal({ children, id, identity, label }: {
  readonly children: ReactNode
  readonly id: RelationPanel
  readonly identity: string
  readonly label: string
}) {
  const controller = useContext(InspectorPortalContext)
  useLayoutEffect(
    () => controller?.registerRelated(identity, id, label),
    [controller?.registerRelated, id, identity, label],
  )
  const target = controller?.relatedTargets.get(id) ?? null
  return target === null ? null : createPortal(children, target)
}

function RelatedSlot({ id }: { readonly id: RelationPanel }) {
  const controller = useContext(InspectorPortalContext)
  const [node, setNode] = useState<HTMLElement | null>(null)
  useLayoutEffect(
    () => node === null ? undefined : controller?.attachRelated(id, node),
    [controller?.attachRelated, id, node],
  )
  return <div className="inspector-related-slot" ref={setNode} />
}

export function loadInspectorWidth(storage: Pick<Storage, "getItem">): number {
  try {
    const stored = Number.parseInt(storage.getItem(WIDTH_KEY) ?? "", 10)
    return Number.isFinite(stored) ? clampWidth(stored) : DEFAULT_WIDTH
  } catch {
    return DEFAULT_WIDTH
  }
}

export function clampWidth(width: number): number {
  return Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, Math.round(width)))
}

export function Inspector({
  chart,
  detail,
  detailAvailable,
  entityChart,
  entityChartAvailable,
  onClose,
  onPanel,
  panel,
  title,
  t,
}: {
  readonly chart: ReactNode
  readonly detail: ReactNode
  readonly detailAvailable: boolean
  readonly entityChart: ReactNode
  readonly entityChartAvailable: boolean
  readonly onClose: () => void
  readonly onPanel: (panel: Exclude<InspectorPanel, null>) => void
  readonly panel: Exclude<InspectorPanel, null>
  readonly title: string
  readonly t: Translate
}) {
  const related = useContext(InspectorPortalContext)?.relatedTabs ?? []
  const [width, setWidth] = useState(() => loadInspectorWidth(persistentStorage))
  const [maximized, setMaximized] = useState(false)
  const modal = useModalMode()
  const root = useRef<HTMLElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const resizeCleanup = useRef<(() => void) | null>(null)
  const style = { "--inspector-width": `${width}px` } as CSSProperties

  // A resize can outlive the gesture when the panel closes or WebKit cancels
  // the pointer. The handler owns its listeners and releases them on unmount.
  useEffect(() => () => { resizeCleanup.current?.() }, [])

  // Only the compact, scrim-covered layout is modal. aria-hidden (without
  // inert) keeps VoiceOver out of the background while leaving the app's
  // programmatic background controls and portals working. This effect is
  // declared first so its cleanup releases isolation before focus is restored.
  useLayoutEffect(() => {
    const surface = root.current
    if (!modal || surface === null) return
    const release = isolateBackground([surface])
    const stopHolding = holdFocus([surface], () => activePortalRoots([surface]))
    return () => {
      stopHolding()
      release()
    }
  }, [modal])

  // The initiator is remembered from the real interaction, not just
  // activeElement: WebKit may leave focus on BODY after a button click.
  // Every close path restores focus through the unmount cleanup.
  useLayoutEffect(() => {
    const surface = root.current
    if (surface === null) return
    opener.current = rememberOpener(undefined, [surface])
    focusNode(surface)
    return () => {
      restoreFocus([opener.current])
    }
  }, [])

  useEffect(() => {
    try { persistentStorage.setItem(WIDTH_KEY, String(width)) } catch {}
  }, [width])

  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) {
        event.preventDefault()
        if (maximized) {
          setMaximized(false)
          return
        }
        onClose()
        return
      }
      // Desktop stays non-modal. On compact layouts Tab is moved explicitly
      // across visible controls, including portals owned by this surface.
      if (event.key !== "Tab" || !modal) return
      const surface = root.current
      if (surface === null) return
      cycleTabFocus(event, [surface, ...activePortalRoots([surface])])
    }
    window.addEventListener("keydown", keydown)
    return () => window.removeEventListener("keydown", keydown)
  }, [maximized, modal, onClose])

  const beginResize = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!event.isPrimary || event.button !== 0) return
    event.preventDefault()
    resizeCleanup.current?.()
    const handle = event.currentTarget
    const pointerId = event.pointerId
    const start = event.clientX
    const initial = width
    try { handle.setPointerCapture(pointerId) } catch {}
    const move = (next: PointerEvent) => {
      if (next.pointerId !== pointerId) return
      setWidth(clampWidth(initial + start - next.clientX))
    }
    const finish = (next?: PointerEvent) => {
      if (next !== undefined && next.pointerId !== pointerId) return
      window.removeEventListener("pointermove", move)
      window.removeEventListener("pointerup", finish)
      window.removeEventListener("pointercancel", finish)
      handle.removeEventListener("lostpointercapture", finish)
      try {
        if (handle.hasPointerCapture(pointerId)) handle.releasePointerCapture(pointerId)
      } catch {}
      resizeCleanup.current = null
    }
    window.addEventListener("pointermove", move)
    window.addEventListener("pointerup", finish)
    window.addEventListener("pointercancel", finish)
    handle.addEventListener("lostpointercapture", finish)
    resizeCleanup.current = finish
  }

  return <>
    <button aria-label={t("inspector.close")} className="inspector-scrim" onClick={onClose} type="button" />
    <aside aria-label={t("inspector.title")} aria-modal={modal || undefined} className={`inspector${maximized ? " inspector-maximized" : ""}`} data-panel={panel} data-testid="inspector" ref={root} role="dialog" style={style} tabIndex={-1}>
      <button
        aria-label={t("inspector.resize")}
        aria-orientation="vertical"
        aria-valuemax={MAX_WIDTH}
        aria-valuemin={MIN_WIDTH}
        aria-valuenow={width}
        className="inspector-splitter"
        onKeyDown={(event) => {
          if (event.key !== "ArrowLeft" && event.key !== "ArrowRight" && event.key !== "Home" && event.key !== "End") return
          event.preventDefault()
          setWidth(event.key === "Home" ? MIN_WIDTH : event.key === "End" ? MAX_WIDTH : clampWidth(width + (event.key === "ArrowLeft" ? 16 : -16)))
        }}
        onPointerDown={beginResize}
        role="separator"
        type="button"
      />
      <header className="inspector-head">
        <div className="inspector-tabs" role="tablist">
          <button aria-selected={panel === "detail"} disabled={!detailAvailable} onClick={() => onPanel("detail")} role="tab" type="button">{t("inspector.detail")}</button>
          <button aria-selected={panel === "chart"} onClick={() => onPanel("chart")} role="tab" type="button">{t("inspector.chart")}</button>
          {related.map((tab) => <button aria-selected={panel === tab.id} data-testid={`inspector-tab-${tab.id}`} key={tab.id} onClick={() => onPanel(tab.id)} role="tab" type="button">{tab.label}</button>)}
        </div>
        <strong title={title}>{title}</strong>
        <div className="flex flex-none items-center gap-1">
          {panel === "chart" && <button aria-label={t(maximized ? "inspector.restore" : "inspector.maximize")} aria-pressed={maximized} className="inspector-maximize" data-testid="inspector-maximize" onClick={() => setMaximized((current) => !current)} type="button">{maximized ? <Minimize2 aria-hidden="true" size={13} /> : <Maximize2 aria-hidden="true" size={13} />}</button>}
          <button aria-label={t("common.close")} className="inspector-close" onClick={onClose} type="button">×</button>
        </div>
      </header>
      <div className="inspector-body" data-testid={`inspector-${panel}`} role="tabpanel">
        {/* The detail stays mounted while the Chart tab is open: the selected
            entity's history section portals from it into the chart slot. */}
        {detailAvailable && <div className={panel === "detail" ? "contents" : "hidden"}>{detail}</div>}
        {(panel === "chart" || (!detailAvailable && related.every((tab) => tab.id !== panel))) && (entityChartAvailable && detailAvailable ? entityChart : chart)}
        {related.map((tab) => panel === tab.id ? <RelatedSlot id={tab.id} key={tab.id} /> : null)}
      </div>
    </aside>
  </>
}
