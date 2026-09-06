import type { Cell } from "./api"

export const HOUR_MICROS = 3_600_000_000

export interface HeatmapInterval {
  readonly start: number
  readonly end: number
}

export const HEATMAP_STEPS = 6

// Square-root steps preserve contrast in skewed rows. Zero is distinct from null.
export function heatmapIntensity(value: number, max: number): number {
  if (value <= 0 || max <= 0) return 0
  return Math.max(1, Math.min(HEATMAP_STEPS, Math.ceil(Math.sqrt(value / max) * HEATMAP_STEPS)))
}

export interface HeatmapBand {
  readonly total: number | null
  readonly cells: readonly (number | null)[]
}

export interface HeatmapViewRow {
  readonly typeId: string
  readonly identity: readonly (string | null)[]
  readonly labels: Readonly<Record<string, Cell>>
  // Distinct identities represented by a grouped row; otherwise null.
  readonly members: number | null
  readonly total: number | null
  readonly cells: readonly (number | null)[]
}

export type HeatmapSummary = "mean" | "sum" | "max"

export interface HeatmapView {
  readonly cumulative: boolean
  readonly summary: HeatmapSummary
  readonly intervals: readonly HeatmapInterval[]
  readonly rows: readonly HeatmapViewRow[]
  readonly totals: HeatmapBand
  readonly others: HeatmapBand
  readonly othersCount: number
  readonly entityCount: number
}

// Fold rows hidden by the compact view into the Others band.
export function collapseHeatmapView(view: HeatmapView, top: number): HeatmapView {
  if (view.rows.length <= top) return view
  const kept = view.rows.slice(0, top)
  const folded = view.rows.slice(top)
  const cells = view.others.cells.map((cell, index) => {
    const contributions = [cell, ...folded.map((row) => row.cells[index] ?? null)].filter((stored): stored is number => stored !== null)
    return contributions.length === 0 ? null : contributions.reduce((sum, stored) => sum + stored, 0)
  })
  const totals = [view.others.total, ...folded.map((row) => row.total)].filter((stored): stored is number => stored !== null)
  const total = totals.length === 0
    ? null
    : view.summary === "max" ? Math.max(...totals) : totals.reduce((sum, stored) => sum + stored, 0)
  return {
    ...view,
    rows: kept,
    others: { cells, total },
    othersCount: view.othersCount + folded.length,
  }
}

export function heatmapViewMax(view: HeatmapView): number {
  let max = 0
  for (const cells of [...view.rows.map((row) => row.cells), view.others.cells]) {
    for (const cell of cells) if (cell !== null && cell > max) max = cell
  }
  return max
}
