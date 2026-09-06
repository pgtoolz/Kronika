export interface MetricSample {
  readonly segmentId: string
  readonly timestamp: number
  readonly value: number | null
}

export function buildMetricSamples<Row extends { readonly segmentId: string; readonly timestamp: number }>(
  rows: readonly Row[],
  read: (row: Row) => number | null | undefined,
): readonly MetricSample[] {
  return rows.flatMap((row) => {
    const stored = read(row)
    if (stored === undefined) return []
    return [{
      segmentId: row.segmentId,
      timestamp: row.timestamp,
      value: stored === null || Number.isFinite(stored) ? stored : null,
    }]
  })
}
