import { createContext, useContext } from "react"

import { PRODUCT_SECTION_GROUPS, type SnapshotDirection } from "./api"
import type { Source } from "./address"
import { POSTGRES_OVERVIEW_SECTIONS } from "./postgres-overview"

export interface CursorNavigation {
  readonly pending: boolean
  readonly step: (direction: SnapshotDirection) => void
}

// Shared timeline controls use the active screen's source, including when its
// next observation is not present in the already loaded chart.
export const CursorNavigationContext = createContext<CursorNavigation | null>(null)

export function useCursorNavigation(): CursorNavigation | null {
  return useContext(CursorNavigationContext)
}

export function snapshotNavigationSections(source: Source, postgres: string, host: readonly string[]): readonly string[] {
  if (source === "host") return host
  if (source === "processes") return ["os_process"]
  if (source === "events") return PRODUCT_SECTION_GROUPS.events
  const sections: Readonly<Record<string, readonly string[]>> = {
    activity: ["pg_stat_activity"], vacuum: ["pg_stat_progress_vacuum"], locks: ["pg_locks"],
    databases: ["pg_stat_database"], statements: ["pg_stat_statements"], plans: ["pg_store_plans"],
    tables: ["pg_stat_user_tables"], indexes: ["pg_stat_user_indexes"],
    overview: POSTGRES_OVERVIEW_SECTIONS,
  }
  return sections[postgres] ?? []
}
