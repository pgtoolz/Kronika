import type { DataRow } from "./api"
import { buildMetricSamples } from "./chart"
import { asNumber, rawText, value } from "./model"
import type { ChartPoint } from "./series-chart"

export type ActivityDurationField = "backend_age_ms" | "query_duration_ms" | "state_duration_ms" | "transaction_duration_ms"

export function backendAgeMs(row: DataRow): number | null {
  return elapsedSince(row, "backend_start")
}

export function activityDurationMs(row: DataRow): number | null {
  if (rawText(value(row, "state")) !== "active") return null
  return elapsedSince(row, "query_start")
}

export function transactionDurationMs(row: DataRow): number | null {
  return elapsedSince(row, "xact_start")
}

export function stateDurationMs(row: DataRow): number | null {
  if (rawText(value(row, "state")) === "idle") return null
  return elapsedSince(row, "state_change")
}

export function decorateActivityRow(row: DataRow): DataRow {
  return {
    ...row,
    values: {
      ...row.values,
      backend_age_ms: backendAgeMs(row),
      query_duration_ms: activityDurationMs(row),
      state_duration_ms: stateDurationMs(row),
      transaction_duration_ms: transactionDurationMs(row),
    },
  }
}

export function activityDurationHistory(rows: readonly DataRow[], field: ActivityDurationField): readonly ChartPoint[] {
  const source = activityDurationSource(field)
  if (source === null) return []
  const ordered = rows.slice().sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  return buildMetricSamples(ordered, (row) => {
    if (!Object.hasOwn(row.values, source)) return undefined
    if ((field === "query_duration_ms" || field === "state_duration_ms") && !Object.hasOwn(row.values, "state")) return undefined
    return activityDuration(row, field)
  })
}

export function activityDurationSource(field: string): string | null {
  if (field === "backend_age_ms") return "backend_start"
  if (field === "query_duration_ms") return "query_start"
  if (field === "state_duration_ms") return "state_change"
  if (field === "transaction_duration_ms") return "xact_start"
  return null
}

function activityDuration(row: DataRow, field: ActivityDurationField): number | null {
  if (field === "backend_age_ms") return backendAgeMs(row)
  if (field === "query_duration_ms") return activityDurationMs(row)
  if (field === "state_duration_ms") return stateDurationMs(row)
  return transactionDurationMs(row)
}

function elapsedSince(row: DataRow, field: string): number | null {
  const started = asNumber(value(row, field))
  return started === null || started <= 0 || started > row.timestamp ? null : (row.timestamp - started) / 1_000
}
