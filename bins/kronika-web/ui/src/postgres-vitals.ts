import type { DataRow } from "./api"
import { asNumber, rawText, value } from "./model"
import { intervalMetric } from "./postgres-metrics"
import type { ChartPoint } from "./series-chart"

// Counter deltas pair adjacent rows with the same type and identity.

export interface VitalSeries {
  // Rates at snapshots with a previous reading.
  readonly points: readonly ChartPoint[]
  // Sum of recorded deltas for the hour.
  readonly total: number | null
}

const EMPTY_VITAL: VitalSeries = { points: [], total: null }

export type CounterGroups = ReadonlyMap<string, readonly DataRow[]>

export function counterGroups(rows: readonly DataRow[], identity: readonly string[] = ["datid"]): CounterGroups {
  const ordered = new Map<string, DataRow[]>()
  for (const row of [...rows].sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))) {
    const key = [row.typeId, ...identity.map((field) => rawText(value(row, field)) ?? "")].join("\u{1f}")
    const series = ordered.get(key)
    if (series === undefined) ordered.set(key, [row])
    else series.push(row)
  }
  return ordered
}

// Sums field rates across identities.
export function sumCounterVital(ordered: CounterGroups, fields: readonly string[]): VitalSeries {
  const rates = new Map<number, { segmentId: string; value: number }>()
  let total: number | null = null
  for (const series of ordered.values()) {
    for (let index = 1; index < series.length; index += 1) {
      const earlier = series[index - 1]
      const later = series[index]
      if (earlier === undefined || later === undefined || earlier.typeId !== later.typeId) continue
      for (const field of fields) {
        const rate = intervalMetric(earlier, later, field)
        if (rate === null) continue
        const slot = rates.get(later.timestamp)
        if (slot === undefined) rates.set(later.timestamp, { segmentId: later.segmentId, value: rate })
        else slot.value += rate
        total = (total ?? 0) + rate * ((later.timestamp - earlier.timestamp) / 1_000_000)
      }
    }
  }
  if (rates.size === 0) return EMPTY_VITAL
  return {
    points: [...rates.entries()]
      .sort(([left], [right]) => left - right)
      .map(([timestamp, slot]) => ({ segmentId: slot.segmentId, timestamp, value: slot.value })),
    total,
  }
}

// Reduces gauges at each snapshot. An all-null snapshot remains null.
export function gaugeSeries(rows: readonly DataRow[], field: string, reduce: "sum" | "max"): readonly ChartPoint[] {
  const moments = new Map<number, { segmentId: string; value: number | null }>()
  for (const row of rows) {
    const stored = asNumber(value(row, field))
    const slot = moments.get(row.timestamp)
    if (slot === undefined) {
      moments.set(row.timestamp, { segmentId: row.segmentId, value: stored })
      continue
    }
    if (stored === null) continue
    slot.value = slot.value === null ? stored : reduce === "sum" ? slot.value + stored : Math.max(slot.value, stored)
  }
  return [...moments.entries()]
    .sort(([left], [right]) => left - right)
    .map(([timestamp, slot]) => ({ segmentId: slot.segmentId, timestamp, value: slot.value }))
}

// Sparse sections emit no row for an empty pass. Anchor timestamps delimit
// passes so the reducer can emit an explicit zero.
export function anchoredSeries(
  rows: readonly DataRow[],
  anchors: readonly DataRow[],
  reduce: (pass: readonly DataRow[]) => number | null,
): readonly ChartPoint[] {
  const moments = new Map<number, string>()
  for (const anchor of anchors) {
    if (!moments.has(anchor.timestamp)) moments.set(anchor.timestamp, anchor.segmentId)
  }
  if (moments.size === 0) return []
  const ordered = [...moments.entries()].sort(([left], [right]) => left - right)
  const sorted = [...rows].sort((left, right) => left.timestamp - right.timestamp)
  let at = 0
  return ordered.map(([timestamp, segmentId], index) => {
    const next = ordered[index + 1]?.[0] ?? Number.POSITIVE_INFINITY
    while (at < sorted.length && (sorted[at]?.timestamp ?? Number.POSITIVE_INFINITY) <= timestamp) at += 1
    const start = at
    while (at < sorted.length && (sorted[at]?.timestamp ?? Number.POSITIVE_INFINITY) < next) at += 1
    return { segmentId, timestamp, value: reduce(sorted.slice(start, at)) }
  })
}

export function countSeries(rows: readonly DataRow[], matches: (row: DataRow) => boolean): readonly ChartPoint[] {
  const moments = new Map<number, { segmentId: string; value: number }>()
  for (const row of rows) {
    const slot = moments.get(row.timestamp)
    if (slot === undefined) moments.set(row.timestamp, { segmentId: row.segmentId, value: matches(row) ? 1 : 0 })
    else if (matches(row)) slot.value += 1
  }
  return [...moments.entries()]
    .sort(([left], [right]) => left - right)
    .map(([timestamp, slot]) => ({ segmentId: slot.segmentId, timestamp, value: slot.value }))
}

export function peakPoint(points: readonly ChartPoint[]): ChartPoint | null {
  let peak: ChartPoint | null = null
  for (const point of points) {
    if (point.value === null) continue
    if (peak === null || peak.value === null || point.value > peak.value) peak = point
  }
  return peak
}

export function lastValue(points: readonly ChartPoint[]): number | null {
  for (let index = points.length - 1; index >= 0; index -= 1) {
    const stored = points[index]
    if (stored !== undefined && stored.value !== null) return stored.value
  }
  return null
}

export function shareOfTotals(part: VitalSeries, whole: VitalSeries): number | null {
  if (part.total === null || whole.total === null || whole.total <= 0) return null
  return part.total / whole.total
}

export interface SettingChange {
  readonly timestamp: number
  readonly name: string
  readonly from: string | null
  readonly to: string | null
}

// `pg_settings` records on change; the first row for a name is the baseline.
export function settingChanges(rows: readonly DataRow[]): readonly SettingChange[] {
  const byName = new Map<string, DataRow[]>()
  for (const row of [...rows].sort((left, right) => left.timestamp - right.timestamp)) {
    const name = rawText(value(row, "name"))
    if (name === null) continue
    const series = byName.get(name)
    if (series === undefined) byName.set(name, [row])
    else series.push(row)
  }
  const changes: SettingChange[] = []
  for (const [name, series] of byName) {
    for (let index = 1; index < series.length; index += 1) {
      const earlier = series[index - 1]
      const later = series[index]
      if (earlier === undefined || later === undefined) continue
      const from = rawText(value(earlier, "setting"))
      const to = rawText(value(later, "setting"))
      if (from === to) continue
      changes.push({ timestamp: later.timestamp, name, from, to })
    }
  }
  return changes.sort((left, right) => left.timestamp - right.timestamp || left.name.localeCompare(right.name))
}

// Before the first row in the hour, use that row because the section records
// only changes.
export function settingAt(rows: readonly DataRow[], name: string, cursor: number): string | null {
  let before: DataRow | null = null
  let after: DataRow | null = null
  for (const row of rows) {
    if (rawText(value(row, "name")) !== name) continue
    if (row.timestamp <= cursor) {
      if (before === null || row.timestamp > before.timestamp) before = row
    } else if (after === null || row.timestamp < after.timestamp) {
      after = row
    }
  }
  const found = before ?? after
  return found === null ? null : rawText(value(found, "setting"))
}
