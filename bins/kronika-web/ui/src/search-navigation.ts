import { searchFields, type SearchSurface } from "./search"

export function searchSurfaceForLocation(source: string, postgresSection: string): SearchSurface | null {
  if (source === "processes") return "os_process"
  if (source === "events") return "events"
  if (source !== "postgresql") return null
  return searchSurfaceForSection(postgresSection)
}

export function searchSurfaceForSection(section: string): SearchSurface | null {
  const direct = section as SearchSurface
  if (searchFields(direct) !== undefined) return direct
  switch (section) {
    case "activity": return "pg_stat_activity"
    case "vacuum": return "pg_stat_progress_vacuum"
    case "statements": return "pg_stat_statements"
    case "plans": return "pg_store_plans"
    case "locks": return "pg_locks"
    case "databases": return "pg_stat_database"
    case "tables": return "pg_stat_user_tables"
    case "indexes": return "pg_stat_user_indexes"
    default: return null
  }
}

export function findAfterSurfaceNavigation(
  currentSurface: SearchSurface | null,
  targetSurface: SearchSurface | null,
  currentFind: string,
  targetFind?: string,
): string {
  if (targetFind !== undefined) return targetFind
  return currentSurface !== null && currentSurface === targetSurface ? currentFind : ""
}
