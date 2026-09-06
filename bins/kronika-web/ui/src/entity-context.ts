import { registry } from "kronika:registry"

import { type DataRow, type Finding } from "./api"
import type { Translate } from "./help"
import { rawText, value } from "./model"

export interface EntityContext {
  readonly identity: readonly (readonly [string, string])[]
  readonly label: string
  readonly logicalName: string
  readonly typeId: string
}

export type FindingRoute = "activity" | "databases" | "events" | "locks" | "overview" | "plans" | "processes" | "statements" | "system" | "vacuum"

export function entityContext(finding: Finding, row: DataRow | null, t?: Translate): EntityContext | null {
  const layout = registry.find((candidate) => candidate.typeId === finding.typeId)
  if (row === null || row.logicalName !== finding.logicalName || row.typeId !== finding.typeId
    || layout === undefined) return null
  // The activity threshold is snapshot-wide; its locator row does not identify a backend.
  if (finding.logicalName === "pg_stat_activity") return null
  const fields = finding.logicalName === "os_process" ? ["pid"] : layout.identity
  if (fields.length === 0) return null
  const identity = fields.map((field) => [field, rawText(value(row, field))] as const)
  if (identity.some(([, stored]) => stored === null)) return null
  const exact = identity as readonly (readonly [string, string])[]
  return {
    identity: exact,
    label: contextLabel(row, exact, t),
    logicalName: finding.logicalName,
    typeId: finding.typeId,
  }
}

export function contextMatches(row: DataRow, context: EntityContext): boolean {
  const pidOnly = context.logicalName === "os_process" || context.logicalName === "pg_stat_activity"
  return row.logicalName === context.logicalName && (pidOnly || row.typeId === context.typeId)
    && context.identity.every(([field, expected]) => rawText(value(row, field)) === expected)
}

export function contextualRows(
  rows: readonly DataRow[],
  context: EntityContext | null,
  exact: DataRow | null = null,
): readonly DataRow[] {
  if (context === null) return rows
  const filtered = rows.filter((row) => contextMatches(row, context))
  return filtered.length === 0 && exact !== null && contextMatches(exact, context) ? [exact] : filtered
}

export function findingRoute(finding: Finding): FindingRoute {
  const name = finding.logicalName
  if (name === "os_process") return "processes"
  if (name === "health" || name === "instance_metadata" || name.startsWith("os_")) return "system"
  if (name === "pg_stat_activity") return "activity"
  if (name === "pg_stat_progress_vacuum") return "vacuum"
  if (name === "pg_stat_statements") return "statements"
  if (name === "pg_store_plans" || name === "pg_store_plans_info") return "plans"
  if (name === "pg_locks") return "locks"
  if (name === "pg_stat_database") return "databases"
  return name.startsWith("pg_") && !name.startsWith("pg_log_") ? "overview" : "events"
}

function contextLabel(row: DataRow, identity: readonly (readonly [string, string])[], t?: Translate): string {
  const pid = rawText(value(row, "pid"))
  if ((row.logicalName === "os_process" || row.logicalName === "pg_stat_activity") && pid !== null) return `PID ${pid}`
  const subject = row.logicalName === "pg_stat_statements"
    ? translated(t, "filter.context.query", "Query {id}", { id: rawText(value(row, "queryid")) ?? "—" })
    : row.logicalName === "pg_store_plans"
      ? translated(t, "filter.context.plan", "Plan {id}", { id: rawText(value(row, "planid")) ?? "—" })
      : null
  if (subject !== null) {
    const scope = [rawText(value(row, "datname")), rawText(value(row, "usename"))].filter((part): part is string => part !== null && part !== "")
    return [subject, ...scope].join(" · ")
  }
  return identity.map(([field, stored]) => `${field}=${stored}`).join(" · ")
}

function translated(t: Translate | undefined, key: string, fallback: string, slots: Readonly<Record<string, string>>): string {
  const template = t?.(key, slots) ?? Object.entries(slots).reduce((text, [name, stored]) => text.replace(`{${name}}`, stored), fallback)
  return template
}
