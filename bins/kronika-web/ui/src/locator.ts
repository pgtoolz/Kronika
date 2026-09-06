import type { DataRow, Finding } from "./api"

export function rowMatchesLocator(
  row: DataRow,
  locator: Pick<Finding, "segmentId" | "typeId" | "rowOrdinal" | "timestamp">,
): boolean {
  return row.segmentId === locator.segmentId
    && row.typeId === locator.typeId
    && row.ordinal === locator.rowOrdinal
    && row.timestamp === locator.timestamp
}
