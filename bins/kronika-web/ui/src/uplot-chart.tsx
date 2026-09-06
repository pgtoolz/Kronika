import uPlot, { type AlignedData } from "uplot"
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react"

import type { DisplayTimeFormatter } from "./display-time"
import { useDisplayTime } from "./display-time-context"
import { LabelHelp, type Translate } from "./help"
import { orderedRecordedTimes } from "./keyboard"
import { humanDurationAxis, type Locale } from "./model"
import { niceCeiling } from "./spark"

export type ChartScale = "percent" | "nonnegative" | "signed"

export interface RecordedPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export interface RecordedSeries {
  readonly color: "cyan" | "amber" | "violet" | "green" | "red" | "gray" | "blue" | "rose"
  readonly id: string
  readonly helpKey: string
  readonly label: string
  readonly labelKey: string
  readonly points: readonly RecordedPoint[]
  readonly scale: ChartScale
  readonly tick?: ((number: number, locale: Locale) => string) | undefined
  readonly tickAxis?: "duration" | undefined
  readonly unit: string
  readonly value: (number: number, locale: Locale) => string
}

export interface ChartFrame {
  readonly data: AlignedData
  readonly isolated: ReadonlyMap<number, readonly number[]>
  readonly timestamps: readonly number[]
}

export interface ScalePartition {
  readonly key: string
  readonly label: string
  readonly scale: ChartScale
  readonly seriesIds: readonly string[]
  readonly unit: string
}

export interface ChartDecoration {
  readonly from: number
  readonly to: number
  readonly tone: "future" | "selection" | "unavailable"
}

export interface ChartThreshold {
  readonly below: number
  readonly seriesId: string
}

export type ChartVariant = "default" | "preview" | "inspector"

// Finding markers get their own horizontal lane above the plot, so they never
// occlude the drawn lines. The lane exists only when a marker layer is passed.
export const MARKER_LANE_PX = 20

// Many series on one chart read as spaghetti: the legend becomes the picker,
// one series at a time, with "All" returning the composition.
export interface SeriesIsolation {
  readonly anchor?: string | undefined
}

export interface SeriesStats {
  readonly last: number
  readonly max: number
  readonly min: number
  readonly p50: number
  readonly p90: number
  readonly p99: number
}

// Statistics of the drawn samples, nearest-rank: the chart claims nothing the
// pixels do not show. `last` is the latest sample in time order.
export function seriesStats(values: readonly number[]): SeriesStats | null {
  const finite = values.filter((value) => Number.isFinite(value))
  if (finite.length === 0) return null
  const sorted = [...finite].sort((left, right) => left - right)
  const pick = (quantile: number) => sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(quantile * sorted.length) - 1))]!
  return { last: finite.at(-1)!, max: sorted.at(-1)!, min: sorted[0]!, p50: pick(0.5), p90: pick(0.9), p99: pick(0.99) }
}

export interface ChartStatsRow {
  readonly line: RecordedSeries
  readonly stats: SeriesStats
}

export function chartStatsRows(series: readonly RecordedSeries[], frame: ChartFrame): readonly ChartStatsRow[] {
  return series.flatMap((line, ordinal) => {
    const values = Array.from(frame.data[ordinal + 1] ?? []).filter((value): value is number => typeof value === "number")
    const measured = seriesStats(values)
    return measured === null ? [] : [{ line, stats: measured }]
  })
}

const ISOLATE_ABOVE = 4

// The operator's explicit choice wins; untouched state auto-isolates a crowded
// chart onto its anchor series (or the first) and leaves small charts whole.
export function effectiveIsolation(
  seriesIds: readonly string[],
  choice: string | null | undefined,
  anchor: string | undefined,
): string | null {
  if (seriesIds.length <= 1) return null
  if (choice !== undefined && choice !== null && seriesIds.includes(choice)) return choice
  if (choice === null) return null
  if (seriesIds.length > ISOLATE_ABOVE) return anchor !== undefined && seriesIds.includes(anchor) ? anchor : seriesIds[0]!
  return null
}

const NO_DECORATIONS: readonly ChartDecoration[] = []

interface ChartTopologyPartition {
  readonly key: string
  readonly scale: ChartScale
  readonly seriesIndices: readonly number[]
}

interface ChartTopology {
  readonly partitions: readonly ChartTopologyPartition[]
  readonly seriesScaleKeys: readonly string[]
  readonly signature: string
}

interface ChartRuntimeState {
  readonly decorations: readonly ChartDecoration[]
  readonly frame: ChartFrame
  readonly locale: Locale
  readonly navigationTimes: readonly number[]
  readonly series: readonly RecordedSeries[]
  readonly threshold: ChartThreshold | undefined
  readonly time: DisplayTimeFormatter
}

export function UPlotChart({
  cursor,
  hour,
  isolate,
  locale,
  onCursor,
  onPreview,
  reading,
  series,
  className,
  decorations = NO_DECORATIONS,
  markerLayer,
  navigationTimestamps,
  onPlotWidth,
  stats = false,
  status,
  testId,
  threshold,
  t,
  variant = "default",
}: {
  readonly className?: string | undefined
  readonly cursor?: number | undefined
  readonly decorations?: readonly ChartDecoration[] | undefined
  readonly hour: number
  readonly isolate?: SeriesIsolation | undefined
  readonly locale: Locale
  readonly markerLayer?: ReactNode | undefined
  readonly navigationTimestamps?: readonly number[] | undefined
  readonly onCursor?: ((timestamp: number) => void) | undefined
  readonly onPreview?: ((timestamp: number | null) => void) | undefined
  readonly onPlotWidth?: ((width: number) => void) | undefined
  readonly reading?: string | undefined
  readonly series: readonly RecordedSeries[]
  readonly stats?: boolean | undefined
  readonly status?: ReactNode | undefined
  readonly testId?: string | undefined
  readonly threshold?: ChartThreshold | undefined
  readonly t: Translate
  readonly variant?: ChartVariant | undefined
}) {
  const time = useDisplayTime()
  const titleId = useId()
  const summaryId = useId()
  const shell = useRef<HTMLElement>(null)
  const host = useRef<HTMLDivElement>(null)
  const plot = useRef<uPlot | null>(null)
  const onCursorRef = useRef(onCursor)
  const onPreviewRef = useRef(onPreview)
  const onPlotWidthRef = useRef(onPlotWidth)
  const selectedRef = useRef<number | null>(null)
  const [hovered, setHovered] = useState<number | null>(null)
  const [keyboardIndex, setKeyboardIndex] = useState(0)
  const end = hour + 3_600_000_000
  const [isolatedChoice, setIsolatedChoice] = useState<string | null | undefined>(undefined)
  const isolatedId = isolate === undefined
    ? null
    : effectiveIsolation(series.map(({ id }) => id), isolatedChoice, isolate.anchor)
  // A stable identity: filtering per render would rebuild the whole uPlot
  // instance on every cursor step while a series is isolated.
  const drawnSeries = useMemo(() => isolatedId === null ? series : series.filter(({ id }) => id === isolatedId), [isolatedId, series])
  const visibleSeries = useMemo(() => drawnSeries.map((line) => ({
    ...line,
    points: line.points.filter(({ timestamp }) => timestamp >= hour && timestamp < end),
  })), [drawnSeries, end, hour])
  const frame = useMemo(() => alignRecordedSeries(visibleSeries), [visibleSeries])
  const navigationTimes = useMemo(
    () => chartNavigationTimestamps(frame, navigationTimestamps, hour, end),
    [end, frame, hour, navigationTimestamps],
  )
  const compact = variant === "preview"
  const topologySignature = chartTopology(visibleSeries, compact).signature
  const topology = useMemo(() => chartTopology(visibleSeries, compact), [compact, topologySignature])
  const [themeRevision, setThemeRevision] = useState(0)
  const exact = hovered === null ? null : exactReadings(frame, drawnSeries, hovered, locale, time)
  const selected = cursor === undefined || cursor < hour || cursor >= end ? null : cursor
  const keyboardTimestamp = navigationTimes[keyboardIndex] ?? null
  const runtime = useRef<ChartRuntimeState>({ decorations, frame, locale, navigationTimes, series: visibleSeries, threshold, time })
  const hoveredRef = useRef(hovered)
  onCursorRef.current = onCursor
  onPreviewRef.current = onPreview
  onPlotWidthRef.current = onPlotWidth
  selectedRef.current = selected
  hoveredRef.current = hovered
  runtime.current = { decorations, frame, locale, navigationTimes, series: visibleSeries, threshold, time }

  useLayoutEffect(() => {
    const element = host.current
    if (element === null || runtime.current.frame.timestamps.length === 0) return
    const initialBounds = element.getBoundingClientRect()
    const options = chartOptions(topology, runtime, hour, end, selectedRef, Math.max(1, Math.round(initialBounds.width)), Math.max(1, Math.round(initialBounds.height)), compact, variant !== "default", markerLayer !== undefined, (chart) => {
      if (variant !== "default") return
      const index = chart.cursor.idx
      const timestamp = index === null || index === undefined ? null : runtime.current.frame.timestamps[index] ?? null
      setHovered(timestamp)
    }, (chart) => {
      const root = shell.current
      if (root === null) return
      const left = chart.over.offsetLeft
      const top = chart.over.offsetTop
      const width = chart.over.offsetWidth
      const endReserve = Number.parseFloat(getComputedStyle(root).getPropertyValue("--chart-marker-end-reserve")) || 0
      root.style.setProperty("--chart-plot-left", `${element.offsetLeft + left}px`)
      root.style.setProperty("--chart-plot-top", `${element.offsetTop + top}px`)
      root.style.setProperty("--chart-plot-width", `${width}px`)
      onPlotWidthRef.current?.(Math.max(1, width - endReserve))
    })
    const chart = new uPlot(options, runtime.current.frame.data, element)
    plot.current = chart
    const canvas = chart.root.querySelector("canvas")
    canvas?.setAttribute("aria-hidden", "true")
    const pointerTimestamp = (clientX: number) => {
      const bounds = chart.over.getBoundingClientRect()
      return nearestRecordedTimestamp(runtime.current.navigationTimes, chart.posToVal(clientX - bounds.left, "x"))
    }
    let pointerFrame = 0
    let previewed: number | null = null
    const clearPreview = () => {
      cancelAnimationFrame(pointerFrame)
      pointerFrame = 0
      if (previewed === null) return
      previewed = null
      onPreviewRef.current?.(null)
    }
    const follow = (event: PointerEvent) => {
      if (event.pointerType === "touch") return
      const clientX = event.clientX
      cancelAnimationFrame(pointerFrame)
      pointerFrame = requestAnimationFrame(() => {
        pointerFrame = 0
        const timestamp = pointerTimestamp(clientX)
        if (timestamp === null || timestamp === previewed) return
        previewed = timestamp
        onPreviewRef.current?.(timestamp)
      })
    }
    const select = (event: PointerEvent) => {
      cancelAnimationFrame(pointerFrame)
      pointerFrame = 0
      const timestamp = pointerTimestamp(event.clientX)
      if (timestamp !== null) {
        onCursorRef.current?.(timestamp)
        if (previewed !== null) previewed = null
      } else clearPreview()
    }
    if (variant !== "default") {
      chart.over.addEventListener("pointermove", follow)
      chart.over.addEventListener("pointerleave", clearPreview)
      chart.over.addEventListener("pointercancel", clearPreview)
    }
    chart.over.addEventListener("pointerup", select)
    let resizeFrame = 0
    const resize = () => {
      cancelAnimationFrame(resizeFrame)
      resizeFrame = requestAnimationFrame(() => {
        const bounds = element.getBoundingClientRect()
        chart.setSize({ width: Math.max(1, Math.round(bounds.width)), height: Math.max(1, Math.round(bounds.height)) })
      })
    }
    resize()
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(resize)
    observer?.observe(element)
    window.addEventListener("resize", resize)
    return () => {
      cancelAnimationFrame(resizeFrame)
      cancelAnimationFrame(pointerFrame)
      observer?.disconnect()
      window.removeEventListener("resize", resize)
      if (variant !== "default") chart.over.removeEventListener("pointermove", follow)
      if (variant !== "default") chart.over.removeEventListener("pointerleave", clearPreview)
      if (variant !== "default") chart.over.removeEventListener("pointercancel", clearPreview)
      chart.over.removeEventListener("pointerup", select)
      clearPreview()
      chart.destroy()
      plot.current = null
    }
  }, [compact, end, frame.timestamps.length === 0, hour, markerLayer !== undefined, themeRevision, topology, variant])

  useLayoutEffect(() => {
    const chart = plot.current
    if (chart === null) return
    const latest = runtime.current
    chart.batch(() => {
      chart.setData(latest.frame.data, false)
      for (const partition of topology.partitions) {
        const [min, max] = chartPartitionRange(partition, latest.series, latest.frame)
        chart.setScale(partition.key, { min, max })
      }
      chart.redraw(true, true)
    })
    const hoverTimestamp = hoveredRef.current
    if (hoverTimestamp === null) return
    if (!latest.frame.timestamps.includes(hoverTimestamp)) {
      hoveredRef.current = null
      setHovered(null)
      return
    }
    chart.setCursor({ left: chart.valToPos(hoverTimestamp, "x"), top: chart.cursor.top ?? 0 }, false)
    setHovered((current) => current === hoverTimestamp ? current : hoverTimestamp)
  }, [decorations, frame, locale, navigationTimes, threshold, time, topology, visibleSeries])

  useEffect(() => {
    const observer = new MutationObserver(() => setThemeRevision((revision) => revision + 1))
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "data-theme"] })
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    plot.current?.redraw()
  }, [selected])

  useEffect(() => {
    if (selected === null) return
    const nearest = nearestRecordedTimestamp(navigationTimes, selected)
    const index = nearest === null ? -1 : navigationTimes.indexOf(nearest)
    if (index >= 0) setKeyboardIndex((current) => current === index ? current : index)
  }, [navigationTimes, selected])

  useEffect(() => {
    setKeyboardIndex((index) => Math.min(index, Math.max(0, navigationTimes.length - 1)))
  }, [navigationTimes.length])

  const summary = chartSummary(visibleSeries, frame, hour, end, locale, time)
  const isolatable = isolate !== undefined && series.length > 1
  const statsRows = useMemo(() => stats ? chartStatsRows(visibleSeries, frame) : [], [frame, stats, visibleSeries])
  return <figure
    className={`uplot-figure launch-timeline relative m-0 flex min-w-0 max-w-full flex-col overflow-hidden ${variant === "preview" ? "h-[94px] min-h-[94px] basis-[94px] px-1 pb-0.5 pt-0.5" : variant === "inspector" ? "h-[300px] min-h-[300px] p-2" : stats ? "h-[244px]" : "h-[200px]"} [.pg-table-shell_.pg-detail_&]:h-[200px] [.pg-table-shell_.pg-detail_&]:max-h-[200px] [.pg-table-shell_.pg-detail_&]:flex-none${className === undefined ? "" : ` ${className}`}${isolatable ? " uplot-isolatable" : ""}`}
    data-hovered-timestamp={hovered ?? undefined}
    data-testid={testId}
    data-navigation-count={navigationTimes.length}
    data-selected-timestamp={selected ?? undefined}
    ref={shell}
  >
    {variant !== "preview" && <figcaption className="mb-[5px] grid min-h-[24px] flex-none grid-cols-[minmax(0,1fr)_auto] items-center gap-2 font-sans text-xs font-medium text-fg3 [.uplot-isolatable_&]:gap-y-1 max-[520px]:px-1" id={titleId}>
      <span className={`chart-series-labels flex min-w-0 flex-auto items-center gap-1.5 [&::-webkit-scrollbar]:hidden ${isolatable ? "flex-wrap gap-y-1 overflow-visible whitespace-normal" : "overflow-x-auto whitespace-nowrap [scrollbar-width:none]"}`}>{isolatable && <button
        aria-pressed={isolatedId === null}
        className="series-pick min-h-5 cursor-pointer rounded-[var(--radius-xs)] border border-line3 bg-s2 px-1.5 py-px text-xs text-fg2 transition-colors hover:bg-s3 aria-pressed:border-accent aria-pressed:bg-accent-soft aria-pressed:text-fg"
        data-testid={testId === undefined ? undefined : `${testId}-all`}
        onClick={() => setIsolatedChoice(null)}
        type="button"
      ><span aria-hidden="true" className="mr-1 inline-block h-1.5 w-1.5 rounded-full bg-fg4" />{t("chart.series.all")}</button>}{series.map((line) => isolatable
        ? <span className="inline-flex min-w-0 items-center gap-0.5" key={line.id}><button
          aria-pressed={isolatedId === line.id}
          className="series-pick min-h-5 cursor-pointer rounded-[var(--radius-xs)] border border-line3 bg-s2 px-1.5 py-px text-xs text-fg2 transition-colors hover:bg-s3 aria-pressed:border-accent aria-pressed:bg-accent-soft aria-pressed:text-fg"
          data-testid={testId === undefined ? undefined : `${testId}-series-${line.id}`}
          onClick={() => setIsolatedChoice(isolatedId === line.id ? null : line.id)}
          type="button"
        ><span aria-hidden="true" className="mr-1 inline-block h-1.5 w-1.5 rounded-full" style={{ backgroundColor: `var(${chartColor(line.color)})` }} />{line.label}</button><LabelHelp helpKey={line.helpKey} iconOnly labelKey={line.labelKey} t={t} /></span>
        : <span className="inline-flex min-w-0 items-center gap-1" key={line.id}><span aria-hidden="true" className="h-1.5 w-1.5 flex-none rounded-full" style={{ backgroundColor: `var(${chartColor(line.color)})` }} /><LabelHelp helpKey={line.helpKey} labelKey={line.labelKey} t={t} /></span>)}</span>
      {reading !== undefined && <strong className={`chart-current col-start-2 min-w-0 flex-none overflow-hidden text-ellipsis whitespace-nowrap font-mono font-normal tabular-nums text-fg2 ${isolatable ? "ml-auto max-w-none" : ""}`}>{reading}</strong>}
    </figcaption>}
    <p className="chart-summary absolute m-0 h-px w-px overflow-hidden whitespace-nowrap [clip-path:inset(50%)]" id={summaryId}>{summary}</p>
    <div aria-describedby={summaryId} aria-label={drawnSeries.map(({ label, unit }) => `${label}${unit === "" ? "" : `, ${unit}`}`).join("; ")} className="uplot-host h-[180px] w-full min-w-0 max-w-full min-h-0 flex-auto overflow-hidden [&>.uplot]:h-full [&>.uplot]:!w-full [&_.u-wrap]:max-w-full [.timeline-chart_&]:h-auto [.timeline-chart_&]:min-h-0" ref={host} role="img" />
    {stats && <div className="mt-1 min-h-[46px] flex-none overflow-hidden border-t border-line2 pt-1 font-mono text-sm tabular-nums text-fg3" data-testid="chart-stats">
      {statsRows.length !== 0 && <div className={`grid min-w-0 gap-x-2 px-1 text-right [&>*:first-child]:text-left ${variant === "inspector" ? "grid-cols-[minmax(82px,1.35fr)_repeat(3,minmax(48px,1fr))] gap-x-1" : "grid-cols-[minmax(90px,1.6fr)_repeat(6,minmax(48px,1fr))] max-[760px]:grid-cols-[minmax(82px,1.35fr)_repeat(3,minmax(48px,1fr))] max-[760px]:gap-x-1"}`}>
        <span aria-hidden="true" /><span>{t("chart.stats.last")}</span><span className={variant === "inspector" ? "hidden" : "max-[760px]:hidden"}>{t("chart.stats.min")}</span><span>{t("chart.stats.max")}</span><span className={variant === "inspector" ? "hidden" : "max-[760px]:hidden"}>{t("chart.stats.p50")}</span><span className={variant === "inspector" ? "hidden" : "max-[760px]:hidden"}>{t("chart.stats.p90")}</span><span>{t("chart.stats.p99")}</span>
        {statsRows.map(({ line, stats: measured }) => <div className="contents" key={line.id}>
          <strong className="overflow-hidden text-ellipsis whitespace-nowrap text-left font-sans font-medium text-fg2"><span aria-hidden="true" className="mr-1 inline-block h-1.5 w-1.5 rounded-full" style={{ backgroundColor: `var(${chartColor(line.color)})` }} />{line.label}</strong>
          <span className="overflow-hidden text-ellipsis whitespace-nowrap text-fg2">{line.value(measured.last, locale)}</span><span className={variant === "inspector" ? "hidden" : "max-[760px]:hidden"}>{line.value(measured.min, locale)}</span><span className="overflow-hidden text-ellipsis whitespace-nowrap">{line.value(measured.max, locale)}</span><span className={variant === "inspector" ? "hidden" : "max-[760px]:hidden"}>{line.value(measured.p50, locale)}</span><span className={variant === "inspector" ? "hidden" : "max-[760px]:hidden"}>{line.value(measured.p90, locale)}</span><span className="overflow-hidden text-ellipsis whitespace-nowrap">{line.value(measured.p99, locale)}</span>
        </div>)}
      </div>}
    </div>}
    {status !== undefined && <div className="uplot-status pointer-events-none absolute bottom-7 right-0 z-[8] flex items-center justify-center left-[var(--chart-plot-left,0)] top-[var(--chart-plot-top,26px)] [&_[data-testid=series-status]]:min-h-0 [&_[data-testid=series-status]]:border [&_[data-testid=series-status]]:border-line2 [&_[data-testid=series-status]]:bg-s2 [&_[data-testid=series-status]]:px-[9px] [&_[data-testid=series-status]]:py-[3px]">{status}</div>}
    {markerLayer !== undefined && <div className="pointer-events-none absolute z-[7] left-[var(--chart-plot-left,62px)] w-[max(1px,calc(var(--chart-plot-width,calc(100%_-_70px))_-_var(--chart-marker-end-reserve,0px)))]" data-testid="chart-marker-track" style={{ height: MARKER_LANE_PX - 2, top: `max(0px, calc(var(--chart-plot-top, 44px) - ${MARKER_LANE_PX}px))` }}>{markerLayer}</div>}
    {/* The 94px preview cannot host a stacked readout: there it collapses to
        one wrapping line pinned inside the figure instead of spilling over
        the controls below. */}
    {exact !== null && variant === "default" && <div aria-hidden="true" className="chart-tooltip pointer-events-none absolute right-2 z-[9] grid max-w-[min(340px,calc(100%_-_16px))] gap-[3px] rounded-[var(--radius-md)] border border-line3 bg-s2/95 px-2 py-1.5 font-sans text-xs shadow-[var(--shadow-pop)] top-[calc(var(--chart-plot-top,26px)+6px)] [&_span]:flex [&_span]:justify-between [&_span]:gap-3 [&_strong]:font-mono [&_strong]:font-normal [&_strong]:tabular-nums [&_strong]:text-fg [&_time]:flex [&_time]:justify-between [&_time]:gap-2 [&_time_strong]:font-sans [&_time_strong]:font-medium" data-testid="chart-hover-readout">
        {exact.values.map(({ label, output, unit }) => <span key={label}>{label}{unit === "" ? "" : ` (${unit})`}<strong>{output}</strong></span>)}
      </div>}
    <input
      aria-label={locale === "ru" ? "Запись графика" : "Chart sample"}
      aria-valuetext={keyboardTimestamp === null ? undefined : navigationSampleText(series, frame, navigationTimes, keyboardTimestamp, locale, time)}
      className="chart-navigator absolute m-0 h-px w-px overflow-hidden whitespace-nowrap [clip-path:inset(50%)] focus:left-2 focus:z-[9] focus:h-7 focus:w-[min(320px,calc(100%-16px))] focus:[clip-path:none]"
      data-recorded-timestamp={keyboardTimestamp ?? undefined}
      disabled={navigationTimes.length === 0}
      max={Math.max(0, navigationTimes.length - 1)}
      min="0"
      onChange={(event) => {
        const index = Number(event.currentTarget.value)
        const timestamp = navigationTimes[index]
        setKeyboardIndex(index)
        if (timestamp !== undefined) {
          setHovered(timestamp)
          if (timestamp !== selectedRef.current) onCursorRef.current?.(timestamp)
        }
      }}
      type="range"
      value={keyboardIndex}
    />
  </figure>
}

export function alignRecordedSeries(series: readonly RecordedSeries[]): ChartFrame {
  const allTimestamps = series.flatMap((line) => line.points.map((point) => point.timestamp))
  const invalid = allTimestamps.find((timestamp) => !Number.isSafeInteger(timestamp))
  if (invalid !== undefined) throw new Error(`invalid chart timestamp ${invalid}`)
  const timestamps = [...new Set(allTimestamps)]
    .sort((left, right) => left - right)
  const positions = new Map(timestamps.map((timestamp, index) => [timestamp, index]))
  const isolated = new Map<number, readonly number[]>()
  const columns = series.map((line, seriesIndex) => {
    const column: (number | null | undefined)[] = Array(timestamps.length).fill(undefined)
    for (const point of line.points.slice().sort((left, right) => left.timestamp - right.timestamp || left.segmentId.localeCompare(right.segmentId))) {
      const index = positions.get(point.timestamp)
      if (index !== undefined) {
        const current = column[index]
        if (current !== undefined && !Object.is(current, point.value)) {
          // Adjacent segments overlap at their boundary. A new segment records
          // an explicit null there when a rate has no cross-restart baseline;
          // that discontinuity must win over the previous segment's endpoint.
          if (current === null || point.value === null) column[index] = null
          else throw new Error(`conflicting chart sample ${line.id}@${point.timestamp}`)
        } else column[index] = point.value
      }
    }
    isolated.set(seriesIndex + 1, isolatedSampleIndices(column))
    return column
  })
  return { data: [timestamps, ...columns] as AlignedData, isolated, timestamps }
}

export function chartNavigationTimestamps(
  frame: Pick<ChartFrame, "timestamps">,
  navigationTimestamps: readonly number[] | undefined,
  hour: number,
  end: number,
): readonly number[] {
  if (navigationTimestamps === undefined) return frame.timestamps
  return orderedRecordedTimes(navigationTimestamps.filter((timestamp) => timestamp >= hour && timestamp < end))
}

export function isolatedSampleIndices(values: readonly (number | null | undefined)[]): readonly number[] {
  const output: number[] = []
  for (let index = 0; index < values.length; index += 1) {
    if (typeof values[index] !== "number") continue
    let before = index - 1
    while (before >= 0 && values[before] === undefined) before -= 1
    let after = index + 1
    while (after < values.length && values[after] === undefined) after += 1
    if ((before < 0 || values[before] === null) && (after >= values.length || values[after] === null)) output.push(index)
  }
  return output
}

export function exactReadings(frame: ChartFrame, series: readonly RecordedSeries[], timestamp: number, locale: Locale, time: Pick<DisplayTimeFormatter, "axis" | "clock">) {
  const index = frame.timestamps.indexOf(timestamp)
  if (index < 0) return null
  return {
    time: compactChartTime(timestamp, time, chartSecondsUseful(frame.timestamps, time)),
    values: series.map((line, ordinal) => {
      const stored = frame.data[ordinal + 1]?.[index]
      return { label: line.label, output: typeof stored === "number" ? line.value(stored, locale) : "—", unit: line.unit }
    }),
  }
}

export function scaleRange(scale: ChartScale, values: readonly number[]): readonly [number, number] {
  if (scale === "percent") return [0, 100]
  const finite = values.filter(Number.isFinite)
  if (scale === "nonnegative") return [0, niceCeiling(Math.max(0, ...finite))]
  if (finite.length === 0) return [-1, 1]
  const low = Math.min(...finite)
  const high = Math.max(...finite)
  if (low === high) {
    if (low === 0) return [-1, 1]
    return low < 0 ? [-niceCeiling(-low), 0] : [0, niceCeiling(high)]
  }
  return [low < 0 ? -niceCeiling(-low) : 0, high > 0 ? niceCeiling(high) : 0]
}

export function chartTimeRange(hour: number, end: number, width: number, gutter = 52, sideAxes = 70): [number, number] {
  const drawable = Math.max(1, width - sideAxes - gutter)
  return [hour, end + (end - hour) * gutter / drawable]
}

export function nearestRecordedTimestamp(timestamps: readonly number[], target: number): number | null {
  const first = timestamps[0]
  if (first === undefined) return null
  const last = timestamps.at(-1)!
  if (target <= first) return first
  if (target >= last) return last
  let low = 1
  let high = timestamps.length - 1
  while (low < high) {
    const middle = Math.floor((low + high) / 2)
    if (timestamps[middle]! < target) low = middle + 1
    else high = middle
  }
  const right = timestamps[low]!
  const left = timestamps[low - 1]!
  return target - left <= right - target ? left : right
}

export function scalePartitions(series: readonly RecordedSeries[]): readonly ScalePartition[] {
  const stored = new Map<string, RecordedSeries[]>()
  for (const line of series) {
    const key = scaleKey(line)
    const grouped = stored.get(key) ?? []
    grouped.push(line)
    stored.set(key, grouped)
  }
  return [...stored].map(([key, grouped]) => ({
    key,
    label: grouped.map(({ label }) => label).join(" / "),
    scale: grouped[0]!.scale,
    seriesIds: grouped.map(({ id }) => id),
    unit: grouped[0]!.unit,
  }))
}

function chartTopology(series: readonly RecordedSeries[], compact: boolean): ChartTopology {
  const grouped = new Map<string, number[]>()
  for (let index = 0; index < series.length; index += 1) {
    const line = series[index]!
    const key = scaleKey(line)
    const indices = grouped.get(key) ?? []
    indices.push(index)
    grouped.set(key, indices)
  }
  const seriesScaleKeys: string[] = Array(series.length)
  const partitions = [...grouped.values()].map((seriesIndices, index) => {
    const key = `y:${index}`
    for (const seriesIndex of seriesIndices) seriesScaleKeys[seriesIndex] = key
    return { key, scale: series[seriesIndices[0]!]!.scale, seriesIndices }
  })
  const signature = JSON.stringify({
    axes: partitions.map(({ seriesIndices }) => {
      const line = series[seriesIndices[0]!]!
      return [seriesIndices, !compact && line.unit !== "" && line.tickAxis !== "duration"]
    }),
    series: series.map(({ color, id, scale, tickAxis }) => [id, color, scale, tickAxis ?? ""]),
  })
  return { partitions, seriesScaleKeys, signature }
}

function chartPartitionRange(
  partition: ChartTopologyPartition,
  series: readonly RecordedSeries[],
  frame: ChartFrame,
): readonly [number, number] {
  const grouped = partition.seriesIndices.flatMap((seriesIndex) =>
    Array.from(frame.data[seriesIndex + 1] ?? []).filter((value): value is number => typeof value === "number"),
  )
  return scaleRange(partition.scale, grouped)
}

function chartOptions(
  topology: ChartTopology,
  runtime: { readonly current: ChartRuntimeState },
  hour: number,
  end: number,
  selected: { readonly current: number | null },
  width: number,
  height: number,
  compact: boolean,
  sharedCursor: boolean,
  markerLane: boolean,
  onHover: (chart: uPlot) => void,
  onGeometry: (chart: uPlot) => void,
): uPlot.Options {
  const { frame, series } = runtime.current
  const styles = getComputedStyle(document.documentElement)
  const color = (name: string) => styles.getPropertyValue(name).trim()
  const mono = styles.getPropertyValue("--font-mono").trim() || "ui-monospace, monospace"
  const axisFont = `${compact ? 10 : 11}px ${mono}`
  const scales = Object.fromEntries(topology.partitions.map((partition) => {
    const [min, max] = chartPartitionRange(partition, series, frame)
    const { key } = partition
    return [key, { auto: false, range: [min, max] }]
  }))
  const decorate = (chart: uPlot) => {
    const { decorations, series, threshold } = runtime.current
    const context = chart.ctx
    context.save()
    if (threshold !== undefined) {
      const seriesIndex = series.findIndex(({ id }) => id === threshold.seriesId)
      const scale = seriesIndex < 0 ? undefined : topology.seriesScaleKeys[seriesIndex]
      if (scale !== undefined) {
        const boundary = chart.valToPos(threshold.below, scale, true)
        const zero = chart.valToPos(0, scale, true)
        context.fillStyle = color("--color-chart-threshold")
        context.fillRect(chart.bbox.left, Math.min(boundary, zero), chart.bbox.width, Math.abs(zero - boundary))
      }
    }
    for (const decoration of decorations) {
      const from = chart.valToPos(Math.max(hour, decoration.from), "x", true)
      const to = chart.valToPos(Math.min(end, decoration.to), "x", true)
      context.fillStyle = color(decoration.tone === "future" ? "--color-chart-future" : decoration.tone === "selection" ? "--color-chart-selection" : "--color-chart-unavailable")
      context.fillRect(Math.min(from, to), chart.bbox.top, Math.abs(to - from), chart.bbox.height)
      if (decoration.tone === "selection") {
        // The exported interval is a selection on the plot: its edges are drawn
        // so a short range stays visible beside the cursor.
        context.fillStyle = color("--color-accent")
        context.fillRect(Math.min(from, to), chart.bbox.top, uPlot.pxRatio, chart.bbox.height)
        context.fillRect(Math.max(from, to) - uPlot.pxRatio, chart.bbox.top, uPlot.pxRatio, chart.bbox.height)
      }
    }
    context.restore()
  }
  const drawSelection = (chart: uPlot) => {
    const { frame, series } = runtime.current
    const context = chart.ctx
    const selectedTimestamp = selected.current
    context.save()
    if (selectedTimestamp !== null) {
      const x = chart.valToPos(selectedTimestamp, "x", true)
      context.strokeStyle = color("--color-cursor")
      context.setLineDash([3 * uPlot.pxRatio, 3 * uPlot.pxRatio])
      context.beginPath()
      context.moveTo(x, chart.bbox.top)
      context.lineTo(x, chart.bbox.top + chart.bbox.height)
      context.stroke()
      const dataIndex = frame.timestamps.indexOf(selectedTimestamp)
      if (dataIndex >= 0) {
        context.setLineDash([])
        for (let ordinal = 0; ordinal < series.length; ordinal += 1) {
          const number = frame.data[ordinal + 1]?.[dataIndex]
          if (typeof number !== "number") continue
          const y = chart.valToPos(number, topology.seriesScaleKeys[ordinal]!, true)
          context.fillStyle = color(chartColor(series[ordinal]!.color))
          context.beginPath()
          context.arc(x, y, 3.5 * uPlot.pxRatio, 0, Math.PI * 2)
          context.fill()
        }
      }
    }
    context.restore()
  }
  const singleSeries = series.length === 1
  const areaFill = (chart: uPlot, base: string) => {
    // Alpha suffixes only compose with 6-digit hex tokens; anything else
    // draws the plain colour instead of a broken gradient.
    if (!/^#[0-9a-fA-F]{6}$/.test(base)) return base
    const gradient = chart.ctx.createLinearGradient(0, chart.bbox.top, 0, chart.bbox.top + chart.bbox.height)
    gradient.addColorStop(0, `${base}2b`)
    gradient.addColorStop(1, `${base}00`)
    return gradient
  }
  return {
    width,
    height,
    ms: 1,
    pxAlign: true,
    legend: { show: false },
    ...(markerLane ? { padding: [MARKER_LANE_PX + 2, null, null, null] } : {}),
    scales: { x: { auto: false, range: chartTimeRange(hour, end, width, compact ? 38 : 52, topology.partitions.length * (compact ? 46 : 70)), time: false }, ...scales },
    axes: [
      { scale: "x", side: 2, size: compact ? 18 : 30, gap: compact ? 2 : 4, ticks: { size: compact ? 4 : 6, stroke: color("--color-line3") }, font: axisFont, space: (_chart, _axis, _scale, _increment, space) => Math.max(compact ? 62 : 84, space), stroke: color("--color-fg3"), grid: { show: false }, values: (_chart, splits) => splits.map((timestamp) => timestamp > end ? "" : axisTimeLabel(timestamp, runtime.current.time)) },
      ...topology.partitions.map(({ key, seriesIndices }, axisIndex) => {
        const initialLine = series[seriesIndices[0]!]!
        return { ...(!compact && initialLine.unit !== "" && initialLine.tickAxis !== "duration" ? { label: () => runtime.current.series[seriesIndices[0]!]?.unit ?? "" } : {}), font: axisFont, gap: compact ? 2 : 4, ticks: { size: compact ? 4 : 6, stroke: color("--color-line3") }, scale: key, side: axisIndex % 2 === 0 ? 3 : 1, size: compact ? 46 : 70, stroke: color("--color-fg3"), grid: { stroke: axisIndex === 0 ? color("--color-line") : "transparent" }, values: (_chart: uPlot, splits: number[]) => {
          const { locale, series: latestSeries } = runtime.current
          const line = latestSeries[seriesIndices[0]!]!
          const unit = line.unit
          // Duration axes read in one unit chosen from the range top.
          if (line.tickAxis === "duration") {
            const peak = Math.max(0, ...splits)
            return splits.map((value) => `${humanDurationAxis(value, peak, locale)}${unit}`)
          }
          return splits.map((value) => (line.tick ?? line.value)(value, locale))
        } }
      }),
    ],
    cursor: {
      show: !sharedCursor,
      dataIdx: (_chart, _seriesIndex, closestIndex) => closestIndex,
      drag: { setScale: false, x: false, y: false },
      move: (chart, left, top) => {
        const { frame } = runtime.current
        if (left < 0 || frame.timestamps.length === 0) return [left, top]
        const timestamp = nearestRecordedTimestamp(frame.timestamps, chart.posToVal(left, "x"))
        return timestamp === null ? [left, top] : [chart.valToPos(timestamp, "x"), top]
      },
      points: { show: !sharedCursor, size: 6 },
      y: false,
    },
    hooks: { draw: [drawSelection], drawClear: [decorate], ready: [onGeometry], setCursor: [onHover], setSize: [onGeometry] },
    series: [
      { label: "time" },
      ...series.map((line, index) => ({
        label: line.label,
        scale: topology.seriesScaleKeys[index]!,
        stroke: color(chartColor(line.color)),
        width: compact ? 1.5 : 2,
        ...(singleSeries ? { fill: (chart: uPlot) => areaFill(chart, color(chartColor(line.color))) } : {}),
        points: { filter: () => [...(runtime.current.frame.isolated.get(index + 1) ?? [])], show: true, size: 5 },
      })),
    ],
  }
}

function chartColor(tone: RecordedSeries["color"]): string {
  if (tone === "cyan") return "--color-series-1"
  if (tone === "amber") return "--color-series-2"
  if (tone === "violet") return "--color-series-4"
  if (tone === "green") return "--color-series-3"
  if (tone === "red") return "--color-series-5"
  if (tone === "blue") return "--color-series-6"
  if (tone === "rose") return "--color-series-7"
  return "--color-fg3"
}

export function axisTimeLabel(timestamp: number, time: Pick<DisplayTimeFormatter, "axis">): string {
  return compactTimePart(time.axis(timestamp))
}

export function compactChartTime(timestamp: number, time: Pick<DisplayTimeFormatter, "axis" | "clock">, seconds: boolean): string {
  return compactTimePart(seconds ? time.clock(timestamp) : time.axis(timestamp))
}

export function chartSecondsUseful(timestamps: readonly number[], time: Pick<DisplayTimeFormatter, "axis">): boolean {
  const minutes = new Set<string>()
  for (const timestamp of timestamps) {
    const minute = axisTimeLabel(timestamp, time)
    if (minutes.has(minute)) return true
    minutes.add(minute)
  }
  return false
}

function scaleKey(line: RecordedSeries): string {
  return `y:${line.unit || line.id}:${line.scale}`
}

export function chartSummary(series: readonly RecordedSeries[], frame: ChartFrame, hour: number, end: number, locale: Locale, time: Pick<DisplayTimeFormatter, "hourRange">): string {
  const labels = series.map((line, ordinal) => {
    const column = frame.data[ordinal + 1] ?? []
    const values = column.filter((value): value is number => typeof value === "number")
    const nulls = column.filter((value) => value === null).length
    const range = values.length === 0 ? "—" : `${line.value(Math.min(...values), locale)}…${line.value(Math.max(...values), locale)}`
    return locale === "ru"
      ? `${line.label} (${line.unit || "без единицы"}): ${values.length} записей, ${nulls} явных пустых значений, минимум…максимум ${range}`
      : `${line.label} (${line.unit || "unitless"}): ${values.length} samples, ${nulls} explicit nulls, minimum…maximum ${range}`
  }).join("; ")
  return `${time.hourRange(hour).primary}. ${labels}.`
}

export function sampleText(series: readonly RecordedSeries[], frame: ChartFrame, timestamp: number, locale: Locale, time: Pick<DisplayTimeFormatter, "axis" | "clock">): string {
  const exact = exactReadings(frame, series, timestamp, locale, time)
  if (exact === null) return ""
  return `${exact.time}; ${exact.values.map(({ label, output, unit }) => `${label}${unit === "" ? "" : ` (${unit})`}: ${output}`).join("; ")}`
}

export function navigationSampleText(
  series: readonly RecordedSeries[],
  frame: ChartFrame,
  navigationTimestamps: readonly number[],
  timestamp: number,
  locale: Locale,
  time: Pick<DisplayTimeFormatter, "axis" | "clock">,
): string {
  return sampleText(series, frame, timestamp, locale, time)
    || compactChartTime(timestamp, time, chartSecondsUseful(navigationTimestamps, time))
}

function compactTimePart(output: string): string {
  return output.trim().split(/\s+/)[0] ?? output
}
