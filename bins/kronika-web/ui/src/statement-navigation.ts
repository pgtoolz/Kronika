import type { DataRow } from "./api"
import { rawText, value } from "./model"
import { canonicalSearch } from "./search"

export interface RelatedNavigation {
  readonly expression: string
  readonly queryId?: string | undefined
  readonly planId?: string | undefined
  readonly section: "plans" | "statements"
}

export interface StatementTextTarget {
  readonly queryId: string
}

const VADV_PLAN_TYPE_ID = "1004001"

export function statementsForActivity(row: DataRow): RelatedNavigation | null {
  const queryId = nonzero(row, "query_id")
  const databaseId = nonzero(row, "datid")
  const database = rawText(value(row, "datname"))
  if (queryId === null || databaseId === null || database === null || database === "") return null
  return related("statements", [{ key: "database", value: database }, { key: "query_id", value: queryId }], queryId)
}

export function statementsForPlan(row: DataRow): RelatedNavigation | null {
  const queryId = nonzero(row, row.typeId === VADV_PLAN_TYPE_ID ? "queryid_stat_statements" : "queryid")
  const databaseId = nonzero(row, "dbid")
  const roleId = nonzero(row, "userid")
  const database = rawText(value(row, "datname"))
  const role = rawText(value(row, "usename"))
  if (queryId === null || databaseId === null || roleId === null || database === null || role === null) return null
  return related("statements", [
    { key: "database", value: database }, { key: "role", value: role }, { key: "query_id", value: queryId },
  ], queryId)
}

export function statementTextForPlan(row: DataRow): StatementTextTarget | null {
  const queryId = nonzero(row, row.typeId === VADV_PLAN_TYPE_ID ? "queryid_stat_statements" : "queryid")
  return queryId === null ? null : { queryId }
}

export function plansForStatement(row: DataRow): RelatedNavigation | null {
  const queryId = nonzero(row, "queryid")
  const databaseId = nonzero(row, "dbid")
  const roleId = nonzero(row, "userid")
  const database = rawText(value(row, "datname"))
  const role = rawText(value(row, "usename"))
  if (queryId === null || databaseId === null || roleId === null || database === null || role === null) return null
  return related("plans", [
    { key: "database", value: database }, { key: "role", value: role }, { key: "query_id", value: queryId },
  ], queryId)
}

export function plansForPlanId(row: DataRow): RelatedNavigation | null {
  const planId = nonzero(row, "planid")
  if (planId === null) return null
  const expression = canonicalSearch([{ key: "plan_id", value: planId }], "pg_store_plans")
  return expression === null ? null : { expression, planId, section: "plans" }
}

function related(
  section: RelatedNavigation["section"],
  clauses: readonly { readonly key: string; readonly value: string }[],
  queryId: string,
): RelatedNavigation | null {
  const expression = canonicalSearch(clauses, section === "plans" ? "pg_store_plans" : "pg_stat_statements")
  return expression === null ? null : { expression, queryId, section }
}

function nonzero(row: DataRow, field: string): string | null {
  const stored = rawText(value(row, field))
  return stored === null || stored === "0" ? null : stored
}
