import type { DataRow } from "./api"
import { rawText, value } from "./model"

export const KRONIKA_MONITOR_QUERY_PREFIX = "/* kronika:"

export function isKronikaMonitorStatement(row: DataRow): boolean {
  return rawText(value(row, "query"))?.startsWith(KRONIKA_MONITOR_QUERY_PREFIX) === true
}
