import type { DataRow, HourData, LaneContext } from "./api"
import { activityFor } from "./model"

export function hasPostgresTelemetry(data: Pick<HourData, "postgresqlConfigured" | "postgresqlPresent">): boolean {
  return data.postgresqlConfigured === true || data.postgresqlPresent === true
}

export function recordedLinuxEnabled(contexts: readonly LaneContext[]): boolean {
  return contexts.length === 0 || contexts.some((context) => context.osEnabled !== false)
}

export function postgresProcessesShared(contexts: readonly LaneContext[], segmentId: string): boolean {
  return contexts.find((context) => context.segmentId === segmentId)?.postgresqlProcessesShared === true
}

export function activityForProcess(process: DataRow | null, activities: readonly DataRow[], contexts: readonly LaneContext[], cursor: number) {
  const eligible = process !== null && postgresProcessesShared(contexts, process.segmentId)
    ? activities.filter((row) => row.segmentId === process.segmentId) : []
  return activityFor(process, eligible, process?.timestamp ?? cursor)
}
