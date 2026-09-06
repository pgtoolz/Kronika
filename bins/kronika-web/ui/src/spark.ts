import type { ChartPoint } from "./series-chart"

// Pure geometry for the ledger sparklines, kept JSX-free for direct unit
// testing. Missing intervals break the line and isolated samples become dots —
// the same honesty rules the large charts follow.
export const SPARK_WIDTH = 240
export const SPARK_HEIGHT = 28
export const SPARK_PAD = 2

export interface SparkGeometry {
  readonly dots: readonly string[]
  readonly path: string
}

export function niceCeiling(value: number): number {
  if (!Number.isFinite(value) || value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  return (normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10) * magnitude
}

export function sparkScaleMax(kind: "share" | "rate" | "count" | "bytes", series: readonly (readonly ChartPoint[])[]): number {
  if (kind === "share") return 100
  const peak = Math.max(0, ...series.flat().flatMap((point) => point.value !== null && Number.isFinite(point.value) ? [point.value] : []))
  return niceCeiling(peak)
}

export function sparkGeometry(points: readonly ChartPoint[], hour: number, end: number, max: number): SparkGeometry {
  const ordered = [...points].sort((left, right) => left.timestamp - right.timestamp)
  const x = (timestamp: number) => ((timestamp - hour) / (end - hour)) * SPARK_WIDTH
  const y = (value: number) => SPARK_HEIGHT - SPARK_PAD - Math.max(0, Math.min(1, value / Math.max(1e-9, max))) * (SPARK_HEIGHT - 2 * SPARK_PAD)
  const segments: string[][] = []
  const dots: string[] = []
  let active: string[] = []
  for (const point of ordered) {
    if (point.value === null || !Number.isFinite(point.value)) {
      if (active.length !== 0) segments.push(active)
      active = []
      continue
    }
    active.push(`${x(point.timestamp).toFixed(2)} ${y(point.value).toFixed(2)}`)
  }
  if (active.length !== 0) segments.push(active)
  const path = segments.flatMap((segment) => {
    if (segment.length === 1) {
      dots.push(segment[0]!)
      return []
    }
    return [`M${segment[0]} L${segment.slice(1).join(" ")}`]
  }).join(" ")
  return { dots, path }
}

export function sparkCursorX(cursor: number, hour: number, end: number): number | null {
  if (cursor < hour || cursor >= end) return null
  return ((cursor - hour) / (end - hour)) * SPARK_WIDTH
}
