import { ChevronDown, ChevronRight } from "lucide-react"
import { useMemo, type ReactNode } from "react"

import type { LanePoint } from "./api"
import { LabelHelp, type Translate } from "./help"
import { humanBytes, humanPercent, measure, type Locale } from "./model"
import { readingAt, type ChartPoint } from "./series-chart"
import { SparkCell } from "./spark-cell"
import { sparkScaleMax } from "./spark"

export type UseResourceKey = "cpu" | "memory" | "disk" | "network" | "cgroup_cpu" | "cgroup_memory" | "cgroup_io" | "cgroup_pids"
export type LedgerKey = UseResourceKey

export interface UseCell {
  readonly lane: string
  readonly kind: "share" | "rate" | "count" | "bytes"
  readonly metric: string
  readonly second?: string
  // A share needs a recorded limit. Without one the cell shows the plain
  // measurement instead of staying an inert dash.
  readonly fallback?: UseCell
}

export interface UseResource {
  readonly key: UseResourceKey
  readonly utilisation: UseCell | null
  readonly saturation: UseCell | null
  readonly errors: UseCell | null
}

// The container rows describe the collector's own cgroup, the scope the
// recording runs in. Their lanes come from `os_cgroup_*` rows selected by the
// exact membership paths and from the pressure of that cgroup; a host CPU count
// or host `/proc` value never stands in for them.
export const USE_RESOURCES: readonly UseResource[] = [
  {
    key: "cgroup_cpu",
    utilisation: {
      lane: "cg_cpu_share", kind: "share", metric: "cgroup_used_cores",
      fallback: { lane: "cg_cpu_cores", kind: "count", metric: "cgroup_used_cores" },
    },
    saturation: { lane: "cg_cpu_throttle", kind: "share", metric: "cgroup_used_cores", second: "cg_cpu_psi" },
    errors: null,
  },
  {
    key: "cgroup_memory",
    utilisation: {
      lane: "cg_memory", kind: "share", metric: "current",
      fallback: { lane: "cg_memory_bytes", kind: "bytes", metric: "current" },
    },
    saturation: { lane: "cg_mem_psi", kind: "share", metric: "current" },
    errors: { lane: "cg_oom", kind: "rate", metric: "current" },
  },
  {
    key: "cgroup_io",
    utilisation: { lane: "cg_io_read", kind: "bytes", metric: "rbytes", second: "cg_io_write" },
    saturation: { lane: "cg_io_psi", kind: "share", metric: "rbytes" },
    errors: null,
  },
  {
    key: "cgroup_pids",
    utilisation: {
      lane: "cg_pids_share", kind: "share", metric: "tasks_current",
      fallback: { lane: "cg_pids", kind: "count", metric: "tasks_current" },
    },
    saturation: null,
    errors: null,
  },
  {
    key: "cpu",
    utilisation: { lane: "cpu_busy", kind: "share", metric: "cpu_busy" },
    saturation: { lane: "cpu_stall", kind: "share", metric: "cpu_stall" },
    errors: null,
  },
  {
    key: "memory",
    utilisation: { lane: "memory", kind: "share", metric: "memory" },
    saturation: { lane: "mem_swap", kind: "rate", metric: "mem_swap" },
    errors: { lane: "mem_oom", kind: "rate", metric: "oom_kill" },
  },
  {
    key: "disk",
    utilisation: { lane: "disk_busy", kind: "share", metric: "disk_busy" },
    saturation: { lane: "disk_queue", kind: "count", metric: "disk_queue" },
    errors: null,
  },
  {
    key: "network",
    utilisation: { lane: "net_rx", kind: "bytes", metric: "network_rx", second: "net_tx" },
    saturation: { lane: "net_drop", kind: "rate", metric: "net_drop" },
    errors: { lane: "net_errors", kind: "rate", metric: "network_errors" },
  },
]

export const USE_COLUMNS = ["utilisation", "saturation", "errors"] as const
export type UseColumn = typeof USE_COLUMNS[number]

export const CONTAINER_RESOURCES: ReadonlySet<UseResourceKey> = new Set(["cgroup_cpu", "cgroup_memory", "cgroup_io", "cgroup_pids"])

export function isContainerResource(key: UseResourceKey): boolean {
  return CONTAINER_RESOURCES.has(key)
}

export function UseTable({
  containerScopes = false,
  cursor,
  expanded,
  hour,
  lanePoints,
  locale,
  metric,
  onCellSelect,
  onOpenRow,
  onToggle,
  renderExpansion,
  visibleResources,
  withContent,
  t,
}: {
  readonly containerScopes?: boolean | undefined
  readonly cursor: number
  readonly expanded: ReadonlySet<LedgerKey>
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly locale: Locale
  readonly metric: string | null
  readonly onCellSelect: (key: UseResourceKey, metric: string) => void
  readonly onOpenRow: (key: UseResourceKey) => void
  readonly onToggle: (key: LedgerKey) => void
  readonly renderExpansion: (key: LedgerKey) => ReactNode
  readonly visibleResources?: ReadonlySet<UseResourceKey> | undefined
  readonly withContent: ReadonlySet<UseResourceKey>
  readonly t: Translate
}) {
  // A row exists when the hour carries anything for its resource: lane
  // readings, group metrics or entity tables. Cells without a lane stay "—".
  const byLane = useMemo(() => lanePointsByLane(lanePoints), [lanePoints])
  const shown = useMemo(() => USE_RESOURCES.filter((resource) =>
    (containerScopes || !isContainerResource(resource.key))
    && (visibleResources === undefined || visibleResources.has(resource.key))
    && (withContent.has(resource.key) || USE_COLUMNS.some((column) => resolveCell(resource, column, byLane) !== null))), [byLane, containerScopes, visibleResources, withContent])
  const resourceLabel = (key: UseResourceKey) => t(containerScopes && key === "network" ? "use.resource.namespace_network" : `use.resource.${key}`)
  const verdicts = useMemo(() => ledgerVerdicts(shown, byLane, cursor, locale, t, resourceLabel, containerScopes), [byLane, containerScopes, cursor, locale, shown, t, resourceLabel])
  const end = hour + 3_600_000_000
  if (shown.length === 0) return null
  const resourceRow = (resource: UseResource) => {
    const open = expanded.has(resource.key)
    return <div data-testid={`use-group-${resource.key}`} key={resource.key}>
      <div className="use-row" data-expanded={open || undefined} data-testid={`use-row-${resource.key}`}>
        <button aria-expanded={open} className="use-resource flex cursor-pointer items-center gap-1 self-stretch border-0 bg-transparent px-2 py-[7px] text-left font-sans text-sm font-medium text-fg2 hover:bg-s3 coarse:min-h-11 max-[760px]:px-[5px]" data-testid={`use-toggle-${resource.key}`} onClick={() => onToggle(resource.key)} type="button">
          {open ? <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} /> : <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />}
          {resourceLabel(resource.key)}
        </button>
        {USE_COLUMNS.map((column) => {
          const resolved = resolveCell(resource, column, byLane)
          if (resolved === null) {
            return <span className="use-cell relative flex min-w-0 items-center justify-center text-sm text-fg4" data-testid={`use-empty-${resource.key}-${column}`} key={column} title={t("use.not_measured")}>—</span>
          }
          const { cell, points, second } = resolved
          const primary = seriesReading(points, cursor, locale, cell.kind, t("unit.per_second"))
          const secondary = second === undefined ? null : seriesReading(second, cursor, locale, cell.kind, t("unit.per_second"))
          const laneLabels = [t(`use.lane.${cell.lane}`), ...(cell.second === undefined ? [] : [t(`use.lane.${cell.second}`)])]
          const readings = [primary, ...(secondary === null ? [] : [secondary])]
          const accessibleName = `${resourceLabel(resource.key)} · ${t(`use.${column}`)} · ${laneLabels.map((label, index) => `${label}: ${readings[index]}`).join(" · ")}`
          return <span className="use-cell relative min-w-0 px-2 py-1.5 coarse:min-h-11 max-[760px]:px-[5px]" key={column}>
            <button
              aria-label={accessibleName}
              aria-pressed={metric === cell.metric}
              className="use-cell-action absolute inset-0 z-0 cursor-pointer border-0 bg-transparent"
              data-testid={`use-cell-${resource.key}-${column}`}
              onClick={(event) => { event.stopPropagation(); onCellSelect(resource.key, cell.metric) }}
              title={accessibleName}
              type="button"
            />
            <span className={`use-lane pointer-events-none relative z-[1] flex items-baseline justify-start gap-[7px] max-[760px]:flex-wrap max-[760px]:gap-x-[5px] max-[760px]:gap-y-0.5 max-[760px]:[&>span]:flex-none [&>span]:min-w-0 [&>span]:font-sans [&>span]:text-sm [&>span]:text-fg3 [&_strong]:ml-auto [&_strong]:flex-none [&_strong]:whitespace-nowrap [&_strong]:font-mono [&_strong]:text-sm [&_strong]:font-normal [&_strong]:tabular-nums [&_strong]:text-fg2 max-[760px]:[&_strong]:ml-0 max-[760px]:[&_strong]:w-full max-[760px]:[&_strong]:whitespace-normal max-[760px]:[&_strong]:leading-tight${second === undefined ? "" : " max-[900px]:flex-wrap max-[900px]:gap-y-0.5 max-[900px]:[&>span]:flex-none max-[900px]:[&_strong]:ml-0 max-[900px]:[&_strong]:w-full"}`}>
              <span>{laneLabels.join(" · ")}</span>
              <strong>{readings.join(" · ")}</strong>
            </span>
            <span className="pointer-events-none relative z-[1]"><SparkCell cursor={cursor} end={end} hour={hour} max={sparkScaleMax(cell.kind, [points, ...(second === undefined ? [] : [second])])} points={points} second={second} /></span>
          </span>
        })}
      </div>
      {open && <div className="use-expansion border-b border-line2 bg-s1" data-testid={`use-expansion-${resource.key}`}>{renderExpansion(resource.key)}</div>}
    </div>
  }
  const scope = (key: "container" | "namespace" | "host") => <h2 className="use-scope" data-testid={`use-scope-${key}`}>{t(`use.scope.${key}`)}</h2>
  const container = shown.filter(({ key }) => isContainerResource(key))
  const network = shown.filter(({ key }) => key === "network")
  const host = shown.filter(({ key }) => !isContainerResource(key) && key !== "network")
  return <section aria-label={t("use.title")} className="use-table" data-testid="use-table">
    <header className="grid grid-cols-[minmax(96px,130px)_repeat(3,minmax(0,1fr))] border-b border-line2 text-sm font-medium text-fg3 [&>span]:px-2 [&>span]:py-[5px] max-[760px]:grid-cols-[80px_repeat(3,minmax(0,1fr))] max-[760px]:[&>span]:px-[5px]">
      <span>{t("use.resource")}</span>
      {USE_COLUMNS.map((column) => {
        const verdict = verdicts[column]
        const verdictClass = "use-verdict block min-w-0 max-w-full overflow-hidden text-ellipsis whitespace-nowrap p-0 text-left font-mono text-sm font-normal tabular-nums text-fg2"
        return <span className="use-header-cell flex min-w-0 flex-col items-start gap-y-0.5" key={column}>
          <LabelHelp helpKey={`use.${column}.help`} labelKey={`use.${column}`} t={t} />
          {verdict.key === null
            ? <span className={verdictClass} data-testid={`use-verdict-${column}`} title={verdict.text}>{verdict.text}</span>
            : <button
              aria-label={t("use.verdict.open", { resource: resourceLabel(verdict.key), verdict: verdict.text })}
              className={`${verdictClass} cursor-pointer border-0 bg-transparent hover:text-fg focus-visible:outline-2 focus-visible:outline-accent coarse:min-h-11 coarse:min-w-11`}
              data-testid={`use-verdict-${column}`}
              onClick={() => onOpenRow(verdict.key as UseResourceKey)}
              title={verdict.text}
              type="button"
            >{verdict.text}</button>}
        </span>
      })}
    </header>
    {containerScopes ? <>
      {container.length > 0 && <>{scope("container")}{container.map(resourceRow)}</>}
      {network.length > 0 && <>{scope("namespace")}{network.map(resourceRow)}</>}
      {host.length > 0 && <>{scope("host")}{host.map(resourceRow)}</>}
    </> : shown.map(resourceRow)}
  </section>
}

export interface ResolvedCell {
  readonly cell: UseCell
  readonly points: readonly ChartPoint[]
  readonly second: readonly ChartPoint[] | undefined
}

export function resolveCell(resource: UseResource, column: UseColumn, byLane: ReadonlyMap<string, readonly ChartPoint[]>): ResolvedCell | null {
  let cell: UseCell | undefined = resource[column] ?? undefined
  while (cell !== undefined) {
    const points = byLane.get(cell.lane) ?? []
    if (seriesHasReading(points)) {
      return { cell, points, second: cell.second === undefined ? undefined : byLane.get(cell.second) ?? [] }
    }
    cell = cell.fallback
  }
  return null
}

export interface LedgerVerdict {
  // The row the verdict points at; null when nothing measured contributes.
  readonly key: UseResourceKey | null
  readonly text: string
}

export function ledgerVerdicts(
  rows: readonly UseResource[],
  byLane: ReadonlyMap<string, readonly ChartPoint[]>,
  cursor: number,
  locale: Locale,
  t: Translate,
  resourceLabel: (key: UseResourceKey) => string,
  containerScopes = false,
): Readonly<Record<UseColumn, LedgerVerdict>> {
  const perSecond = t("unit.per_second")
  let utilisation: LedgerVerdict = { key: null, text: "—" }
  let peak = -1
  for (const resource of rows) {
    const resolved = resolveCell(resource, "utilisation", byLane)
    if (resolved === null || resolved.cell.kind !== "share") continue
    const value = readingAt(resolved.points, cursor)
    if (value !== null && Number.isFinite(value) && value > peak) {
      peak = value
      utilisation = { key: resource.key, text: `${humanPercent(value, locale)} · ${resourceLabel(resource.key)}` }
    }
  }

  const pressures: { key: UseResourceKey; share: number; text: string }[] = []
  let measuredSaturation = false
  for (const resource of rows) {
    const resolved = resolveCell(resource, "saturation", byLane)
    if (resolved === null) continue
    measuredSaturation = true
    const lanes: (readonly [string, readonly ChartPoint[]])[] = [[resolved.cell.lane, resolved.points]]
    if (resolved.cell.second !== undefined && resolved.second !== undefined) lanes.push([resolved.cell.second, resolved.second])
    for (const [lane, points] of lanes) {
      const max = seriesMax(points)
      if (max === null || max <= 0) continue
      const scope = isContainerResource(resource.key) ? "container" : containerScopes && resource.key === "network" ? "namespace" : "host"
      pressures.push({
        key: resource.key,
        share: resolved.cell.kind === "share" ? max : -1,
        text: `${t(`use.scope.${scope}`)} · ${t(`use.lane.${lane}`)} ${reading(max, locale, resolved.cell.kind, perSecond)}`,
      })
    }
  }
  const worstPressure = pressures.reduce<{ key: UseResourceKey; share: number; text: string } | null>((worst, candidate) =>
    worst === null || candidate.share > worst.share ? candidate : worst, null)
  const saturation: LedgerVerdict = !measuredSaturation
    ? { key: null, text: "—" }
    : worstPressure === null
      ? { key: null, text: t("use.verdict.quiet") }
      : { key: worstPressure.key, text: pressures.map(({ text }) => text).join(" · ") }

  let events = 0
  let measuredErrors = false
  let worstErrors: { key: UseResourceKey; events: number } | null = null
  for (const resource of rows) {
    const resolved = resolveCell(resource, "errors", byLane)
    if (resolved === null || resolved.cell.kind !== "rate") continue
    measuredErrors = true
    const total = integrateRate(resolved.points) + (resolved.second === undefined ? 0 : integrateRate(resolved.second))
    events += total
    if (total > 0 && (worstErrors === null || total > worstErrors.events)) worstErrors = { key: resource.key, events: total }
  }
  const errors: LedgerVerdict = !measuredErrors
    ? { key: null, text: "—" }
    : { key: worstErrors?.key ?? null, text: events > 0 ? t("use.verdict.events", { count: measure(Math.round(events), locale) }) : t("use.verdict.quiet") }

  return { utilisation, saturation, errors }
}

export function seriesMax(points: readonly ChartPoint[]): number | null {
  let max: number | null = null
  for (const point of points) {
    if (point.value === null || !Number.isFinite(point.value)) continue
    if (max === null || point.value > max) max = point.value
  }
  return max
}

// A rate lane carries per-second events for the interval that ends at each
// sample; the hour's count is each rate times its own interval.
export function integrateRate(points: readonly ChartPoint[]): number {
  const ordered = [...points].sort((left, right) => left.timestamp - right.timestamp)
  let total = 0
  for (let index = 1; index < ordered.length; index++) {
    const point = ordered[index] as ChartPoint
    const before = ordered[index - 1] as ChartPoint
    if (point.value === null || !Number.isFinite(point.value)) continue
    total += point.value * (point.timestamp - before.timestamp) / 1_000_000
  }
  return total
}

export function lanePointsByLane(lanePoints: readonly LanePoint[]): ReadonlyMap<string, readonly ChartPoint[]> {
  const map = new Map<string, ChartPoint[]>()
  for (const point of lanePoints) {
    const stored = map.get(point.lane)
    const entry = { segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }
    if (stored === undefined) map.set(point.lane, [entry])
    else stored.push(entry)
  }
  return map
}

export function seriesHasReading(points: readonly ChartPoint[]): boolean {
  return points.some((point) => point.value !== null && Number.isFinite(point.value))
}

export function seriesReading(
  points: readonly ChartPoint[],
  cursor: number,
  locale: Locale,
  kind: UseCell["kind"],
  perSecond: string,
): string {
  const stored = readingAt(points, cursor)
  return stored === null ? "—" : reading(stored, locale, kind, perSecond)
}

export function reading(value: number, locale: Locale, kind: UseCell["kind"], perSecond: string): string {
  if (kind === "share") return humanPercent(value, locale)
  if (kind === "bytes") return humanBytes(value, locale, perSecond)
  if (kind === "count") return measure(value, locale)
  return measure(value, locale, perSecond)
}
