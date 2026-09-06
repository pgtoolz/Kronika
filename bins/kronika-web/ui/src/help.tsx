import { useEffect, useId, useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"

export type Translate = (key: string, slots?: Readonly<Record<string, string | number>>) => string

interface TooltipPosition {
  readonly left: number
  readonly placement: "above" | "below"
  readonly top: number
}

interface Rectangle {
  readonly bottom: number
  readonly height: number
  readonly left: number
  readonly top: number
  readonly width: number
}

export function LabelHelp({
  labelKey,
  helpKey,
  helpText,
  iconOnly = false,
  t,
  testId,
}: {
  readonly labelKey: string
  readonly helpKey: string
  // A reading that changes while the page is open — how stale the hour is —
  // carries its sentence instead of a dictionary key.
  readonly helpText?: string | undefined
  readonly iconOnly?: boolean
  readonly t: Translate
  readonly testId?: string
}) {
  const [open, setOpen] = useState(false)
  const [position, setPosition] = useState<TooltipPosition | null>(null)
  const id = useId()
  const anchor = useRef<HTMLButtonElement>(null)
  const root = useRef<HTMLSpanElement>(null)
  const tooltip = useRef<HTMLSpanElement>(null)
  const closeTimer = useRef<number | null>(null)
  const openTimer = useRef<number | null>(null)
  const pinned = useRef(false)
  const cancelClose = () => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current)
      closeTimer.current = null
    }
    if (openTimer.current === null) return
    window.clearTimeout(openTimer.current)
    openTimer.current = null
  }
  const close = () => {
    cancelClose()
    pinned.current = false
    setOpen(false)
  }
  // A pointer crossing a dense row passes several marks; opening after a
  // short wait keeps the row quiet while a deliberate hover still answers.
  // Focus and a click open at once: both are deliberate already.
  const enter = () => {
    cancelClose()
    setOpen(true)
  }
  const hover = () => {
    cancelClose()
    if (open) return
    openTimer.current = window.setTimeout(() => {
      openTimer.current = null
      setOpen(true)
    }, 150)
  }
  const leave = () => {
    cancelClose()
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null
      if (!pinned.current && document.activeElement !== anchor.current) setOpen(false)
    }, 80)
  }
  useEffect(() => {
    if (!open) return
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      close()
    }
    const outside = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return
      if (root.current?.contains(event.target) || tooltip.current?.contains(event.target)) return
      close()
    }
    window.addEventListener("keydown", escape)
    document.addEventListener("pointerdown", outside, true)
    return () => {
      window.removeEventListener("keydown", escape)
      document.removeEventListener("pointerdown", outside, true)
    }
  }, [open])
  useEffect(() => () => cancelClose(), [])
  const [arrowLeft, setArrowLeft] = useState<number | null>(null)
  useLayoutEffect(() => {
    if (!open) return
    const update = () => {
      const anchorRect = anchor.current?.getBoundingClientRect()
      const tooltipRect = tooltip.current?.getBoundingClientRect()
      if (anchorRect === undefined || tooltipRect === undefined) return
      const next = placeTooltip(anchorRect, tooltipRect, { height: window.innerHeight, width: window.innerWidth })
      setPosition((current) => current?.left === next.left && current.top === next.top && current.placement === next.placement ? current : next)
      // The box is clamped to the viewport; the arrow keeps pointing at the
      // mark rather than at the middle of wherever the box landed.
      const centre = anchorRect.left + anchorRect.width / 2 - next.left
      setArrowLeft(Math.max(10, Math.min(tooltipRect.width - 10, centre)))
    }
    update()
    window.addEventListener("resize", update)
    window.addEventListener("scroll", update, true)
    return () => {
      window.removeEventListener("resize", update)
      window.removeEventListener("scroll", update, true)
    }
  }, [helpText, open, t(helpKey)])
  return (
    <>
      {/* Touch needs a large target, not a large mark: in a table head the mark
          steps in by its own reach and grows an invisible 36x36 target, which
          fits inside the cell and takes nothing from the next column. */}
      <span className="label-help relative inline-flex items-center [&>*+*]:ml-[5px] [.detail-dt_&]:block [.detail-dt_&]:max-w-full [.detail-dt_&>span]:block [.detail-dt_&>span]:w-fit [.detail-dt_&>span]:max-w-full [.detail-dt_&>span]:overflow-hidden [.detail-dt_&>span]:text-ellipsis [.detail-dt_&>span]:whitespace-nowrap [.detail-dt_&>span]:border-b [.detail-dt_&>span]:border-dotted [.detail-dt_&>span]:border-line4 [.lane-label>&]:flex-none [.chart-series-labels>&]:flex-none [.chart-series-labels>&+&]:before:mr-1.5 [.chart-series-labels>&+&]:before:text-fg4 [.chart-series-labels>&+&]:before:content-['·'] [.chart-series-labels_&>span]:overflow-hidden [.chart-series-labels_&>span]:text-ellipsis [.chart-series-labels_&>span]:whitespace-nowrap [.series-reading>&]:min-w-0 [.series-reading>&>span]:overflow-hidden [.series-reading>&>span]:text-ellipsis [.series-reading>&>span]:whitespace-nowrap [.entity-header-cell>&]:ml-1 [.entity-header-cell>&]:flex-none [.entity-header-cell>&]:opacity-0 [.entity-header-cell>&]:pointer-events-none [.entity-header-cell>&]:transition-opacity [.entity-header-cell>&]:duration-[120ms] [.entity-header-cell:hover>&]:opacity-100 [.entity-header-cell:hover>&]:pointer-events-auto [.entity-header-cell:focus-within>&]:opacity-100 [.entity-header-cell:focus-within>&]:pointer-events-auto coarse:[.entity-header-cell>&]:ml-0 coarse:[.entity-header-cell>&]:mr-[11px] coarse:[.entity-header-cell>&]:h-9 coarse:[.entity-header-cell>&]:self-stretch coarse:[.entity-header-cell>&]:opacity-100 coarse:[.entity-header-cell>&]:pointer-events-auto" onMouseEnter={hover} onMouseLeave={leave} ref={root}>
        {!iconOnly && <span>{t(labelKey)}</span>}
        <button
          aria-describedby={open ? id : undefined}
          aria-expanded={open}
          aria-label={`${t(labelKey)} — ${t("help.open")}`}
          className="help-dot inline-flex h-3.5 w-3.5 cursor-help items-center justify-center rounded-full border border-line4 bg-s2 p-0 text-xs leading-none text-accent3 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent [.series-reading_&]:flex-none [.chart-series-labels_&]:flex-none [.detail-dt_&]:absolute [.detail-dt_&]:inset-0 [.detail-dt_&]:m-0 [.detail-dt_&]:w-auto [.detail-dt_&]:border-0 [.detail-dt_&]:opacity-0 [.detail-dt_&]:focus-visible:opacity-100 coarse:[.entity-header-cell_&]:relative coarse:[.entity-header-cell_&]:after:absolute coarse:[.entity-header-cell_&]:after:-inset-[11px] coarse:[.entity-header-cell_&]:after:content-['']"
          data-testid={testId}
          onBlur={close}
          onClick={() => {
            pinned.current = !pinned.current
            setOpen(pinned.current)
          }}
          onFocus={enter}
          ref={anchor}
          type="button"
        >?</button>
      </span>
      {open && typeof document !== "undefined" && createPortal(
        <span
          className="fixed z-[1000] max-h-[calc(100vh_-_16px)] w-max max-w-[min(310px,70vw,calc(100vw_-_16px))] overflow-auto rounded-[var(--radius-sm)] border border-line3 bg-s2 px-[10px] py-[7px] text-left text-sm normal-case leading-[1.45] text-fg shadow-pop"
          data-placement={position?.placement}
          data-testid="help-tooltip"
          id={id}
          onMouseEnter={hover}
          onMouseLeave={leave}
          ref={tooltip}
          role="tooltip"
          style={{ left: position?.left ?? 0, top: position?.top ?? 0, visibility: position === null ? "hidden" : "visible" }}
        >
          <span
            aria-hidden="true"
            className={`absolute left-1/2 h-[7px] w-[7px] -translate-x-1/2 rotate-45 border-line3 bg-s2 ${position?.placement === "above" ? "-bottom-[4px] border-b border-r" : "-top-[4px] border-l border-t"}`}
            style={{ left: arrowLeft ?? "50%" }}
          />
          {helpText ?? t(helpKey)}
        </span>,
        document.body,
      )}
    </>
  )
}

export function placeTooltip(
  anchor: Rectangle,
  size: Pick<Rectangle, "height" | "width">,
  viewport: { readonly height: number; readonly width: number },
): TooltipPosition {
  const margin = 8
  const offset = 6
  const below = anchor.bottom + offset
  const above = anchor.top - size.height - offset
  const roomBelow = viewport.height - anchor.bottom
  const roomAbove = anchor.top
  const placement = below + size.height <= viewport.height - margin || (above < margin && roomBelow >= roomAbove)
    ? "below"
    : "above"
  const desiredTop = placement === "below" ? below : above
  const maximumTop = Math.max(margin, viewport.height - size.height - margin)
  const top = Math.max(margin, Math.min(maximumTop, desiredTop))
  const desiredLeft = anchor.left + anchor.width / 2 - size.width / 2
  const maximumLeft = Math.max(margin, viewport.width - size.width - margin)
  const left = Math.max(margin, Math.min(maximumLeft, desiredLeft))
  return { left, placement, top }
}

export function HelpPanel({
  items,
  onClose,
  t,
}: {
  readonly items: readonly { readonly label: string; readonly help: string }[]
  readonly onClose: () => void
  readonly t: Translate
}) {
  return (
    <aside aria-label={t("help.title")} className="fixed bottom-0 right-0 top-0 z-[100] w-[min(92vw,430px)] max-w-[430px] overflow-auto border-l border-line4 bg-s1 p-[18px] shadow-[-20px_0_50px_var(--color-shadow-a)]" data-testid="help-panel">
      <div className="flex items-center justify-between">
        <div>
          <p className="m-0 text-xs text-fg4">?</p>
          <h2>{t("help.title")}</h2>
        </div>
        <button aria-label={t("help.close")} className="icon-button" onClick={onClose} type="button">×</button>
      </div>
      <p className="border border-line3 bg-s2 p-2 text-sm text-accent2">{t("help.shortcut")}</p>
      <dl className="my-[18px]">
        {items.map((item) => <div className="border-t border-line2 py-3" key={item.label}><dt className="text-sm text-fg">{t(item.label)}</dt><dd className="mt-[5px] text-sm leading-[1.6] text-fg3">{t(item.help)}</dd></div>)}
      </dl>
    </aside>
  )
}
