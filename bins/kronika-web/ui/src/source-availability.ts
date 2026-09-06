import type { HourData } from "./api"

export function hasPostgresTelemetry(data: Pick<HourData, "postgresqlConfigured" | "postgresqlPresent">): boolean {
  return data.postgresqlConfigured === true || data.postgresqlPresent === true
}
