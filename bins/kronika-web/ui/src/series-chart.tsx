import { useMemo, useRef } from "react"

import { compact, floorHour, humanPercent, type Locale } from "./model"
import { LabelHelp, type Translate } from "./help"
import type { HistoryStatus } from "./history-request"
import { UPlotChart, type ChartScale as SemanticScale, type RecordedSeries } from "./uplot-chart"

export interface ChartPoint {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export interface NumericChartPoint extends ChartPoint {
  readonly value: number
}

export type ChartScale = "auto" | "percent" | "count" | "duration" | "nonnegative" | "signed"

export function SeriesChart({
  cursor,
  durationAxis = false,
  empty,
  hour,
  helpKey,
  labelKey,
  locale,
  format,
  points,
  scale = "nonnegative",
  second,
  secondHelpKey,
  secondLabelKey,
  status = "ready",
  stats = false,
  t,
  tickFormat,
  unit = "",
  onCursor,
}: {
  readonly cursor?: number | undefined
  readonly durationAxis?: boolean | undefined
  readonly empty?: string | undefined
  readonly hour: number
  readonly helpKey: string
  readonly labelKey: string
  readonly locale: Locale
  readonly format?: ((value: number, locale: Locale) => string) | undefined
  readonly points: readonly ChartPoint[]
  readonly scale?: ChartScale | undefined
  readonly second?: readonly ChartPoint[] | undefined
  readonly secondHelpKey?: string | undefined
  readonly secondLabelKey?: string | undefined
  readonly status?: HistoryStatus | undefined
  readonly stats?: boolean | undefined
  readonly t: Translate
  readonly tickFormat?: ((value: number, locale: Locale) => string) | undefined
  readonly unit?: string | undefined
  readonly onCursor?: ((timestamp: number) => void) | undefined
}) {
  const visible = useMemo(() => ({
    points: pointsInHour(points, hour),
    second: second === undefined ? undefined : pointsInHour(second, hour),
  }), [hour, points, second])
  const numeric = numericChartPoints(visible.points, visible.second)
  const reading = readingAt(visible.points, cursor)
  const hasData = numeric.length !== 0
  const formatValue = format ?? (scale === "percent" ? humanPercent : compact)
  const label = t(labelKey)
  const formatter = useRef(formatValue)
  formatter.current = formatValue
  const stableFormat = useMemo(() => (number: number, place: Locale) => formatter.current(number, place), [])
  const tickFormatter = useRef(tickFormat ?? formatValue)
  tickFormatter.current = tickFormat ?? formatValue
  const stableTickFormat = useMemo(() => (number: number, place: Locale) => tickFormatter.current(number, place), [])
  const semantic: SemanticScale = scale === "percent" ? "percent" : scale === "auto" || scale === "signed" ? "signed" : "nonnegative"
  const series = useMemo<readonly RecordedSeries[]>(() => [
    { color: "cyan", helpKey, id: "primary", label, labelKey, points: visible.points, scale: semantic, tick: stableTickFormat, tickAxis: durationAxis ? "duration" : undefined, unit, value: stableFormat },
    ...(visible.second === undefined || secondHelpKey === undefined || secondLabelKey === undefined ? [] : [{ color: "amber" as const, helpKey: secondHelpKey, id: "secondary", label: t(secondLabelKey), labelKey: secondLabelKey, points: visible.second, scale: semantic, tick: stableTickFormat, tickAxis: (durationAxis ? "duration" : undefined) as "duration" | undefined, unit, value: stableFormat }]),
  ], [durationAxis, helpKey, label, labelKey, secondHelpKey, secondLabelKey, semantic, stableFormat, stableTickFormat, t, unit, visible])
  const statusText = status === "loading"
    ? t("history.loading")
    : status === "error" ? t("history.error") : empty ?? t("history.empty")
  const statusLine = <p data-status={status} data-testid="series-status" className={`flex min-h-[30px] items-center px-0 py-[3px] text-sm ${status === "error" ? "text-warn" : "text-fg4"}`} role={status === "error" ? "alert" : "status"}>{statusText}</p>
  // An hour that carries no samples stays compact. Anything still resolving keeps the
  // full frame so the page below does not move while a metric loads.
  if (!hasData && status === "ready") {
    return <div className="series-chart [.process-history_&]:mt-0 [.process-history_&]:min-w-0 [.process-history_&]:border [.process-history_&]:border-line2 [.process-history_&]:px-[7px] [.process-history_&]:pb-1 [.process-history_&]:pt-1.5 [.system-entity-history_&]:pt-1">
      <div className="series-reading mb-[5px] flex items-baseline gap-2 font-sans text-xs font-medium text-fg3 [&>.label-help]:min-w-0 [&>span:last-child]:flex-none [&>span:last-child]:whitespace-nowrap [&>span:last-child]:font-mono [&>span:last-child]:font-normal [&>span:last-child]:tabular-nums [&>span:last-child]:text-fg2"><LabelHelp helpKey={helpKey} labelKey={labelKey} t={t} /><span>—</span></div>
      {statusLine}
    </div>
  }
  return <div className="series-chart [.process-history_&]:mt-0 [.process-history_&]:min-w-0 [.process-history_&]:border [.process-history_&]:border-line2 [.process-history_&]:px-[7px] [.process-history_&]:pb-1 [.process-history_&]:pt-1.5 [.system-entity-history_&]:pt-1">
    <UPlotChart
      cursor={cursor}
      hour={hour}
      locale={locale}
      onCursor={onCursor}
      reading={reading === null ? "—" : formatValue(reading, locale)}
      series={series}
      stats={stats}
      status={status === "ready" ? undefined : statusLine}
      t={t}
    />
  </div>
}

export function pointsInHour(points: readonly ChartPoint[], hour: number): readonly ChartPoint[] {
  const end = hour + 3_600_000_000
  return points.filter(({ timestamp }) => timestamp >= hour && timestamp < end)
}

export function numericChartPoints(
  points: readonly ChartPoint[],
  second?: readonly ChartPoint[] | undefined,
): readonly NumericChartPoint[] {
  const both = second === undefined ? points : [...points, ...second]
  return both.filter((point): point is NumericChartPoint => point.value !== null && Number.isFinite(point.value))
}

export function readingAt(points: readonly ChartPoint[], cursor: number | undefined): number | null {
  return sampleAtOrBefore(points, cursor)?.value ?? null
}

export function sampleAtOrBefore(points: readonly ChartPoint[], cursor: number | undefined): ChartPoint | null {
  let chosen: ChartPoint | null = null
  for (const point of points) {
    if (cursor !== undefined && point.timestamp > cursor) continue
    if (chosen === null || point.timestamp >= chosen.timestamp) chosen = point
  }
  return chosen
}

export function uncollectedStart(points: readonly ChartPoint[], hour: number, now = Date.now() * 1_000): number | null {
  if (floorHour(now) !== hour) return null
  const end = hour + 3_600_000_000
  let latest = hour
  for (const point of points) {
    if (point.timestamp >= hour && point.timestamp < end) latest = Math.max(latest, point.timestamp)
  }
  return latest
}
