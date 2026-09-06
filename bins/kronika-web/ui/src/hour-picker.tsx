import { ChevronLeft, ChevronRight } from "lucide-react"
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"

import { calendarDateLabel, calendarMonthDays, calendarMonthLabel, type DisplayTimeFormatter } from "./display-time"
import { useDisplayTime } from "./display-time-context"
import type { Translate } from "./help"
import type { Locale } from "./model"
import type { ReportVisibleRange } from "./report-transport"

const HOUR_MICROS = 3_600_000_000

export function HourPicker({
  availableHours,
  changeHour,
  hour,
  locale,
  visibleRange = null,
  t,
}: {
  readonly availableHours: readonly number[]
  readonly changeHour: (hour: number) => void
  readonly hour: number | null
  readonly locale: Locale
  readonly visibleRange?: ReportVisibleRange | null | undefined
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const selectable = useMemo(() => catalogueHours(availableHours, hour, visibleRange), [availableHours, hour, visibleRange])
  const committedDay = hour === null ? "" : time.dayKey(hour)
  const committedMonth = hour === null ? "" : time.monthKey(hour)
  const [open, setOpen] = useState(false)
  const [day, setDay] = useState(committedDay)
  const [month, setMonth] = useState(committedMonth)
  const [popover, setPopover] = useState<HourPopoverPlacement | null>(null)
  const root = useRef<HTMLDivElement>(null)
  const popoverNode = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const hourCells = useRef<(HTMLButtonElement | null)[]>([])
  const availableDays = useMemo(() => new Set(selectable.map((candidate) => time.dayKey(candidate))), [selectable, time])
  const dayHours = useMemo(() => hoursForDay(selectable, day, time), [day, selectable, time])
  const months = useMemo(() => [...new Set(selectable.map((candidate) => time.monthKey(candidate)))].sort(), [selectable, time])
  const monthIndex = months.indexOf(month)
  const currentIndex = hour === null ? -1 : selectable.indexOf(hour)
  const label = hour === null ? null : time.hourRange(hour)

  useEffect(() => {
    if (!open) return
    const selected = dayHours.indexOf(hour ?? Number.NaN)
    const frame = requestAnimationFrame(() => hourCells.current[Math.max(0, selected)]?.focus())
    const dismiss = (event: PointerEvent) => {
      if (event.target instanceof Node && (root.current?.contains(event.target) || popoverNode.current?.contains(event.target))) return
      setOpen(false)
    }
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      setOpen(false)
      trigger.current?.focus()
    }
    document.addEventListener("pointerdown", dismiss, true)
    window.addEventListener("keydown", escape)
    return () => {
      cancelAnimationFrame(frame)
      document.removeEventListener("pointerdown", dismiss, true)
      window.removeEventListener("keydown", escape)
    }
  }, [dayHours, hour, open])

  useLayoutEffect(() => {
    if (!open) return
    const place = () => {
      const anchor = root.current?.getBoundingClientRect()
      if (anchor === undefined) return
      setPopover(hourPopoverPlacement(anchor, {
        compact: window.matchMedia("(max-width: 760px)").matches,
        height: window.innerHeight,
        width: document.documentElement.clientWidth,
      }))
    }
    place()
    window.addEventListener("resize", place)
    return () => window.removeEventListener("resize", place)
  }, [open])

  const show = () => {
    if (hour === null) return
    if (open) return setOpen(false)
    setDay(committedDay)
    setMonth(committedMonth)
    setOpen(true)
  }
  const move = (index: number) => {
    const candidate = selectable[index]
    setOpen(false)
    if (candidate !== undefined) changeHour(candidate)
  }
  return <div
    className="relative flex items-center gap-1"
    data-testid="hour-picker"
    onBlur={(event) => {
      if (event.relatedTarget instanceof Node && (root.current?.contains(event.relatedTarget) || popoverNode.current?.contains(event.relatedTarget))) return
      setOpen(false)
    }}
    ref={root}
  >
    <button aria-label={t("hour.previous")} className="flex h-8 w-6 cursor-pointer items-center justify-center rounded-[var(--radius-sm)] border-0 bg-transparent p-0 text-fg3 transition-colors hover:bg-s3 hover:text-fg disabled:cursor-not-allowed disabled:opacity-45" data-testid="hour-previous" disabled={currentIndex <= 0} onClick={() => move(currentIndex - 1)} type="button"><ChevronLeft aria-hidden="true" size={15} /></button>
    <button
      aria-controls="hour-picker-popover"
      aria-expanded={open}
      aria-haspopup="dialog"
      aria-label={label === null ? t("hour.picker") : `${t("hour.picker")}: ${label.date} · ${label.primary}`}
      className="flex h-8 min-w-[176px] cursor-pointer flex-col items-start justify-center rounded-[var(--radius-sm)] border border-line2 bg-s2 px-2 text-left text-fg2 transition-colors hover:bg-s3 disabled:cursor-not-allowed disabled:opacity-45 max-[760px]:min-w-0"
      data-testid="hour-picker-trigger"
      disabled={hour === null}
      onClick={show}
      ref={trigger}
      type="button"
    >
      <strong className="whitespace-nowrap text-xs font-semibold leading-[1.2] text-fg">{label?.primary ?? "—"}</strong>
      {label !== null && <small className="text-xs leading-none text-fg4">{label.date}</small>}
    </button>
    <button aria-label={t("hour.next")} className="flex h-8 w-6 cursor-pointer items-center justify-center rounded-[var(--radius-sm)] border-0 bg-transparent p-0 text-fg3 transition-colors hover:bg-s3 hover:text-fg disabled:cursor-not-allowed disabled:opacity-45" data-testid="hour-next" disabled={currentIndex < 0 || currentIndex >= selectable.length - 1} onClick={() => move(currentIndex + 1)} type="button"><ChevronRight aria-hidden="true" size={15} /></button>
    {open && hour !== null && createPortal(<div aria-label={t("hour.picker")} className="fixed left-2.5 top-2.5 z-[1150] grid max-h-[calc(100vh_-_20px)] w-[min(560px,calc(100vw_-_20px))] grid-cols-2 overflow-auto rounded-[var(--radius-md)] border border-line3 bg-s1 p-2.5 shadow-[var(--shadow-pop)] max-[760px]:w-[min(304px,calc(100vw_-_20px))] max-[760px]:grid-cols-1" data-testid="hour-popover" id="hour-picker-popover" ref={popoverNode} role="dialog" style={popover ?? undefined}>
      <header className="col-span-full flex min-h-[27px] items-center justify-between border-b border-line3 px-0.5 pb-[7px] max-[760px]:col-span-1">
        <strong className="font-sans text-sm font-medium text-fg" data-testid="hour-current">{calendarDateLabel(time.dayKey(hour), locale)}</strong>
      </header>
      <div className="min-w-0 border-r border-line3 pb-0 pl-0 pr-[9px] pt-[7px] max-[760px]:border-b max-[760px]:border-r-0 max-[760px]:px-0 max-[760px]:py-[7px]" data-testid="day-picker">
        <div className="flex items-center justify-between">
          <button aria-label={t("hour.month.previous")} className="flex cursor-pointer items-center rounded-[var(--radius-xs)] border-0 bg-transparent p-1 text-fg2 transition-colors hover:bg-s3 disabled:cursor-not-allowed disabled:opacity-30" disabled={monthIndex <= 0} onClick={() => setMonth(months[monthIndex - 1] ?? month)} type="button"><ChevronLeft aria-hidden="true" size={15} /></button>
          <strong aria-live="polite" className="text-md font-semibold text-fg2" data-testid="picker-month">{calendarMonthLabel(month, locale)}</strong>
          <button aria-label={t("hour.month.next")} className="flex cursor-pointer items-center rounded-[var(--radius-xs)] border-0 bg-transparent p-1 text-fg2 transition-colors hover:bg-s3 disabled:cursor-not-allowed disabled:opacity-30" disabled={monthIndex < 0 || monthIndex >= months.length - 1} onClick={() => setMonth(months[monthIndex + 1] ?? month)} type="button"><ChevronRight aria-hidden="true" size={15} /></button>
        </div>
        <div aria-label={t("hour.picker")} className="grid grid-cols-7 gap-0.5" data-testid="day-grid" role="group">
          {calendarMonthDays(month).map((candidate) => {
            const hasData = availableDays.has(candidate)
            return <button aria-label={calendarDateLabel(candidate, locale)} aria-pressed={candidate === day} className={`h-[29px] p-0 rounded-[var(--radius-xs)] font-sans text-md font-medium tabular-nums aria-pressed:border-accent2 aria-pressed:bg-s4 aria-pressed:text-accent3 ${hasData ? "cursor-pointer border border-line2 bg-s2 text-fg2" : "border-0 bg-transparent text-fg-null"}`} data-day={candidate} data-testid="day-cell" disabled={!hasData} key={candidate} onClick={() => setDay(candidate)} type="button">{candidate.slice(-2)}</button>
          })}
        </div>
      </div>
      <div aria-label={t("hour.hours")} className="ml-[9px] mt-[7px] grid grid-cols-3 content-start gap-1 max-[760px]:ml-0 max-[760px]:mt-2" data-testid="hour-grid" role="group">
        {dayHours.map((candidate, index) => {
          const clock = time.hourLabel(candidate)
          return <button aria-label={clock} aria-pressed={candidate === hour} className="h-[33px] cursor-pointer whitespace-nowrap rounded-[var(--radius-xs)] border border-line2 bg-s2 px-0.5 font-mono text-md tabular-nums transition-colors text-fg3 hover:bg-s3 hover:text-fg aria-pressed:border-accent2 aria-pressed:bg-s4 aria-pressed:text-accent3 aria-pressed:shadow-[inset_0_0_0_1px_var(--color-accent-line)]" data-instant={candidate} data-testid="hour-cell" key={candidate} onClick={() => { changeHour(candidate); setOpen(false); trigger.current?.focus() }} onKeyDown={(event) => { const next = pickerFocusIndex(index, event.key, dayHours.length); if (next !== null) { event.preventDefault(); hourCells.current[next]?.focus() } }} ref={(node) => { hourCells.current[index] = node }} tabIndex={candidate === hour || hour !== null && time.dayKey(hour) !== day && index === 0 ? 0 : -1} type="button">{clock}</button>
        })}
      </div>
    </div>, document.body)}
  </div>
}

interface HourPopoverPlacement {
  readonly left: number
  readonly maxHeight: number
  readonly top: number
  readonly width: number
}

export function hourPopoverPlacement(
  anchor: { readonly bottom: number; readonly left: number },
  viewport: { readonly compact?: boolean; readonly height: number; readonly width: number },
): HourPopoverPlacement {
  const edge = 10
  const compact = viewport.compact ?? viewport.width <= 760
  const width = Math.max(0, Math.min(compact ? 304 : 560, viewport.width - 2 * edge))
  const left = Math.min(Math.max(edge, anchor.left), Math.max(edge, viewport.width - width - edge))
  const top = Math.max(edge, anchor.bottom + 6)
  return { left, maxHeight: Math.max(0, viewport.height - top - edge), top, width }
}

export function catalogueHours(
  available: readonly number[],
  current: number | null,
  visibleRange: ReportVisibleRange | null = null,
): readonly number[] {
  if (visibleRange !== null) return visibleHours(visibleRange)
  return [...new Set([...available, current].filter((candidate): candidate is number => candidate !== null && Number.isSafeInteger(candidate)))].sort((left, right) => left - right)
}

export function visibleHours(range: ReportVisibleRange): readonly number[] {
  if (!Number.isSafeInteger(range.from) || !Number.isSafeInteger(range.toExclusive) || range.from >= range.toExclusive) return []
  const first = Math.floor(range.from / HOUR_MICROS) * HOUR_MICROS
  const last = Math.floor((range.toExclusive - 1) / HOUR_MICROS) * HOUR_MICROS
  const hours: number[] = []
  for (let hour = first; hour <= last; hour += HOUR_MICROS) hours.push(hour)
  return hours
}

export function hoursForDay(hours: readonly number[], day: string, time: Pick<DisplayTimeFormatter, "dayKey">): readonly number[] {
  return hours.filter((candidate) => time.dayKey(candidate) === day)
}

export function pickerFocusIndex(current: number, key: string, length: number): number | null {
  if (length === 0) return null
  if (key === "Home") return 0
  if (key === "End") return length - 1
  const step = key === "ArrowLeft" ? -1 : key === "ArrowRight" ? 1 : key === "ArrowUp" ? -3 : key === "ArrowDown" ? 3 : 0
  return step === 0 ? null : Math.max(0, Math.min(length - 1, current + step))
}

export function hourHasData(hour: number, available: readonly number[]): boolean {
  return available.includes(hour)
}
