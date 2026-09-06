import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import { fieldNameForLocator, type DataRow, type Finding, type LanePoint } from "./api"
import { buildMetricSamples } from "./chart"
import { CursorRow } from "./cursor-row"
import { mergeObservationTimestamps, observationTimestamps } from "./cursor-timestamps"
import { useDisplayTime } from "./display-time-context"
import { findingOrder, findingSummary, summarizeFindings } from "./finding-presentation"
import { useExportSelection } from "./export-context"
import { LabelHelp, type Translate } from "./help"
import { keyboardTargetOwnsArrows, moveCursor, orderedRecordedTimes } from "./keyboard"
import { asNumber, compact, humanBytes, humanCores, humanDuration, humanPercent, type Locale, value } from "./model"
import { emptyHourStatusKey } from "./refresh"
import { sampleAtOrBefore, uncollectedStart } from "./series-chart"
import { UPlotChart, type ChartDecoration, type RecordedSeries } from "./uplot-chart"

export const MARKER_CLUSTER_PX = 88

interface SeriesPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export interface GroupedFinding {
  readonly composition: readonly { readonly count: number; readonly kind: Finding["kind"] }[]
  readonly findings: readonly Finding[]
}

interface TimelineSeries {
  readonly color: "cyan" | "amber" | "violet"
  readonly field: string
  readonly points: readonly SeriesPoint[]
}

interface TimelineLane {
  readonly key: string
  readonly series: readonly TimelineSeries[]
  readonly threshold?: number | undefined
}

export type FindingShape = "circle" | "diamond" | "triangle"
export type TimelinePresentation = "preview" | "inspector"

export function Timeline({
  cursor,
  environment,
  findings,
  health,
  hour,
  lanePoints,
  locale,
  navigationTimestamps,
  onCursor,
  onFinding,
  onOpenChart,
  onPreview,
  onSelectedLane,
  primaryLane = "health",
  presentation = "preview",
  selectedLane: controlledLane,
  t,
}: {
  readonly cursor: number
  readonly environment: "machine" | "container" | null
  readonly findings: readonly Finding[]
  readonly health: readonly DataRow[]
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly navigationTimestamps?: readonly number[] | undefined
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding, grouped?: readonly Finding[]) => void
  readonly onOpenChart?: (() => void) | undefined
  readonly onPreview?: ((timestamp: number | null) => void) | undefined
  readonly onSelectedLane?: ((lane: string) => void) | undefined
  readonly primaryLane?: string | undefined
  readonly presentation?: TimelinePresentation | undefined
  readonly selectedLane?: string | undefined
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const [previewCursor, setPreviewCursor] = useState<number | null>(null)
  const displayCursor = previewCursor ?? cursor
  const preview = useCallback((timestamp: number | null) => {
    setPreviewCursor((current) => current === timestamp ? current : timestamp)
    onPreview?.(timestamp)
  }, [onPreview])
  useEffect(() => {
    setPreviewCursor(null)
    onPreview?.(null)
  }, [cursor, hour, onPreview])
  useEffect(() => () => onPreview?.(null), [onPreview])
  const end = hour + 3_600_000_000
  const healthTrack = useMemo(() => healthTimelineSeries(health), [health])
  const lanes = useMemo<readonly TimelineLane[]>(() => {
    const of = (name: string) => lanePoints
      .filter((point) => point.lane === name)
      .map((point) => ({ segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }))
    const one = (color: TimelineSeries["color"], field: string, points: readonly SeriesPoint[]): readonly [TimelineSeries] => [{ color, field, points }]
    const recorded = (name: string) => of(name).some((point) => point.value !== null)
    const lane = (color: TimelineSeries["color"], key: string): TimelineLane => ({ key, series: one(color, key, of(key)) })
    // Unknown scope cannot substitute host values for the container.
    const resources = environment === "container"
      ? [
        lane("cyan", recorded("cg_cpu_share") ? "cg_cpu_share" : "cg_cpu_cores"),
        lane("amber", "cg_cpu_psi"),
        lane("violet", recorded("cg_memory") ? "cg_memory" : "cg_memory_bytes"),
        lane("cyan", "cg_io_psi"),
      ]
      : environment === "machine"
        ? [lane("cyan", "cpu_busy"), lane("amber", "cpu_stall"), lane("violet", "memory"), lane("cyan", "io_stall")]
        : []
    return [
      { key: "health", series: healthTrack.series, threshold: healthTrack.threshold },
      ...resources,
      lane("cyan", "pg_running"),
      lane("amber", "pg_waiting"),
      { key: "oldest_xact", series: one("violet", "pg_oldest_xact", of("pg_oldest_xact")) },
    ].filter((candidate) => candidate.key === "health"
      ? candidate.series.some((line) => line.points.length !== 0)
      : candidate.series.some((line) => line.points.some((point) => point.value !== null)))
  }, [environment, healthTrack, lanePoints])
  const [localLane, setLocalLane] = useState(primaryLane)
  const selectedLane = controlledLane ?? localLane
  const setSelectedLane = (lane: string) => {
    if (controlledLane === undefined) setLocalLane(lane)
    onSelectedLane?.(lane)
  }
  const previousPrimary = useRef(primaryLane)
  useEffect(() => {
    if (previousPrimary.current === primaryLane) return
    previousPrimary.current = primaryLane
    setSelectedLane(primaryLane)
  }, [primaryLane])
  useEffect(() => {
    if (lanes.some((lane) => lane.key === selectedLane)) return
    if (controlledLane !== undefined) return
    setSelectedLane(lanes.find((lane) => lane.key === primaryLane)?.key ?? lanes[0]?.key ?? "health")
  }, [controlledLane, lanes, primaryLane, selectedLane])
  const selected = lanes.find((lane) => lane.key === selectedLane) ?? lanes[0]
  const laneTimes = useMemo(() => timelineNavigationTimes(lanes), [lanes])
  const cursorTimes = useMemo(
    () => navigationTimestamps === undefined
      ? laneTimes
      : mergeObservationTimestamps(navigationTimestamps, ...lanes.flatMap((lane) => lane.series.map((line) => line.points))),
    [laneTimes, navigationTimestamps],
  )
  const [plotWidth, setPlotWidth] = useState(920)
  const markers = useMemo(() => groupFindings(findings, hour, end, plotWidth), [end, findings, hour, plotWidth])
  useEffect(() => {
    const move = (event: KeyboardEvent) => {
      if (event.defaultPrevented || (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
        || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey
        || keyboardTargetOwnsArrows(event.target) || cursorTimes.length === 0) return
      event.preventDefault()
      const timestamp = moveCursor(cursor, cursorTimes, event.key)
      if (timestamp !== cursor) onCursor(timestamp)
    }
    window.addEventListener("keydown", move)
    return () => window.removeEventListener("keydown", move)
  }, [cursor, cursorTimes, onCursor])
  const recorded = useMemo(() => selected === undefined ? [] : toRecordedSeries(selected, locale, t), [locale, selected, t])
  const healthAt = selected?.key === "health" ? healthEvaluationAtOrBefore(selected.series, displayCursor) : null
  const current = (selected?.series ?? []).map((line) => {
    const key = selected?.key ?? "health"
    const number = key === "health" ? healthAt === null ? null : exactValue(line.points, healthAt) : sampleAtOrBefore(line.points, displayCursor)?.value ?? null
    return `${key === "health" ? `${t(`lane.health.${line.field}`)} ` : ""}${number === null ? "—" : format(number, key, locale)}`
  }).join(" · ")
  const exportSelection = useExportSelection()
  const decorations = useMemo(() => {
    const drawn = timelineDecorations(lanes, selected?.series ?? [], hour, end)
    return exportSelection === null ? drawn : [...drawn, { from: exportSelection.from, to: exportSelection.to, tone: "selection" as const }]
  }, [exportSelection, end, hour, lanes, selected])
  const threshold = useMemo(() => selected?.threshold === undefined ? undefined : { below: selected.threshold, seriesId: "overall_health" }, [selected])
  const selectedReading = selected === undefined ? "—" : laneReading(selected, displayCursor, locale, t)
  const markerLayer = <>{markers.map((marker, index) => {
    const first = marker.findings[0]
    if (first === undefined) return null
    return <FindingMarker
      key={`${first.timestamp}:${first.kind}:${index}`}
      marker={marker}
      onActivate={() => {
        if (first.timestamp !== cursor) onCursor(first.timestamp)
        onFinding(first, marker.findings)
      }}
      share={shareOf(first.timestamp, hour, end)}
      t={t}
      time={time.timestamp}
    />
  })}</>
  if (selected === undefined) {
    return findings.length === 0
      ? <section className="flex h-[124px] min-h-[124px] items-center justify-center border-y border-line2 bg-s1 text-sm text-fg4" data-presentation={presentation} data-testid="timeline-empty">{t(emptyHourStatusKey(hour))}</section>
      : <section className="flex h-[124px] min-h-[124px] items-center justify-center border-y border-line2 bg-s1 text-sm text-fg4" data-presentation={presentation} data-testid="timeline-empty">{t("status.no_data")}</section>
  }
  return <section aria-label={t("hour.range", { range: time.hourRange(hour).primary })} className={`timeline-shell mt-2 flex flex-col overflow-hidden border-y border-line2 bg-s1 timeline-${presentation}`} data-export-from={exportSelection?.from} data-export-to={exportSelection?.to} data-presentation={presentation}>
    <div className="timeline-rail flex h-7 min-w-0 flex-none overflow-hidden border-b border-line2">
      {presentation === "inspector"
        ? <label className="timeline-metric-picker"><span>{t("inspector.timeline")}</span><select aria-label={t("inspector.timeline")} data-testid="timeline-metric-select" onChange={(event) => setSelectedLane(event.currentTarget.value)} value={selected.key}>{lanes.map((lane) => <option key={lane.key} value={lane.key}>{t(`lane.${lane.key}.label`)}</option>)}</select></label>
        : <><div className="timeline-lanes flex min-w-0 flex-1 gap-0.5 overflow-hidden px-1">
          {lanes.map((lane) => <LaneLabel
            help={`lane.${lane.key}.help`}
            key={lane.key}
            label={`lane.${lane.key}.label`}
            onSelect={() => setSelectedLane(lane.key)}
            primary={lane.key === selected.key}
            reading={lane.key === selected.key ? laneReading(lane, displayCursor, locale, t) : compactLaneReading(lane, displayCursor, locale, t)}
            fullReading={laneReading(lane, displayCursor, locale, t)}
            t={t}
          />)}
        </div><div className="timeline-preview-picker min-w-0 flex-1 items-center gap-1 px-1">
          <select aria-label={t("inspector.timeline")} data-testid="timeline-preview-metric-select" onChange={(event) => setSelectedLane(event.currentTarget.value)} value={selected.key}>{lanes.map((lane) => <option key={lane.key} value={lane.key}>{t(`lane.${lane.key}.label`)}</option>)}</select>
          <span className="timeline-preview-reading min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-right text-sm tabular-nums text-fg" data-testid="timeline-preview-reading" title={selectedReading}>{selectedReading}</span>
        </div></>}
      {presentation === "preview" && onOpenChart !== undefined && <button aria-label={t("inspector.open_chart")} className="timeline-open-chart" onClick={onOpenChart} title={t("inspector.open_chart")} type="button"><span aria-hidden="true">↗</span><span>{t("inspector.chart")}</span></button>}
    </div>
    <UPlotChart
      className="timeline-chart"
      cursor={displayCursor}
      decorations={decorations}
      hour={hour}
      locale={locale}
      markerLayer={markerLayer}
      navigationTimestamps={cursorTimes}
      onCursor={onCursor}
      onPreview={preview}
      onPlotWidth={setPlotWidth}
      reading={current}
      series={recorded}
      stats={presentation === "inspector"}
      t={t}
      testId="hour-timeline"
      threshold={threshold}
      variant={presentation}
    />
    {presentation === "preview" && <CursorRow cursor={displayCursor} cursorTimes={cursorTimes} onCursor={onCursor} reading={selectedReading} t={t} />}
  </section>
}

export function timelineRecordedTimes(series: readonly { readonly points: readonly { readonly timestamp: number }[] }[]): readonly number[] {
  return orderedRecordedTimes(series.flatMap((line) => line.points.map((point) => point.timestamp)))
}

export function timelineNavigationTimes(
  lanes: readonly { readonly series: readonly { readonly points: readonly { readonly timestamp: number }[] }[] }[],
): readonly number[] {
  return observationTimestamps(...lanes.flatMap((lane) => lane.series.map((line) => line.points)))
}

export function sampleWindow(lanes: readonly { readonly series: readonly { readonly points: readonly SeriesPoint[] }[] }[]): { readonly start: number; readonly end: number } | null {
  const timestamps = lanes.flatMap((lane) => lane.series)
    .flatMap((line) => line.points)
    .flatMap((point) => typeof point.value === "number" && Number.isFinite(point.value) ? [point.timestamp] : [])
  if (timestamps.length === 0) return null
  return { start: Math.min(...timestamps), end: Math.max(...timestamps) }
}

export function timelineDecorations(
  lanes: readonly { readonly series: readonly { readonly points: readonly SeriesPoint[] }[] }[],
  selected: readonly { readonly points: readonly SeriesPoint[] }[],
  hour: number,
  end: number,
  now = Date.now() * 1_000,
): readonly ChartDecoration[] {
  const available = sampleWindow(lanes)
  const output: ChartDecoration[] = []
  if (available !== null && available.start > hour) output.push({ from: hour, to: available.start, tone: "unavailable" })
  if (available !== null && available.end < end) output.push({ from: available.end, to: end, tone: "unavailable" })
  const future = uncollectedStart(selected.flatMap((line) => line.points), hour, now)
  if (future !== null && future < end) output.push({ from: future, to: end, tone: "future" })
  return output
}

export function timelineSeriesHelpKey(lane: string, field: string): string {
  return lane === "health" ? `lane.health.${field}.help` : `lane.${lane}.help`
}

const PERCENT_LANES: ReadonlySet<string> = new Set(["health", "cpu_busy", "cpu_stall", "memory", "io_stall", "cg_cpu_share", "cg_cpu_psi", "cg_memory", "cg_io_psi"])

function laneUnit(key: string, locale: Locale): string {
  if (PERCENT_LANES.has(key)) return "%"
  if (key === "oldest_xact" || key === "cg_memory_bytes") return ""
  if (key === "cg_cpu_cores") return locale === "ru" ? "ядра" : "cores"
  return locale === "ru" ? "количество" : "count"
}

function toRecordedSeries(lane: TimelineLane, locale: Locale, t: Translate): readonly RecordedSeries[] {
  const percent = PERCENT_LANES.has(lane.key)
  const unit = laneUnit(lane.key, locale)
  return lane.series.map((line) => ({
    color: line.color,
    helpKey: timelineSeriesHelpKey(lane.key, line.field),
    id: line.field,
    label: lane.key === "health" ? t(`lane.health.${line.field}`) : t(`lane.${lane.key}.label`),
    labelKey: lane.key === "health" ? `lane.health.${line.field}` : `lane.${lane.key}.label`,
    points: line.points,
    scale: percent ? "percent" as const : "nonnegative" as const,
    tick: (number: number, place: Locale) => format(number, lane.key, place),
    unit,
    value: (number: number, place: Locale) => format(number, lane.key, place),
  }))
}

function LaneLabel({ label, help, fullReading, onSelect, primary, reading, t }: { readonly label: string; readonly help: string; readonly fullReading: string; readonly onSelect: () => void; readonly primary: boolean; readonly reading: string; readonly t: Translate }) {
  const accessible = `${t(label)}: ${fullReading}`
  return <div data-primary={primary || undefined} className={`lane-label timeline-lane-label flex h-7 min-w-0 items-center gap-1.5 overflow-hidden rounded-t-[var(--radius-xs)] px-[7px] text-left font-sans text-xs font-medium text-fg3 hover:bg-accent-soft hover:text-accent3${primary ? " bg-s3 text-fg2 shadow-[inset_0_-2px_var(--color-accent)]" : ""}`} title={accessible}>
    <button aria-label={accessible} aria-pressed={primary} className="lane-select flex min-w-0 flex-auto cursor-pointer items-center gap-1.5 self-stretch overflow-hidden border-0 bg-transparent p-0 text-left [font-family:inherit]" onClick={onSelect} type="button">
      <span className="timeline-lane-name min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{t(label)}</span>
      <span data-testid="lane-reading" className={`timeline-lane-reading ml-auto min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-right font-mono font-normal tabular-nums ${primary ? "text-md text-accent3" : "text-sm text-fg"}`} title={reading}>{reading}</span>
    </button>
    <LabelHelp helpKey={help} iconOnly labelKey={label} t={t} />
  </div>
}

export function exactValue(points: readonly SeriesPoint[], cursor: number): number | null {
  const point = points.find((candidate) => candidate.timestamp === cursor)
  return point?.value ?? null
}

export function healthEvaluationAtOrBefore(
  series: readonly { readonly points: readonly { readonly timestamp: number }[] }[],
  cursor: number,
): number | null {
  let chosen: number | null = null
  for (const line of series) for (const point of line.points) {
    if (point.timestamp <= cursor && (chosen === null || point.timestamp > chosen)) chosen = point.timestamp
  }
  return chosen
}

function format(number: number, key: string, locale: Locale): string {
  if (key === "oldest_xact") return humanDuration(number * 1_000, locale)
  if (key === "pg_running" || key === "pg_waiting") return compact(number, locale)
  if (key === "cg_cpu_cores") return humanCores(number, locale)
  if (key === "cg_memory_bytes") return humanBytes(number, locale)
  return humanPercent(number, locale)
}

// An unselected health chip has no room for the three-part split: it shows
// the overall number alone; the split stays in the accessible name and title
// and appears when the lane is selected.
export function compactLaneReading(lane: TimelineLane, cursor: number, locale: Locale, t: Translate): string {
  if (lane.key !== "health") return laneReading(lane, cursor, locale, t)
  const line = lane.series.find((candidate) => candidate.field === "overall_health") ?? lane.series[0]
  if (line === undefined) return "—"
  const healthAt = healthEvaluationAtOrBefore(lane.series, cursor)
  const number = healthAt === null ? null : exactValue(line.points, healthAt)
  return number === null ? "—" : format(number, lane.key, locale)
}

export function laneReading(lane: TimelineLane, cursor: number, locale: Locale, t: Translate): string {
  const healthAt = lane.key === "health" ? healthEvaluationAtOrBefore(lane.series, cursor) : null
  return lane.series.map((line) => {
    const number = lane.key === "health" ? healthAt === null ? null : exactValue(line.points, healthAt) : sampleAtOrBefore(line.points, cursor)?.value ?? null
    const output = number === null ? "—" : format(number, lane.key, locale)
    return lane.key === "health" ? `${t(`lane.health.${line.field}`)} ${output}` : output
  }).join(" · ")
}

export function FindingMarker({ marker, onActivate, share, t, time = String }: { readonly marker: GroupedFinding; readonly onActivate: () => void; readonly t: Translate; readonly share: number; readonly time?: (timestamp: number) => string }) {
  const activate = (event: { preventDefault(): void; stopPropagation(): void }) => {
    event.preventDefault()
    event.stopPropagation()
    onActivate()
  }
  const first = marker.findings[0]
  const last = marker.findings.at(-1)
  if (first === undefined || last === undefined) return null
  const count = marker.composition.reduce((total, item) => total + item.count, 0)
  const displayKind = marker.composition[0]?.kind ?? first.kind
  const kindSummary = findingSummary(marker.findings, t)
  const timeSummary = first.timestamp === last.timestamp ? time(first.timestamp) : `${time(first.timestamp)}–${time(last.timestamp)}`
  return <button
    aria-label={`${kindSummary} · ${timeSummary}`}
    className={`marker-button pointer-events-auto absolute top-1/2 z-[2] flex h-[18px] min-w-[18px] -translate-x-1/2 -translate-y-1/2 cursor-pointer items-center justify-center overflow-visible border-0 bg-transparent p-0 [&>svg]:[filter:drop-shadow(0_1px_2px_var(--color-shadow))] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-cursor${count === 1 ? ` marker-${displayKind}` : " marker-aggregate"}`}
    data-marker-composition={marker.composition.map(({ count, kind }) => `${kind}:${count}`).join(" ")}
    data-marker-count={count}
    data-marker-locator-count={marker.findings.length}
    data-marker-kinds={marker.composition.map(({ kind }) => kind).join(" ")}
    onClick={activate}
    onKeyDown={(event) => {
      event.stopPropagation()
      if (event.key === "Enter" || event.key === " ") activate(event)
    }}
    onPointerDown={(event) => event.stopPropagation()}
    style={{ left: `clamp(${MARKER_CLUSTER_PX / 2}px, ${share * 100}%, calc(100% - ${MARKER_CLUSTER_PX / 2}px))` }}
    type="button"
  >
    {count === 1
      ? <FindingGlyph kind={displayKind} />
      : <span aria-hidden="true" className="marker-cluster-badge box-border flex h-4 items-center gap-1 rounded-full border border-line3 bg-s2/95 pl-1.5 pr-1.5 shadow-[0_1px_3px_var(--color-shadow)]">
        <span className="flex items-center gap-1 [&_svg]:h-2 [&_svg]:w-2">{marker.composition.map(({ count: kindCount, kind }) => <span className="flex items-center gap-0.5" key={kind}>
          <FindingGlyph kind={kind} />
          <strong className="font-sans text-[10px] font-semibold leading-none tabular-nums text-fg">{markerCount(kindCount)}</strong>
        </span>)}</span>
      </span>}
  </button>
}

// Counts above 999 stop informing at marker size.
function markerCount(count: number): string {
  return count > 999 ? "999+" : String(count)
}

function series(rows: readonly DataRow[], field: string): readonly SeriesPoint[] {
  return preferredSeries(rows, [field])
}

function preferredSeries(rows: readonly DataRow[], fields: readonly string[]): readonly SeriesPoint[] {
  return buildMetricSamples(rows, (row) => {
    const field = fields.find((candidate) => Object.hasOwn(row.values, candidate))
    return field === undefined ? undefined : asNumber(value(row, field))
  })
}

export function healthTimelineSeries(rows: readonly DataRow[]): { readonly series: readonly TimelineSeries[]; readonly threshold?: number } {
  const candidates: readonly TimelineSeries[] = [
    { color: "cyan", field: "overall_health", points: series(rows, "overall_health") },
    { color: "amber", field: "os_health", points: series(rows, "os_health") },
    { color: "violet", field: "postgres_health", points: series(rows, "postgres_health") },
  ]
  const shown = candidates.filter((candidate) => candidate.points.some((point) => point.value !== null))
  return { series: shown, ...(shown.some((candidate) => candidate.field === "overall_health") ? { threshold: 50 } : {}) }
}

export function groupFindings(findings: readonly Finding[], hour: number, end: number, pixelWidth: number, clusterWidth = MARKER_CLUSTER_PX): readonly GroupedFinding[] {
  const duration = Math.max(1, end - hour)
  const width = Math.max(1, pixelWidth)
  const ordered = findings.slice().sort(findingOrder)
  const stored: Finding[][] = []
  let active: Finding[] = []
  let anchor = 0
  for (const finding of ordered) {
    const edge = Math.min(width / 2, clusterWidth / 2)
    const x = Math.max(edge, Math.min(width - edge, (finding.timestamp - hour) / duration * width))
    if (active.length === 0 || x - anchor <= clusterWidth) {
      if (active.length === 0) anchor = x
      active.push(finding)
    } else {
      stored.push(active)
      active = [finding]
      anchor = x
    }
  }
  if (active.length !== 0) stored.push(active)
  return stored.map((group) => {
    const summary = summarizeFindings(group)
    const counts: Readonly<Record<Finding["kind"], number>> = {
      event: summary.event,
      known_bad: summary.knownBad,
      spike: summary.spike,
    }
    return {
      composition: FINDING_KINDS.flatMap((kind) => counts[kind] === 0 ? [] : [{ count: counts[kind], kind }]),
      findings: group,
    }
  })
}

const FINDING_KINDS = ["event", "known_bad", "spike"] as const satisfies readonly Finding["kind"][]

export function findingTrack(finding: Finding): string | null {
  if (finding.kind === "event") return null
  const field = fieldNameForLocator(finding)
  if (finding.logicalName === "health" && field === "overall_health") return "health"
  if (finding.logicalName === "os_meminfo" && field === "mem_available") return "memory"
  return null
}

export function findingShape(kind: Finding["kind"]): FindingShape {
  if (kind === "known_bad") return "diamond"
  if (kind === "spike") return "triangle"
  return "circle"
}

function FindingGlyph({ kind }: { readonly kind: Finding["kind"] }) {
  if (kind === "known_bad") return <svg aria-hidden="true" data-marker-shape="diamond" height="11" viewBox="0 0 12 12" width="11"><path d="M6 1 11 6 6 11 1 6Z" fill="var(--color-bad)" stroke="var(--color-bad-edge)" /></svg>
  if (kind === "spike") return <svg aria-hidden="true" data-marker-shape="triangle" height="11" viewBox="0 0 12 12" width="12"><path d="M6 1 11 10.5H1Z" fill="none" stroke="var(--color-warn)" strokeWidth="1.5" /></svg>
  return <svg aria-hidden="true" data-marker-shape="circle" height="10" viewBox="0 0 12 12" width="10"><circle cx="6" cy="6" fill="var(--color-event)" r="4.5" stroke="var(--color-event-edge)" /></svg>
}

export function healthThreshold(field: string): number | null {
  return field === "overall_health" ? 50 : null
}

function shareOf(timestamp: number, hour: number, end: number): number {
  return Math.max(0, Math.min(1, (timestamp - hour) / (end - hour)))
}
