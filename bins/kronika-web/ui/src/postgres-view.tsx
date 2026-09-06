import { ChevronDown, ChevronRight, Copy } from "lucide-react"
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react"
import { registry } from "kronika:registry"

import { copyText } from "./clipboard"
import { DatabasesActivity, PlansActivity, RelationsActivity, StatementsActivity } from "./activity"
import type { DataRow, Finding, HourData, SegmentBound, SnapshotRows, StatementScope } from "./api"
import { buildMetricSamples } from "./chart"
import { contextMatches, contextualRows, type EntityContext } from "./entity-context"
import { DetailList, DetailRow } from "./detail-list"
import { createDisplayTimeFormatter, type DisplayTimeFormatter } from "./display-time"
import { useDisplayTime } from "./display-time-context"
import { detailValueRoleForColumn, EntityTable, EstimatedRows, filterTableRows, unit, type EntityColumn, type TableOrder } from "./entity-table"
import type { Translate } from "./help"
import { acceptResponse, fieldNameForLocator, loadSeries, loadSnapshot, segmentBoundAt } from "./api"
import { buildVacuumEpisodes, delayDelta, phaseRisk, phaseSpanUs, progressSeries, sortVacuumEpisodes, vacuumAtTimestamp, vacuumLayoutHas, vacuumLoadShares, vacuumProcessLoad, type VacuumEpisode, type VacuumProcessLoad } from "./postgres-vacuum"
import { buildLockForest, filterLockForest } from "./postgres-locks"
import { LabelHelp } from "./help"
import { useHistoryRequest, type HistoryState } from "./history-request"
import { InspectorChartPortal, InspectorPortal, InspectorRelatedPortal } from "./inspector"
import { ActivityFacts } from "./detail-activity"
import { PlanStatementPanel, StatementPlansPanel } from "./detail-plans"
import { settingAt } from "./postgres-vitals"
import { ProcessFacts } from "./detail-process"
import { activityFor, asNumber, compact, humanBytes, humanDuration, humanPercent, identifier, measure, rawText, snapshot, value, type Locale } from "./model"
import { activityDurationHistory, activityDurationSource, activityDurationMs, decorateActivityRow, transactionDurationMs } from "./postgres-activity"
import { decoratePostgresIntervalRow, findingSemanticField, intervalMetric, PG_STAT_STATEMENTS_TYPE_IDS, PG_STORE_PLANS_TYPE_IDS, physicalField, physicalFields, planDefaultOrder, postgresHistory, postgresIdentity, statementDefaultOrder, unique, type PlanLens, type PostgresSemanticField, type StatementLens } from "./postgres-metrics"
import { PostgresOverview } from "./postgres-overview"
import { PostgresRelationsView } from "./postgres-relations-view"
import { PostgresSummary, usePostgresSummary, type PostgresSummarySection } from "./postgres-summary"
import { PlanSummary, PlanView, QueryView } from "./plan-view"
import { usePlanQueryText } from "./plan-query"
import { plansForPlanId, plansForStatement, statementsForActivity, statementsForPlan, type RelatedNavigation } from "./statement-navigation"
import type { RelationGroup, RelationLens, RelationNavigation } from "./postgres-relations"
import { SeriesChart, type ChartPoint } from "./series-chart"
import type { SearchSurface } from "./search"
import type { SearchRequestState } from "./search-request"
import { Timeline } from "./timeline"
import type { TableRequestPhase } from "./table-request"
import { isKronikaMonitorStatement } from "./statement-scope"

export type PostgresSection = "overview" | "activity" | "vacuum" | "statements" | "plans" | "locks" | "databases" | "tables" | "indexes"

export const ACTIVITY_DEFAULT_ORDER: TableOrder = { column: "query_duration_ms", descending: true }
export { isKronikaMonitorStatement }

function StatementMonitorScope({ checked, count, forced, onChange, t }: {
  readonly checked: boolean
  readonly count: number
  readonly forced: boolean
  readonly onChange: (checked: boolean) => void
  readonly t: Translate
}) {
  // Kronika's own statements are hidden by default. The count is the server's
  // exact number of rows the workload scope kept off the page. An explicit
  // search, entity context or opened collector row includes them and locks the
  // control, because the reader asked for exactly those rows.
  const status = forced || checked
    ? t("pg.statements.monitor_queries.shown")
    : t("pg.statements.monitor_queries.hidden", { count: String(count) })
  return <label className={`inline-flex min-h-7 min-w-0 max-w-full items-center gap-x-1.5 rounded-[var(--radius-sm)] px-1.5 text-[12px] leading-normal text-fg2 coarse:min-h-11 ${forced ? "cursor-default" : "cursor-pointer hover:bg-s3"}`} data-forced={forced || undefined} data-testid="statement-monitor-scope" title={forced ? t("pg.statements.monitor_queries.forced") : undefined}>
    <input aria-label={t("pg.statements.monitor_queries.toggle")} checked={forced || checked} className="flex-none accent-accent" disabled={forced} onChange={(event) => onChange(event.currentTarget.checked)} type="checkbox" />
    <span className="min-w-0 whitespace-nowrap">{t("pg.statements.monitor_queries.label")}</span>
    <span aria-live="polite" className="min-w-0 whitespace-nowrap tabular-nums text-fg4" data-testid="statement-monitor-count">{status}</span>
  </label>
}

/// The statement scope a view needs: the reader's choice, widened to every
/// statement when a search, entity context or an opened collector row would
/// otherwise lose the rows it refers to.
export function statementScopeFor({ context, focus, pattern, rows, selectedKey, show }: {
  readonly context: EntityContext | null
  readonly focus: DataRow | null
  readonly pattern: string
  readonly rows: readonly DataRow[]
  readonly selectedKey: string | null
  readonly show: boolean
}): { readonly scope: StatementScope; readonly forced: boolean } {
  const selectedMonitorQuery = selectedKey !== null
    && rows.some((row) => rowKey(row) === selectedKey && isKronikaMonitorStatement(row))
  const exactMonitorQuery = focus?.logicalName === "pg_stat_statements" && isKronikaMonitorStatement(focus)
  const forced = pattern.trim() !== ""
    || context?.logicalName === "pg_stat_statements"
    || exactMonitorQuery
    || selectedMonitorQuery
  return { scope: show || forced ? "all" : "workload", forced }
}

const ACTIVITY_PID = pgId("pid", "pg.field.pid", 78, true, false)
const ACTIVITY_BACKEND_TYPE = pgExactText("backend_type", "pg.backend_type", 150, true)

export const ACTIVITY_COLUMNS: readonly EntityColumn[] = [
  ACTIVITY_PID, pgExactText("datname", "pg.datname", 145, false, false), pgExactText("usename", "pg.usename", 130, false, false), pgText("query", "pg.query", 420),
  duration("query_duration_ms", 145), duration("transaction_duration_ms", 155),
  pgExactText("application_name", "pg.application_name", 180, false, false), pgExactText("client_addr", "pg.client_addr", 150),
  pgExactText("state", "pg.state", 140), pgExactText("wait_event_type", "pg.wait_event_type", 135), pgExactText("wait_event", "pg.wait_event", 155),
]

export const ACTIVITY_DETAIL_COLUMNS: readonly EntityColumn[] = [
  ...ACTIVITY_COLUMNS, ACTIVITY_BACKEND_TYPE, pgId("leader_pid", "pg.leader_pid", 105), pgId("query_id", "pg.query_id", 150),
  pgNumber("backend_xmin_age", "pg.backend_xmin_age", 145), pgNumber("backend_xid_age", "pg.backend_xid_age", 145),
  duration("backend_age_ms", 145), duration("state_duration_ms", 155),
]

export { activityDurationHistory, activityDurationMs, transactionDurationMs }

export function activityColumns(showSystem: boolean): readonly EntityColumn[] {
  return showSystem ? [ACTIVITY_PID, ACTIVITY_BACKEND_TYPE, ...ACTIVITY_COLUMNS.slice(1)] : ACTIVITY_COLUMNS
}

export const STATEMENT_COLUMNS: readonly EntityColumn[] = [
  rateNumber("calls_per_second", 120),
  rateMilliseconds("execution_ms_per_second", 165),
  { ...milliseconds("mean_exec_ms_per_call", 170), physicalField: physicalFields(PG_STAT_STATEMENTS_TYPE_IDS, "mean_exec_ms_per_call") },
  text("query", 440, true), rateNumber("rows_per_second", 120),
  rateNumber("shared_blks_hit", 145), rateNumber("shared_blks_read", 145), rateNumber("shared_blks_dirtied", 155), rateNumber("shared_blks_written", 155),
  rateNumber("local_blks_hit", 145), rateNumber("local_blks_read", 145), rateNumber("local_blks_dirtied", 155), rateNumber("local_blks_written", 155),
  rateNumber("temp_blks_read", 145), rateNumber("temp_blks_written", 155), rateBytes("wal_bytes", 145), rateNumber("wal_records", 135),
  rateNumber("wal_fpi", 160), rateNumber("wal_buffers_full", 165),
  rateMilliseconds("shared_blk_read_ms_per_second", 165), rateMilliseconds("shared_blk_write_ms_per_second", 170),
  rateMilliseconds("local_blk_read_ms_per_second", 160), rateMilliseconds("local_blk_write_ms_per_second", 165),
  rateMilliseconds("temp_blk_read_ms_per_second", 160), rateMilliseconds("temp_blk_write_ms_per_second", 165),
  rateNumber("plans"), rateMilliseconds("planning_ms_per_second", 165),
  { ...exactText("datname", 145), help: "pg.field.statement_database.help" }, exactText("usename", 130), { ...id("queryid", 155), help: "pg.field.statement_queryid.help" },
  boolean("toplevel", 105), timestamp("stats_since", 210),
]

const STATEMENT_DERIVED_COLUMNS: readonly EntityColumn[] = [
  number("rows_per_call", 130), number("blocks_per_call", 140),
  percent("hit_pct", 115), bytes("wal_per_call", 135), percent("plan_time_pct", 130),
  number("cv", 90), milliseconds("min_exec_time_ms", 125), milliseconds("max_exec_time_ms", 125),
  milliseconds("mean_exec_time_ms", 135), milliseconds("stddev_exec_time_ms", 145),
]

export const STATEMENT_LENSES: Readonly<Record<StatementLens, readonly string[]>> = {
  load: ["query", "datname", "usename", "queryid", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"],
  per_call: ["query", "datname", "usename", "queryid", "mean_exec_ms_per_call", "rows_per_call", "blocks_per_call", "calls_per_second"],
  io: ["query", "datname", "usename", "queryid", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "shared_blks_written", "local_blks_read", "temp_blks_read", "temp_blks_written"],
  resources: ["query", "datname", "usename", "queryid", "wal_bytes", "wal_per_call", "temp_blks_written", "planning_ms_per_second", "plan_time_pct", "calls_per_second", "execution_ms_per_second"],
  stability: ["query", "datname", "usename", "queryid", "cv", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second"],
}

const PLAN_SUMMARY_COLUMN: EntityColumn = {
  available: (row) => Object.hasOwn(row.values, "plan"),
  field: "plan_summary",
  filterValue: (row) => rawText(value(row, "plan")),
  help: "pg.plan.summary.help",
  label: "pg.plan.summary.label",
  physicalField: "plan",
  render: (row) => <PlanSummary raw={rawText(value(row, "plan"))} />,
  sortable: false,
  sticky: true,
  width: 440,
}
const PLAN_QUERY_ID: EntityColumn = {
  ...id("queryid", 155),
  help: "pg.field.plan_queryid.help",
  available: (row) => row.typeId !== "1004001" && Object.hasOwn(row.values, "queryid"),
  render: (row) => row.typeId === "1004001" ? "—" : rawText(value(row, "queryid")) ?? "—",
}
const PLAN_LAST_QUERY_ID: EntityColumn = {
  ...id("queryid_stat_statements", 190),
  help: "pg.field.queryid_stat_statements.help",
  available: (row) => row.typeId === "1004001" && nonzeroId(value(row, "queryid_stat_statements")),
  render: (row) => row.typeId !== "1004001" || !nonzeroId(value(row, "queryid_stat_statements")) ? "—" : rawText(value(row, "queryid_stat_statements")) ?? "—",
}

export const PLAN_COLUMNS: readonly EntityColumn[] = [
  { ...number("calls", 120), render: (row) => rawText(value(row, "calls")) ?? "—", sortable: false },
  rateNumber("calls_per_second", 120),
  rateMilliseconds("execution_ms_per_second", 165),
  { ...milliseconds("mean_exec_ms_per_call", 170), physicalField: physicalFields(PG_STORE_PLANS_TYPE_IDS, "mean_exec_ms_per_call") },
  PLAN_SUMMARY_COLUMN, rateNumber("rows_per_second", 120),
  id("planid", 155), PLAN_QUERY_ID,
  rateNumber("shared_blks_hit", 145), rateNumber("shared_blks_read", 145), rateNumber("shared_blks_dirtied", 155), rateNumber("shared_blks_written", 155),
  rateNumber("local_blks_hit", 145), rateNumber("local_blks_read", 145), rateNumber("local_blks_dirtied", 155), rateNumber("local_blks_written", 155),
  rateNumber("temp_blks_read", 145), rateNumber("temp_blks_written", 155),
  rateMilliseconds("shared_blk_read_ms_per_second", 165), rateMilliseconds("shared_blk_write_ms_per_second", 170),
  rateMilliseconds("local_blk_read_ms_per_second", 160), rateMilliseconds("local_blk_write_ms_per_second", 165),
  rateMilliseconds("temp_blk_read_ms_per_second", 160), rateMilliseconds("temp_blk_write_ms_per_second", 165),
  rateMilliseconds("planning_ms_per_second", 165), rateNumber("slow_log_calls", 145),
  { ...exactText("datname", 145), help: "pg.field.plan_database.help" }, exactText("usename", 130),
  exactText("cmd_type", 125), PLAN_LAST_QUERY_ID,
]

const PLAN_DERIVED_COLUMNS: readonly EntityColumn[] = [
  number("rows_per_call", 130), number("blocks_per_call", 140), percent("hit_pct", 115),
  percent("plan_time_pct", 130), milliseconds("min_exec_time_ms", 125), milliseconds("max_exec_time_ms", 125),
  milliseconds("mean_exec_time_ms", 135), milliseconds("stddev_exec_time_ms", 145),
  timestamp("first_call", 190), timestamp("last_call", 190),
]

export const PLAN_LENSES: Readonly<Record<PlanLens, readonly string[]>> = {
  load: ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"],
  timing: ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "mean_exec_time_ms", "min_exec_time_ms", "max_exec_time_ms", "stddev_exec_time_ms", "calls_per_second", "first_call", "last_call"],
  io: ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "shared_blks_read", "shared_blks_hit", "hit_pct", "blocks_per_call", "shared_blks_dirtied", "local_blks_read", "temp_blks_read"],
  identity: ["plan_summary", "datname", "usename", "queryid", "queryid_stat_statements", "planid", "cmd_type", "calls", "calls_per_second"],
}

const STATEMENT_ALL_COLUMNS = [...STATEMENT_COLUMNS, ...STATEMENT_DERIVED_COLUMNS]
const PLAN_ALL_COLUMNS = [...PLAN_COLUMNS, ...PLAN_DERIVED_COLUMNS]

export function statementColumns(lens: StatementLens, blockSize: number | null = null, onRelated?: (target: RelatedNavigation) => void, t?: Translate): readonly EntityColumn[] {
  return relatedColumns(postgresByteColumns(columnsInOrder(STATEMENT_ALL_COLUMNS, STATEMENT_LENSES[lens]), blockSize), "statement", onRelated, t)
}

export function planColumns(lens: PlanLens, blockSize: number | null = null, onRelated?: (target: RelatedNavigation) => void, t?: Translate): readonly EntityColumn[] {
  return relatedColumns(postgresByteColumns(columnsInOrder(PLAN_ALL_COLUMNS, PLAN_LENSES[lens]), blockSize), "plan", onRelated, t)
}

function relatedColumns(columns: readonly EntityColumn[], source: "plan" | "statement", onRelated?: (target: RelatedNavigation) => void, t?: Translate): readonly EntityColumn[] {
  if (onRelated === undefined) return columns
  return columns.map((column) => {
    const target = source === "statement" && column.field === "queryid"
      ? plansForStatement
      : source === "plan" && (column.field === "queryid" || column.field === "queryid_stat_statements")
        ? statementsForPlan
        : source === "plan" && column.field === "planid" ? plansForPlanId : null
    if (target === null) return column
    const original = column.render
    return {
      ...column,
      render: (row: DataRow) => {
        const navigation = target(row)
        const output = original?.(row) ?? rawText(value(row, column.field)) ?? "—"
        if (navigation === null) return output
        const identity = navigation.queryId ?? navigation.planId ?? ""
        return <button aria-label={t?.(`pg.related.open_${navigation.section}`, { id: identity }) ?? `Open related ${navigation.section} for ${identity}`} className="max-w-full cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap border-0 bg-transparent p-0 text-left text-accent3 underline decoration-dotted underline-offset-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" data-testid={`related-${column.field}`} onClick={(event) => { event.stopPropagation(); onRelated(navigation) }} type="button">{output}</button>
      },
    }
  })
}

function columnsInOrder(columns: readonly EntityColumn[], fields: readonly string[]): readonly EntityColumn[] {
  const byField = new Map(columns.map((column) => [column.field, column]))
  return fields.flatMap((field) => {
    const column = byField.get(field)
    return column === undefined ? [] : [{ ...column, sticky: field === "query" || field === "plan_summary" }]
  })
}

function lockPidCell(row: DataRow, t: Translate): ReactNode {
  const pid = identifier(value(row, "pid"))
  const depth = asNumber(value(row, "lock_tree_depth")) ?? 1
  const prefix = rawText(value(row, "lock_tree_prefix")) ?? ""
  const extraCell = value(row, "lock_tree_extra_blockers")
  const extraBlockers = Array.isArray(extraCell) ? extraCell : []
  const waitsOnPrepared = value(row, "lock_tree_waits_on_prepared") === true
  const notes = [
    ...extraBlockers.map((blocker) => `+${identifier(blocker)}`),
    ...(waitsOnPrepared ? [`+${t("pg.locks.prepared_transaction")}`] : []),
  ]
  return (
    <span className="lock-tree-cell" data-role={depth === 1 ? "root" : "waiter"} title={notes.join(" ") || undefined}>
      {prefix !== "" && <span aria-hidden="true" className="lock-tree-prefix">{prefix}</span>}
      <span aria-hidden="true" className="lock-tree-marker">{depth === 1 ? "●" : "▸"}</span>
      <span className="lock-tree-pid">{pid}</span>
      {notes.length > 0 && <span className="lock-tree-note">{notes.join(" ")}</span>}
    </span>
  )
}

function blockedByCell(row: DataRow, t: Translate): ReactNode {
  const cell = value(row, "blocked_by")
  if (!Array.isArray(cell) || cell.length === 0) return "—"
  return cell.map((blocker) => blocker === 0 ? t("pg.locks.prepared_transaction") : `PID ${identifier(blocker)}`).join(", ")
}

export function lockRowLabel(row: DataRow, t: Translate): string {
  const pid = identifier(value(row, "pid"))
  const depth = asNumber(value(row, "lock_tree_depth")) ?? 1
  const parent = asNumber(value(row, "lock_tree_parent_pid"))
  const extraCell = value(row, "lock_tree_extra_blockers")
  const extra = Array.isArray(extraCell) ? extraCell.map(identifier) : []
  const parts = [parent === null
    ? t("pg.locks.row.root", { pid })
    : t("pg.locks.row.waiter", { depth, parent, pid })]
  if (extra.length > 0) parts.push(t("pg.locks.row.extra", { pids: extra.join(", ") }))
  if (value(row, "lock_tree_waits_on_prepared") === true) parts.push(t("pg.locks.row.prepared"))
  return parts.join(". ")
}

const LOCK_COLUMN_DEFS: readonly EntityColumn[] = [
  id("pid", 190, true, false), pgExactText("datname", "pg.datname", 145, false, false), pgExactText("usename", "pg.usename", 130, false, false), pgText("query", "pg.query", 420), pgExactText("application_name", "pg.application_name", 180, false, false),
  exactText("lock_target", 260), exactText("lock_relname", 180), exactText("lock_locktype", 145), exactText("lock_mode", 180),
  pgExactText("state", "pg.state", 110), pgExactText("wait_event_type", "pg.wait_event_type", 135), pgExactText("wait_event", "pg.wait_event", 155), timestamp("waitstart", 210),
]
export function lockColumns(t: Translate): readonly EntityColumn[] {
  return LOCK_COLUMN_DEFS.map((column) => ({
    ...column,
    ...(column.field === "pid" ? { render: (row: DataRow) => lockPidCell(row, t) } : {}),
    sortable: false,
  }))
}

export function lockDetailColumns(t: Translate): readonly EntityColumn[] {
  return [
    ...lockColumns(t),
    { field: "blocked_by", help: "pg.field.blocked_by.help", kind: "id", label: "pg.field.blocked_by.label", render: (row) => blockedByCell(row, t), sortable: false, width: 220 },
  ]
}

export const LOCK_COLUMNS: readonly EntityColumn[] = lockColumns((key) => key)

export const DATABASE_COLUMNS: readonly EntityColumn[] = [
  exactText("datname", 170, true, false), number("numbackends", 135), number("xact_commit", 145), number("xact_rollback", 145), number("sessions", 125),
  number("tup_returned", 145), number("tup_fetched", 145), number("tup_inserted", 145), number("tup_updated", 145), number("tup_deleted", 145),
  number("blks_read", 140), number("blks_hit", 140), milliseconds("blk_read_time", 150), milliseconds("blk_write_time", 155),
  number("temp_files", 125), bytes("temp_bytes", 145), number("conflicts", 125), number("deadlocks", 125), number("frozen_xid_age", 155),
]

const TABS: readonly { readonly id: PostgresSection; readonly sections?: readonly string[]; readonly divide?: true }[] = [
  { id: "overview" },
  { id: "activity", sections: ["pg_stat_activity"] },
  { id: "vacuum", sections: ["pg_stat_progress_vacuum"] },
  { id: "locks", sections: ["pg_locks"] },
  { id: "statements", sections: ["pg_stat_statements"] },
  { id: "plans", sections: ["pg_store_plans", "pg_store_plans_info"] },
  { divide: true, id: "databases", sections: ["pg_stat_database"] },
  { id: "tables", sections: ["pg_stat_user_tables"] },
  { id: "indexes", sections: ["pg_stat_user_indexes"] },
]


export function PostgresView({
  context,
  densePageState,
  requestPhase,
  onLoadMore,
  onRetry,
  onPattern,
  pattern,
  onOrder,
  order,
  cursor,
  data,
  environment,
  focus,
  focusFinding,
  historyRevision,
  hour,
  locale,
  navigationTimestamps,
  onCursor,
  onContextClear,
  onFinding,
  onOpenChart,
  onPreview,
  onRelated,
  onPlanLens,
  onRelationLens,
  onRelationNavigate,
  onRelationSelectedKey,
  onSelectedLane,
  onSelectedKey,
  onSection,
  onStatementLens,
  planLens,
  segments,
  relationFilters,
  relationLens,
  relationLevel,
  relationSelectedKey,
  searchRequest,
  section,
  selectedKey,
  selectedLane,
  statementLens,
  monitorQueries,
  onMonitorQueries,
  statementScope,
  t,
}: {
  readonly context: EntityContext | null
  readonly densePageState: "idle" | "loading" | "error"
  readonly requestPhase: TableRequestPhase
  readonly onLoadMore: () => void
  readonly onRetry: () => void
  readonly onOrder: (order: TableOrder | null) => void
  readonly onPattern: (pattern: string) => void
  readonly order: TableOrder | undefined
  readonly pattern: string
  readonly cursor: number
  readonly data: HourData
  readonly environment: "machine" | "container" | null
  readonly focus: DataRow | null
  readonly focusFinding: Finding | null
  readonly historyRevision: number
  readonly hour: number
  readonly locale: Locale
  readonly navigationTimestamps: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly onContextClear: () => void
  readonly onFinding: (finding: Finding) => void
  readonly onOpenChart: () => void
  readonly onPreview?: ((timestamp: number | null) => void) | undefined
  readonly onRelated: (target: RelatedNavigation) => void
  readonly onSection: (section: PostgresSection) => void
  readonly onRelationLens: (lens: RelationLens) => void
  readonly onRelationNavigate: (navigation: RelationNavigation) => void
  readonly onRelationSelectedKey: (key: string | null) => void
  readonly onSelectedLane: (lane: string) => void
  readonly onSelectedKey: (key: string | null) => void
  readonly section: PostgresSection
  readonly selectedKey: string | null
  readonly selectedLane: string
  readonly statementLens: StatementLens
  /// The reader's choice to include Kronika's own statements.
  readonly monitorQueries: boolean
  readonly onMonitorQueries: (next: boolean) => void
  /// Scope resolved by the owner of the table request; see `statementScopeFor`.
  readonly statementScope: { readonly scope: StatementScope; readonly forced: boolean }
  readonly planLens: PlanLens
  readonly segments: readonly SegmentBound[]
  readonly onStatementLens: (lens: StatementLens) => void
  readonly onPlanLens: (lens: PlanLens) => void
  readonly relationFilters: Readonly<Record<string, string>>
  readonly relationLens: RelationLens
  readonly relationLevel: RelationGroup
  readonly relationSelectedKey: string | null
  readonly searchRequest: SearchRequestState
  readonly t: Translate
}) {
  const available = (name: string) => data.availableSections.includes(name)
  const blockSize = postgresBlockSize(data.sections.pg_settings ?? [], cursor)
  const locks = useMemo(() => lockColumns(t), [t])
  const lockDetails = useMemo(() => lockDetailColumns(t), [t])
  const postgresSummary = usePostgresSummary(hour, historyRevision, statementScope.scope)
  const statementRows = data.sections.pg_stat_statements ?? NO_ROWS
  // The server keeps the collector's own statements off the page under the
  // workload scope and reports exactly how many it held back.
  const excludedMonitorQueries = data.snapshotRows.find((page) => page.logicalName === "pg_stat_statements")?.excluded ?? 0
  const summary = (summarySection: PostgresSummarySection, lens: string) => <PostgresSummary cursor={cursor} lens={lens} locale={locale} section={summarySection} state={postgresSummary} t={t} />
  useEffect(() => {
    const tab = TABS.find((candidate) => candidate.id === section)
    if (tab === undefined || tab.id === "plans" || tab.id === "vacuum" || tab.id === "tables" || tab.id === "indexes" || tab.sections === undefined || tab.sections.some(available)) return
    onSection("overview")
  }, [data.availableSections, onSection, section])
  return <>
    <Timeline cursor={cursor} environment={environment} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={onCursor} onFinding={onFinding} onOpenChart={onOpenChart} onPreview={onPreview} onSelectedLane={onSelectedLane} primaryLane={section === "statements" || section === "plans" ? "pg_running" : section === "activity" || section === "locks" ? "pg_waiting" : "health"} selectedLane={selectedLane} t={t} />
    <nav aria-label={t("pg.sections")} className="pg-tabs !mt-0 flex min-h-[35px] overflow-x-auto bg-s1">
      {TABS.map((tab) => {
        const enabled = tab.id === "plans" || tab.id === "vacuum" || tab.id === "tables" || tab.id === "indexes" || tab.sections === undefined || tab.sections.some(available)
        return <button aria-current={section === tab.id ? "page" : undefined} className={tab.divide === true ? "ml-2 border-l border-line4" : undefined} disabled={!enabled} key={tab.id} onClick={() => { if (section !== tab.id) onOrder(null); onSection(tab.id) }} title={enabled ? undefined : t("pg.no_section_data")} type="button">
          <span>{t(`pg.section.${tab.id}`)}</span>
        </button>
      })}
    </nav>
    {section === "overview" && <PostgresOverview cursor={cursor} data={data} historyRevision={historyRevision} hour={hour} locale={locale} onCursor={onCursor} t={t} />}
    {section === "activity" && available("pg_stat_activity") && <ActivityView context={context} requestPhase={requestPhase} onContextClear={onContextClear} onCursor={onCursor} onRelated={onRelated} onOrder={onOrder} onPattern={onPattern} onSelectedKey={onSelectedKey} order={order} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_activity" ? focusFinding : null} focus={focus} historyRevision={historyRevision} locale={locale} selectedKey={selectedKey} t={t} />}
    {section === "vacuum" && <VacuumView cursor={cursor} data={data} historyRevision={historyRevision} hour={hour} locale={locale} onCursor={onCursor} onOrder={onOrder} onPattern={onPattern} onRelated={onRelated} onSelectedKey={onSelectedKey} order={order} pattern={pattern} searchRequest={searchRequest} segments={segments} selectedKey={selectedKey} t={t} />}
    {section === "statements" && <><StatementsActivity blockSize={blockSize} cursor={cursor} hour={hour} locale={locale} onCursor={onCursor} onRelated={onRelated} rows={statementRows} scope={statementScope.scope} t={t} /><PostgresLensBar active={statementLens} choices={["load", "per_call", "io", "resources", "stability"]} onChange={onStatementLens} prefix="statement" summary={summary("statements", statementLens)} t={t} /><PgEntityView accessory={<StatementMonitorScope checked={monitorQueries} count={excludedMonitorQueries} forced={statementScope.forced} onChange={onMonitorQueries} t={t} />} columns={statementColumns(statementLens, blockSize, onRelated, t)} context={context} requestPhase={requestPhase} defaultOrder={{ column: statementDefaultOrder(statementLens), descending: true }} densePageState={densePageState} onContextClear={onContextClear} onCursor={onCursor} onLoadMore={onLoadMore} onRetry={onRetry} onOrder={onOrder} onPattern={onPattern} onRelated={onRelated} onSelectedKey={onSelectedKey} pattern={pattern} order={order} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_statements" ? focusFinding : null} focus={focus} historyField={statementLens === "stability" ? "cv" : "mean_exec_ms_per_call"} historyRevision={historyRevision} locale={locale} searchRequest={searchRequest} section="pg_stat_statements" segments={segments} selectedKey={selectedKey} t={t} /></>}
    {section === "plans" && <PostgresLensBar active={planLens} choices={["load", "timing", "io", "identity"]} onChange={onPlanLens} prefix="plan" summary={summary("plans", planLens)} t={t} />}
    {section === "plans" && available("pg_store_plans") && <><PlansActivity blockSize={blockSize} cursor={cursor} hour={hour} locale={locale} onCursor={onCursor} onRelated={onRelated} rows={data.sections.pg_store_plans ?? NO_ROWS} t={t} /><PgEntityView columns={planColumns(planLens, blockSize, onRelated, t)} context={context} requestPhase={requestPhase} defaultOrder={{ column: planDefaultOrder(planLens), descending: true }} densePageState={densePageState} onContextClear={onContextClear} onCursor={onCursor} onLoadMore={onLoadMore} onRetry={onRetry} onRelated={onRelated} onOrder={onOrder} onPattern={onPattern} onSelectedKey={onSelectedKey} pattern={pattern} order={order} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_store_plans" ? focusFinding : null} focus={focus} historyField="mean_exec_ms_per_call" historyRevision={historyRevision} locale={locale} searchRequest={searchRequest} section="pg_store_plans" segments={segments} selectedKey={selectedKey} t={t} /></>}
    {section === "plans" && !available("pg_store_plans") && <p className="m-0 border-y border-line2 bg-s1 p-[22px] text-sm text-fg3" data-testid="pg-plans-empty">{t("pg.plans.empty")}</p>}
    {section === "locks" && <PgEntityView columns={locks} context={context} detailColumns={lockDetails} requestPhase={requestPhase} onContextClear={onContextClear} onCursor={onCursor} onOrder={onOrder} onPattern={onPattern} onSelectedKey={onSelectedKey} order={order} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_locks" ? focusFinding : null} focus={focus} historyField={null} historyRevision={historyRevision} locale={locale} section="pg_locks" selectedKey={selectedKey} t={t} transformRows={buildLockForest} />}
    {section === "databases" && <><DatabasesActivity blockSize={blockSize} cursor={cursor} hour={hour} locale={locale} onCursor={onCursor} onPattern={onPattern} t={t} /><div className="lensbar flex-wrap">{summary("databases", "")}</div><PgEntityView columns={postgresByteColumns(DATABASE_COLUMNS, blockSize)} context={context} requestPhase={requestPhase} onContextClear={onContextClear} onCursor={onCursor} onOrder={onOrder} onPattern={onPattern} onSelectedKey={onSelectedKey} order={order} pattern={pattern} cursor={cursor} data={data} finding={focusFinding?.logicalName === "pg_stat_database" ? focusFinding : null} focus={focus} historyField="xact_commit" historyRevision={historyRevision} locale={locale} section="pg_stat_database" selectedKey={selectedKey} t={t} /></>}
    {section === "tables" && <><RelationsActivity blockSize={blockSize} cursor={cursor} hour={hour} level={relationLevel} locale={locale} onCursor={onCursor} onPattern={onPattern} section="pg_stat_user_tables" t={t} /><PostgresRelationsView blockSize={blockSize} cursor={cursor} data={data} requestPhase={requestPhase} densePageState={densePageState} filters={relationFilters} historyRevision={historyRevision} hour={hour} lens={relationLens} level={relationLevel} locale={locale} onCursor={onCursor} onLens={onRelationLens} onLoadMore={onLoadMore} onNavigate={onRelationNavigate} onOrder={onOrder} onPattern={onPattern} onRetry={onRetry} onSelectedKey={onRelationSelectedKey} order={order} pattern={pattern} searchRequest={searchRequest} section="pg_stat_user_tables" selectedKey={relationSelectedKey} summary={postgresSummary} t={t} /></>}
    {section === "indexes" && <><RelationsActivity blockSize={blockSize} cursor={cursor} hour={hour} level={relationLevel} locale={locale} onCursor={onCursor} onPattern={onPattern} section="pg_stat_user_indexes" t={t} /><PostgresRelationsView blockSize={blockSize} cursor={cursor} data={data} requestPhase={requestPhase} densePageState={densePageState} filters={relationFilters} historyRevision={historyRevision} hour={hour} lens={relationLens} level={relationLevel} locale={locale} onCursor={onCursor} onLens={onRelationLens} onLoadMore={onLoadMore} onNavigate={onRelationNavigate} onOrder={onOrder} onPattern={onPattern} onRetry={onRetry} onSelectedKey={onRelationSelectedKey} order={order} pattern={pattern} searchRequest={searchRequest} section="pg_stat_user_indexes" selectedKey={relationSelectedKey} summary={postgresSummary} t={t} /></>}
  </>
}

export interface ActivityVisibility {
  readonly showIdle: boolean
  readonly showSystem: boolean
}

export function isSystemActivity(row: DataRow): boolean {
  const backendType = rawText(value(row, "backend_type"))
  return backendType !== null && backendType !== "" && backendType !== "client backend"
}

export function isIdleActivity(row: DataRow): boolean {
  return rawText(value(row, "state")) === "idle"
}

export function visibleActivityRows(
  rows: readonly DataRow[],
  visibility: ActivityVisibility,
  focus: DataRow | null = null,
): readonly DataRow[] {
  const focusKey = focus === null ? null : rowKey(focus)
  return rows
    .filter((row) => rowKey(row) === focusKey
      || (visibility.showSystem || !isSystemActivity(row))
        && (visibility.showIdle || !isIdleActivity(row)))
    .map(decorateActivityRow)
    .sort((left, right) => {
      const leftDuration = asNumber(value(left, "query_duration_ms"))
      const rightDuration = asNumber(value(right, "query_duration_ms"))
      if (leftDuration === null && rightDuration === null) {
        return (asNumber(value(right, "transaction_duration_ms")) ?? -1) - (asNumber(value(left, "transaction_duration_ms")) ?? -1)
      }
      if (leftDuration === null) return 1
      if (rightDuration === null) return -1
      return rightDuration - leftDuration
    })
}

function ActivityView({ context, cursor, data, finding, focus, historyRevision, locale, requestPhase, onContextClear, onCursor, onRelated, onOrder, onPattern, onSelectedKey, order, pattern, selectedKey, t }: {
  readonly context: EntityContext | null
  readonly cursor: number
  readonly data: HourData
  readonly finding: Finding | null
  readonly focus: DataRow | null
  readonly historyRevision: number
  readonly locale: Locale
  readonly onContextClear: () => void
  readonly onCursor: (timestamp: number) => void
  readonly onRelated: (target: RelatedNavigation) => void
  readonly requestPhase: TableRequestPhase
  readonly onOrder: (order: TableOrder | null) => void
  readonly onPattern: (pattern: string) => void
  readonly onSelectedKey: (key: string | null) => void
  readonly order: TableOrder | undefined
  readonly pattern: string
  readonly selectedKey: string | null
  readonly t: Translate
}) {
  const [showSystem, setShowSystem] = useState(false)
  const [showIdle, setShowIdle] = useState(false)
  const columns = activityColumns(showSystem)
  const activityOrder = order !== undefined && columns.some(({ field }) => field === order.column) ? order : undefined
  const transformRows = useCallback(
    (rows: readonly DataRow[]) => visibleActivityRows(rows, { showIdle, showSystem }, focus),
    [focus, showIdle, showSystem],
  )
  return <>
    <div className="lensbar flex-wrap" role="group" aria-label={t("pg.section.activity")}>
      <div className="lens-tabs max-[760px]:w-full max-[760px]:[&>button]:min-w-0 max-[760px]:[&>button]:flex-1 max-[760px]:[&>button]:px-1">
        <button aria-pressed={showSystem} data-testid="activity-filter-system" onClick={() => setShowSystem((shown) => !shown)} type="button">{t("pg.activity.system")}</button>
        <button aria-pressed={showIdle} data-testid="activity-filter-idle" onClick={() => setShowIdle((shown) => !shown)} type="button">{t("pg.activity.idle")}</button>
      </div>
    </div>
    <PgEntityView columns={columns} context={context} requestPhase={requestPhase} cursor={cursor} data={data} defaultOrder={ACTIVITY_DEFAULT_ORDER} detailColumns={ACTIVITY_DETAIL_COLUMNS} finding={finding} focus={focus} historyField={null} historyRevision={historyRevision} locale={locale} onContextClear={onContextClear} onCursor={onCursor} onRelated={onRelated} onOrder={onOrder} onPattern={onPattern} onSelectedKey={onSelectedKey} order={activityOrder} pattern={pattern} section="pg_stat_activity" selectedKey={selectedKey} t={t} transformRows={transformRows} />
  </>
}

function PostgresLensBar<L extends string>({ active, choices, onChange, prefix, summary, t }: { readonly active: L; readonly choices: readonly L[]; readonly onChange: (lens: L) => void; readonly prefix: "statement" | "plan"; readonly summary?: ReactNode | undefined; readonly t: Translate }) {
  return <div className="lensbar flex-wrap"><span>{t("pg.lens.label")}</span><div className="lens-tabs max-[760px]:w-full max-[760px]:[&>button]:min-w-0 max-[760px]:[&>button]:flex-1 max-[760px]:[&>button]:px-1" role="group" aria-label={t("pg.lens.label")}>{choices.map((choice) => <button aria-pressed={active === choice} data-testid={`${prefix}-lens-${choice}`} key={choice} onClick={() => onChange(choice)} type="button">{t(`pg.lens.${choice}`)}</button>)}</div>{summary}<div className="ml-auto flex items-center gap-[5px] text-xs text-fg4 [&_i]:ml-2 [&_i]:inline-block [&_i]:h-1.5 [&_i]:w-1.5 [&_i]:rounded-full" aria-label={t("pg.value.legend")}><i className="bg-ok" />{t("pg.value.good")}<i className="bg-warn" />{t("pg.value.warning")}<i className="bg-bad" />{t("pg.value.critical")}</div></div>
}

function VacuumView({ cursor, data, historyRevision, hour, locale, onCursor, onOrder, onPattern, onRelated, onSelectedKey, order, pattern, searchRequest, segments, selectedKey, t }: {
  readonly cursor: number
  readonly data: HourData
  readonly historyRevision: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onOrder: (order: TableOrder | null) => void
  readonly onPattern: (pattern: string) => void
  readonly onRelated: (target: RelatedNavigation) => void
  readonly onSelectedKey: (key: string | null) => void
  readonly order: TableOrder | undefined
  readonly pattern: string
  readonly searchRequest: SearchRequestState
  readonly segments: readonly SegmentBound[]
  readonly selectedKey: string | null
  readonly t: Translate
}) {
  const time = useDisplayTime()
  // Omit fields so each segment returns its own physical layout.
  // Requesting a cross-version union would reject layout-absent columns.
  const hourRows = useHistoryRequest(String(hour), historyRevision,
    (signal) => loadSeries(hour, "pg_stat_progress_vacuum", {}, [], signal))
  const vacuumRequestPhase = hourRows.status === "loading" ? "pending" : hourRows.status
  const allRows = hourRows.value ?? data.sections.pg_stat_progress_vacuum ?? NO_ROWS

  // Missing cadence or clock metadata disables the dependent calculations.
  const [intervalSeconds, setIntervalSeconds] = useState<number | null>(null)
  const [ticksPerSecond, setTicksPerSecond] = useState<number | null>(null)
  const anchorId = segmentBoundAt(segments, cursor)?.id ?? null
  useEffect(() => {
    const bound = segments.find((candidate) => candidate.id === anchorId)
    if (bound === undefined) {
      setIntervalSeconds(null)
      setTicksPerSecond(null)
      return undefined
    }
    const controller = new AbortController()
    acceptResponse(
      loadSnapshot(bound.id, bound.maxTs, [{ fields: ["postgresql_interval_seconds", "clock_ticks_per_sec"], section: "instance_metadata" }], controller.signal),
      controller.signal,
      (loaded) => {
        const metadata = loaded.sections.instance_metadata?.[0] ?? null
        setIntervalSeconds(asNumber(value(metadata, "postgresql_interval_seconds")))
        setTicksPerSecond(asNumber(value(metadata, "clock_ticks_per_sec")))
      },
      () => {
        setIntervalSeconds(null)
        setTicksPerSecond(null)
      },
    )
    return () => controller.abort()
  }, [anchorId, segments])

  const episodes = useMemo(() => buildVacuumEpisodes(allRows, intervalSeconds), [allRows, intervalSeconds])
  const atTs = useMemo(() => vacuumAtTimestamp(allRows, cursor), [allRows, cursor])
  const sorted = useMemo(() => sortVacuumEpisodes(episodes, atTs), [atTs, episodes])
  const displayRows = useMemo(() => sorted.map((episode) => episode.last), [sorted])
  const episodeByKey = useMemo(() => new Map(sorted.map((episode) => [rowKey(episode.last), episode])), [sorted])

  const blockSize = postgresBlockSize(data.sections.pg_settings ?? [], cursor)
  const columns = useMemo(
    () => vacuumColumns(allRows, episodeByKey, atTs, blockSize, time, locale, t),
    [allRows, atTs, blockSize, episodeByKey, locale, t, time],
  )

  const [selected, setSelected] = useState<DataRow | null>(null)
  useEffect(() => {
    setSelected((current) => {
      if (selectedKey === null) return null
      const exact = displayRows.find((row) => rowKey(row) === selectedKey)
      if (exact !== undefined) return exact
      if (vacuumRequestPhase !== "ready" && current !== null) return current
      return current !== null && rowKey(current) === selectedKey
        ? selectedEntity(displayRows, current, "pg_stat_progress_vacuum")
        : null
    })
  }, [displayRows, selectedKey, vacuumRequestPhase])
  const selectedEpisode = selected === null ? undefined : episodeByKey.get(rowKey(selected))
  const joinedActivity = activityFor(selected, data.sections.pg_stat_activity ?? [], cursor)

  const workerCount = useMemo(
    () => snapshot(data.sections.pg_stat_activity ?? [], cursor)
      .filter((row) => rawText(value(row, "backend_type")) === "autovacuum worker").length,
    [cursor, data.sections.pg_stat_activity],
  )
  const workerMax = settingAt(data.sections.pg_settings ?? [], "autovacuum_max_workers", cursor)
  const contentSized = displayRows.length < 10

  return <>
    <div className="lensbar !mt-0 border-t-0">
      <span className="flex items-center gap-1.5 font-sans text-xs text-fg3">
        <LabelHelp
          helpKey="pg.vacuum.workers.help"
          helpText={intervalSeconds === null ? undefined : `${t("pg.vacuum.workers.help")} ${t("pg.vacuum.cadence", { seconds: compact(intervalSeconds, locale) })}`}
          labelKey="pg.vacuum.workers.label"
          t={t}
        />
        <strong className="font-mono text-xs font-normal tabular-nums text-fg2">{workerMax === null ? String(workerCount) : `${workerCount} / ${workerMax}`}</strong>
      </span>
    </div>
    <div className={`pg-entity-layout grid min-h-0 min-w-0 grid-cols-[minmax(0,1fr)]${contentSized ? "" : " pg-entity-fill"}`} data-content-sized={contentSized || undefined}>
      <EntityTable
        columns={columns}
        contentSized={contentSized}
        empty={t("pg.vacuum.empty")}
        label={t("pg.section.vacuum")}
        requestPhase={vacuumRequestPhase}
        locale={locale}
        onOrder={onOrder}
        onPattern={onPattern}
        onSelect={(row) => { setSelected(row); onSelectedKey(rowKey(row)) }}
        order={order}
        pattern={pattern}
        rows={displayRows}
        searchRequest={searchRequest}
        searchSurface="pg_stat_progress_vacuum"
        selectedKey={selected === null ? null : rowKey(selected)}
        t={t}
        testId="pg-vacuum-table"
      />
      {selected !== null && <InspectorPortal identity={`postgres:pg_stat_progress_vacuum:${rowKey(selected)}`} onClose={() => { setSelected(null); onSelectedKey(null) }} title={detailTitle(selected, "pg_stat_progress_vacuum", t)}>
        <PgDetail
          allRows={allRows}
          columns={vacuumDetailColumns(selected, blockSize)}
          cursor={cursor}
          historyField="heap_blks_scanned"
          historyRevision={historyRevision}
          hour={hour}
          locale={locale}
          onCursor={onCursor}
          prelude={selectedEpisode === undefined ? undefined : <VacuumEpisodeFacts episode={selectedEpisode} hour={hour} locale={locale} onCursor={onCursor} t={t} />}
          row={selected}
          section="pg_stat_progress_vacuum"
          segments={segments}
          t={t}
        />
        {joinedActivity.row !== null && <InspectorRelatedPortal id="pg_stat_activity" identity={`vacuum:${rowKey(selected)}`} label={t("pg.section.activity")}>
          <ActivityFacts activity={joinedActivity.row} activityTime={joinedActivity.snapshotTime} locale={locale} onRelated={onRelated} t={t} />
        </InspectorRelatedPortal>}
        {selectedEpisode !== undefined && <InspectorRelatedPortal id="os_process" identity={`vacuum:${rowKey(selected)}`} label={t("pg.related.process_tab")}>
          <VacuumProcessTab blockSize={blockSize} cursor={cursor} episode={selectedEpisode} hour={hour} locale={locale} t={t} ticksPerSecond={ticksPerSecond} />
        </InspectorRelatedPortal>}
      </InspectorPortal>}
    </div>
  </>
}

// relid is the fallback identity when the recorded relation name is absent.
// Display it only as part of the relation label, never as a bare OID.
function vacuumRelationLabel(row: DataRow): string {
  const datname = rawText(value(row, "datname")) ?? identifier(value(row, "datid"))
  const schema = rawText(value(row, "schemaname"))
  const relname = rawText(value(row, "relname"))
  return schema !== null && relname !== null
    ? `${datname}.${schema}.${relname}`
    : `${datname} · relid=${identifier(value(row, "relid"))}`
}

function vacuumHeapCell(row: DataRow, field: string, blockSize: number | null, locale: Locale): string {
  const blocks = asNumber(value(row, field))
  if (blocks === null) return "—"
  return blockSize === null ? measure(blocks, locale) : humanBytes(blocks * blockSize, locale)
}

function vacuumHeapBytes(row: DataRow, field: string, blockSize: number | null): number | null {
  const blocks = asNumber(value(row, field))
  return blocks === null || blockSize === null ? null : blocks * blockSize
}

function vacuumScanPercent(row: DataRow): number | null {
  const scanned = asNumber(value(row, "heap_blks_scanned"))
  const total = asNumber(value(row, "heap_blks_total"))
  return scanned === null || total === null || total <= 0 ? null : Math.max(0, Math.min(100, (scanned / total) * 100))
}

const VACUUM_INDEX_PHASES = new Set(["vacuuming indexes", "cleaning up indexes"])

export function vacuumColumns(
  allRows: readonly DataRow[],
  episodes: ReadonlyMap<string, VacuumEpisode>,
  atTs: number | null,
  blockSize: number | null,
  time: DisplayTimeFormatter,
  locale: Locale,
  t: Translate,
): readonly EntityColumn[] {
  const episode = (row: DataRow) => episodes.get(rowKey(row))
  const phaseOf = (row: DataRow) => rawText(value(row, "phase"))
  return [
    {
      field: "vacuum_seen", label: "pg.vacuum.seen.label", help: "pg.vacuum.seen.help", kind: "text", width: 150,
      render: (row) => atTs !== null && row.timestamp === atTs
        ? <span className="vacuum-chip" data-risk="at-sample">{t("pg.vacuum.at_sample")}</span>
        : <span className="text-fg3">{t("pg.vacuum.last_seen", { time: time.timestamp(row.timestamp) })}</span>,
      sortValue: (row) => row.timestamp,
    },
    {
      field: "datname", label: "pg.vacuum.relation.label", help: "pg.vacuum.relation.help", kind: "text", width: 250, sticky: true,
      render: (row) => vacuumRelationLabel(row),
      sortValue: (row) => vacuumRelationLabel(row),
    },
    {
      field: "is_autovacuum", label: "pg.vacuum.kind.label", help: "pg.vacuum.kind.help", kind: "text", width: 120,
      render: (row) => value(row, "is_autovacuum") === true ? t("pg.vacuum.kind.autovacuum") : t("pg.vacuum.kind.manual"),
      sortValue: (row) => value(row, "is_autovacuum") === true,
    },
    {
      field: "phase", label: "pg.vacuum.phase.label", help: "pg.vacuum.phase.help", kind: "text", width: 175,
      render: (row) => {
        const phase = phaseOf(row)
        return phase === null ? "—" : <span className="vacuum-chip" data-risk={phaseRisk(phase)}>{phase}</span>
      },
      sortValue: (row) => phaseOf(row),
    },
    {
      field: "vacuum_in_phase", label: "pg.vacuum.in_phase.label", help: "pg.vacuum.in_phase.help", kind: "text", width: 145,
      render: (row) => {
        const own = episode(row)
        if (own === undefined) return "—"
        const span = phaseSpanUs(own)
        return `${humanDuration(span / 1000, locale)} · ${own.phaseRows.length}`
      },
      sortValue: (row) => {
        const own = episode(row)
        return own === undefined ? 0 : phaseSpanUs(own)
      },
    },
    pgColumn("pid", "id", 80, false, false),
    {
      field: "vacuum_progress", label: "pg.vacuum.progress.label", help: "pg.vacuum.progress.help", kind: "text", width: 130,
      render: (row) => {
        const own = episode(row)
        const percent = vacuumScanPercent(row)
        if (percent === null) return "—"
        return <span className="vacuum-progress-cell" data-risk={phaseRisk(phaseOf(row))}>
          <VacuumProgressSpark series={own === undefined ? [percent] : progressSeries(own)} />
          <strong>{humanPercent(percent, locale)}</strong>
        </span>
      },
      sortValue: (row) => vacuumScanPercent(row),
    },
    {
      field: "heap_blks_scanned", label: "pg.vacuum.heap_size.label", help: "pg.vacuum.heap_size.help", kind: "text", width: 170,
      render: (row) => `${vacuumHeapCell(row, "heap_blks_scanned", blockSize, locale)} / ${vacuumHeapCell(row, "heap_blks_total", blockSize, locale)}`,
      sortValue: (row) => asNumber(value(row, "heap_blks_total")),
    },
    {
      field: "heap_blks_vacuumed", label: "pg.vacuum.heap_vacuumed.label", help: "pg.vacuum.heap_vacuumed.help", kind: "text", width: 130,
      render: (row) => vacuumHeapCell(row, "heap_blks_vacuumed", blockSize, locale),
      sortValue: (row) => asNumber(value(row, "heap_blks_vacuumed")),
    },
    {
      field: "index_vacuum_count", label: "pg.vacuum.cycles.label", help: "pg.vacuum.cycles.help", kind: "text", width: 110,
      render: (row) => {
        const cycles = asNumber(value(row, "index_vacuum_count"))
        if (cycles === null) return "—"
        return cycles > 1
          ? <span className="vacuum-chip" data-risk="heavy">{t("pg.vacuum.cycles.repeat", { count: compact(cycles, locale) })}</span>
          : compact(cycles, locale)
      },
      sortValue: (row) => asNumber(value(row, "index_vacuum_count")),
    },
    ...vacuumLayoutHas(allRows, "indexes_total") ? [{
      field: "indexes_processed", label: "pg.vacuum.index_progress.label", help: "pg.vacuum.index_progress.help", kind: "text", width: 130,
      render: (row: DataRow) => {
        const phase = phaseOf(row)
        const processed = asNumber(value(row, "indexes_processed"))
        const total = asNumber(value(row, "indexes_total"))
        if (processed === null || total === null) return t("common.unavailable")
        return phase !== null && VACUUM_INDEX_PHASES.has(phase) ? `${compact(processed, locale)} / ${compact(total, locale)}` : "—"
      },
      sortValue: (row: DataRow) => asNumber(value(row, "indexes_processed")),
    } satisfies EntityColumn] : [],
    ...vacuumLayoutHas(allRows, "delay_time") ? [{
      field: "delay_time", label: "pg.vacuum.delay.label", help: "pg.vacuum.delay.help", kind: "text", width: 150,
      render: (row: DataRow) => {
        const total = asNumber(value(row, "delay_time"))
        if (total === null) return t("common.unavailable")
        const delta = episode(row) === undefined ? null : delayDelta(episode(row) as VacuumEpisode)
        return delta === null || delta === 0
          ? humanDuration(total, locale)
          : `${humanDuration(total, locale)} · +${humanDuration(delta, locale)}`
      },
      sortValue: (row: DataRow) => asNumber(value(row, "delay_time")),
    } satisfies EntityColumn] : [],
    {
      field: "vacuum_no_movement", label: "pg.vacuum.no_movement.label", help: "pg.vacuum.no_movement.help", kind: "text", width: 165,
      render: (row) => {
        const still = episode(row)?.noMovement ?? null
        return still === null ? "—" : t("pg.vacuum.no_movement.value", { count: still.samples, span: humanDuration(still.spanUs / 1000, locale) })
      },
      sortValue: (row) => episode(row)?.noMovement?.samples ?? 0,
    },
  ]
}

// Layout-absent fields render as N/A.
export function vacuumDetailColumns(row: DataRow, blockSize: number | null): readonly EntityColumn[] {
  const extra = (field: string, kind: NonNullable<EntityColumn["kind"]>): EntityColumn =>
    ({ field, label: `pg.vacuum.${field}.label`, help: `pg.vacuum.${field}.help`, kind, width: 140 })
  return postgresByteColumns([
    pgColumn("pid", "id", 80, false, false),
    { ...extra("datname", "text"), detailValueRole: "machine" },
    extra("is_autovacuum", "boolean"), { ...extra("phase", "text"), detailValueRole: "machine" },
    extra("heap_blks_total", "number"), extra("heap_blks_scanned", "number"), extra("heap_blks_vacuumed", "number"),
    extra("index_vacuum_count", "number"),
    ...row.typeId === "1012004" ? [extra("num_dead_tuples", "number"), extra("max_dead_tuples", "number")] : [
      extra("dead_tuple_bytes", "bytes"), extra("max_dead_tuple_bytes", "bytes"), extra("num_dead_item_ids", "number"),
      extra("indexes_processed", "number"), extra("indexes_total", "number"),
    ],
    ...row.typeId === "1012006" ? [extra("delay_time", "milliseconds")] : [],
  ], blockSize)
}

// Scale progress to the episode's samples, not the full hour.
function VacuumProgressSpark({ series }: { readonly series: readonly number[] }) {
  const width = 64
  const height = 16
  const pad = 2
  const usableWidth = width - 2 * pad
  const usableHeight = height - 2 * pad
  const points = series.map((percent, index) => {
    const x = series.length < 2 ? width / 2 : pad + (index / (series.length - 1)) * usableWidth
    const y = pad + (1 - percent / 100) * usableHeight
    return [x, y] as const
  })
  const path = points.map(([x, y], index) => `${index === 0 ? "M" : "L"}${x.toFixed(1)} ${y.toFixed(1)}`).join(" ")
  const last = points[points.length - 1]
  return <svg aria-hidden="true" className="vacuum-progress-spark" preserveAspectRatio="none" viewBox={`0 0 ${width} ${height}`}>
    {points.length > 1
      ? <path d={path} fill="none" strokeWidth={1.4} vectorEffect="non-scaling-stroke" />
      : last !== undefined && <path d={`M${last[0].toFixed(1)} ${last[1].toFixed(1)} l0.01 0`} strokeLinecap="round" strokeWidth={3} vectorEffect="non-scaling-stroke" />}
  </svg>
}

function VacuumEpisodeFacts({ episode, hour, locale, onCursor, t }: {
  readonly episode: VacuumEpisode
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const span = phaseSpanUs(episode)
  const first = episode.rows[0] as DataRow
  return <section className="border-b border-line2 px-2 py-1.5" data-testid="vacuum-episode">
    <svg className="block h-[14px] w-full" preserveAspectRatio="none" viewBox="0 0 1000 10">
      {episode.rows.map((sample) => {
        const x = (sample.timestamp - hour) / 3_600_000_000 * 1000
        return <rect
          className="vacuum-tick cursor-pointer"
          data-risk={phaseRisk(rawText(value(sample, "phase")))}
          height={10}
          key={`${sample.segmentId}:${sample.ordinal}`}
          onClick={() => onCursor(sample.timestamp)}
          width={3}
          x={Math.max(0, Math.min(997, x))}
          y={0}
        />
      })}
    </svg>
    <p className="m-0 mt-1 font-sans text-xs text-fg3">
      {t("pg.vacuum.episode", { count: episode.rows.length, span: humanDuration((episode.last.timestamp - first.timestamp) / 1000, locale) })}
      {" · "}
      {t("pg.vacuum.in_phase.value", { count: episode.phaseRows.length, span: humanDuration(span / 1000, locale) })}
      {episode.noMovement === null ? "" : ` · ${t("pg.vacuum.no_movement.value", { count: episode.noMovement.samples, span: humanDuration(episode.noMovement.spanUs / 1000, locale) })}`}
    </p>
  </section>
}

function VacuumProcessTab({ blockSize, cursor, episode, hour, locale, t, ticksPerSecond }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly episode: VacuumEpisode
  readonly hour: number
  readonly locale: Locale
  readonly t: Translate
  readonly ticksPerSecond: number | null
}) {
  const pid = asNumber(value(episode.last, "pid"))
  const [rows, setRows] = useState<readonly DataRow[] | null | undefined>(undefined)
  useEffect(() => {
    setRows(undefined)
    if (pid === null) {
      setRows(null)
      return undefined
    }
    const controller = new AbortController()
    acceptResponse(
      loadSeries(hour, "os_process", { pid: String(pid) }, [], controller.signal),
      controller.signal,
      (loaded) => setRows(loaded),
      () => setRows(null),
    )
    return () => controller.abort()
  }, [hour, pid])
  if (rows === undefined) return <section className="p-3" data-testid="vacuum-process-panel"><p className="m-0 text-sm text-fg4">{t("history.loading")}</p></section>
  if (rows === null || rows.length === 0) return <section className="p-3" data-testid="vacuum-process-panel"><p className="m-0 text-sm text-fg4">{t("pg.related.process_missing")}</p></section>
  const current = snapshot(rows, cursor)[0] ?? null
  const load = vacuumProcessLoad(rows, episode)
  return <section data-testid="vacuum-process-panel">
    {load !== null && <VacuumLoadFacts blockSize={blockSize} episode={episode} load={load} locale={locale} t={t} ticksPerSecond={ticksPerSecond} />}
    <ProcessFacts locale={locale} process={current} processTime={current?.timestamp ?? null} t={t} />
  </section>
}

// Process deltas span the episode endpoints.
// A manual VACUUM backend may include other work in that interval.
function VacuumLoadFacts({ blockSize, episode, load, locale, t, ticksPerSecond }: {
  readonly blockSize: number | null
  readonly episode: VacuumEpisode
  readonly load: VacuumProcessLoad
  readonly locale: Locale
  readonly t: Translate
  readonly ticksPerSecond: number | null
}) {
  const scannedBytes = vacuumHeapBytes(episode.last, "heap_blks_scanned", blockSize)
  const { blockWaitMs, cpuMs, cpuShare, majorFaults, readBytes, readShare, writeBytes } = vacuumLoadShares(load, ticksPerSecond, scannedBytes)
  const nothingRecorded = cpuMs === null && readBytes === null && writeBytes === null && blockWaitMs === null && majorFaults === null
  return <section className="border-b border-line2 px-2 py-1.5" data-testid="vacuum-process-load">
    <span className="mb-1 flex items-center gap-1 font-sans text-xs font-medium text-fg2"><LabelHelp helpKey="pg.vacuum.load.help" labelKey="pg.vacuum.load.label" t={t} /></span>
    {nothingRecorded
      ? <p className="m-0 text-sm text-fg4">{t("pg.vacuum.load.unavailable")}</p>
      : <DetailList>
        {cpuMs !== null && <DetailRow term={<LabelHelp helpKey="pg.vacuum.load.cpu.help" labelKey="pg.vacuum.load.cpu.label" t={t} />}>
          {humanDuration(cpuMs, locale)}{cpuShare === null ? "" : ` · ${humanPercent(cpuShare, locale)}`}
        </DetailRow>}
        {readBytes !== null && <DetailRow term={<LabelHelp helpKey="pg.vacuum.load.read.help" labelKey="pg.vacuum.load.read.label" t={t} />}>
          {humanBytes(readBytes, locale)}{readShare === null ? "" : ` ${t("pg.vacuum.load.read_share", { share: humanPercent(readShare, locale) })}`}
        </DetailRow>}
        {writeBytes !== null && <DetailRow term={<LabelHelp helpKey="pg.vacuum.load.write.help" labelKey="pg.vacuum.load.write.label" t={t} />}>{humanBytes(writeBytes, locale)}</DetailRow>}
        {blockWaitMs !== null && <DetailRow term={<LabelHelp helpKey="pg.vacuum.load.block_wait.help" labelKey="pg.vacuum.load.block_wait.label" t={t} />}>{humanDuration(blockWaitMs, locale)}</DetailRow>}
        {majorFaults !== null && <DetailRow term={<LabelHelp helpKey="pg.vacuum.load.major_faults.help" labelKey="pg.vacuum.load.major_faults.label" t={t} />}>{compact(majorFaults, locale)}</DetailRow>}
      </DetailList>}
  </section>
}

export function postgresMetricHistory(rows: readonly DataRow[], column: EntityColumn, cumulative: boolean): readonly ChartPoint[] {
  const owned = rows.filter((row) => Object.hasOwn(row.values, column.field))
    .slice().sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  if (!cumulative) return buildMetricSamples(owned, (row) => chartPointValue(value(row, column.field), column))
  return owned.map((row, index) => {
    const earlier = owned[index - 1]
    const stored = earlier === undefined || earlier.typeId !== row.typeId ? null : intervalMetric(earlier, row, column.field)
    return { segmentId: row.segmentId, timestamp: row.timestamp, value: chartPointValue(stored, column) }
  })
}

const NO_ROWS: readonly DataRow[] = []
const NO_RATES: readonly string[] = []

function PgEntityView({
  accessory,
  context,
  requestPhase,
  onOrder,
  order,
  columns,
  detailColumns,
  cursor,
  data,
  focus,
  finding,
  historyField,
  historyRevision,
  locale,
  onRelated,
  onPattern,
  onSelectedKey,
  pattern,
  section,
  segments,
  selectedKey,
  t,
  defaultOrder,
  densePageState,
  onLoadMore,
  onContextClear,
  onCursor,
  onRetry,
  searchRequest,
  statusRowCount,
  transformRows,
}: {
  readonly accessory?: ReactNode | undefined
  readonly context?: EntityContext | null | undefined
  readonly columns: readonly EntityColumn[]
  readonly detailColumns?: readonly EntityColumn[] | undefined
  readonly cursor: number
  readonly data: HourData
  readonly focus: DataRow | null
  readonly finding?: Finding | null
  readonly historyField: string | null
  readonly historyRevision: number
  readonly locale: Locale
  readonly onRelated?: ((target: RelatedNavigation) => void) | undefined
  readonly onSelectedKey?: ((key: string | null) => void) | undefined
  readonly section: SearchSurface
  readonly segments?: readonly SegmentBound[] | undefined
  readonly selectedKey?: string | null | undefined
  readonly requestPhase: TableRequestPhase
  readonly onOrder?: ((order: TableOrder | null) => void) | undefined
  readonly onPattern?: ((pattern: string) => void) | undefined
  readonly pattern?: string | undefined
  readonly order?: TableOrder | undefined
  readonly t: Translate
  readonly defaultOrder?: TableOrder | undefined
  readonly densePageState?: "idle" | "loading" | "error" | undefined
  readonly onLoadMore?: (() => void) | undefined
  readonly onContextClear?: (() => void) | undefined
  readonly onCursor: (timestamp: number) => void
  readonly onRetry?: (() => void) | undefined
  readonly searchRequest?: SearchRequestState | undefined
  readonly statusRowCount?: number | undefined
  readonly transformRows?: ((rows: readonly DataRow[]) => readonly DataRow[]) | undefined
}) {
  const time = useDisplayTime()
  const allRows = data.sections[section] ?? NO_ROWS
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  const activeOrder = useMemo(() => order !== undefined && columns.some((column) => column.field === order.column && column.sortable === true)
    ? order
    : defaultOrder, [columns, defaultOrder, order])
  const ranked = useMemo(() => dense
    ? allRows.map(decoratePostgresIntervalRow)
    : snapshot(allRows, cursor), [allRows, cursor, dense])
  const activeContext = context?.logicalName === section ? context : null
  const exactFocus = useMemo(() => focus === null || focus.logicalName !== section || (dense && pattern?.trim() !== "")
    ? null : dense ? decoratePostgresIntervalRow(focus) : focus, [dense, focus, pattern, section])
  const rows = useMemo(() => {
    const contextual = contextualRows(ranked, activeContext, exactFocus)
    return transformRows === undefined ? contextual : transformRows(contextual)
  }, [activeContext, exactFocus, ranked, transformRows])
  const rates = data.rateColumns[section] ?? NO_RATES
  const visibleColumns = useMemo(
    () => visibleEntityColumns(columns, rows, rates),
    [columns, rates, rows],
  )
  const visibleDetailColumns = useMemo(
    () => visibleEntityColumns(detailColumns ?? columns, rows, rates),
    [columns, detailColumns, rates, rows],
  )
  const [selected, setSelected] = useState<DataRow | null>(null)
  useEffect(() => {
    setSelected((current) => {
      if (selectedKey === undefined) return requestPhase !== "ready" && current !== null
        ? current
        : selectedEntity(rows, current, section)
      if (selectedKey === null) return null
      const exact = rows.find((row) => rowKey(row) === selectedKey)
      if (exact !== undefined) return exact
      if (requestPhase !== "ready" && current !== null) return current
      return current !== null && rowKey(current) === selectedKey
        ? selectedEntity(rows, current, section)
        : null
    })
  }, [requestPhase, rows, section, selectedKey])
  const selectedRowKey = selected === null ? null : rowKey(selected)
  const selectedHistoryField = findingHistoryField(visibleColumns, finding, historyField)
  const metadata = data.snapshotRows.find((meta) => meta.logicalName === section)
  const focusPreview = exactFocus !== null && activeContext !== null
    && (densePageState === "loading" || !ranked.some((row) => contextMatches(row, activeContext)))
    ? densePageState === "loading" ? "loading" : "outside"
    : null
  const canLoadMore = metadata?.hasMore === true && metadata.nextCursor !== null
  const displayedRows = useMemo(
    () => section === "pg_locks"
      ? filterLockForest(rows, pattern ?? "")
      : filterTableRows(rows, visibleColumns, pattern ?? "", dense, section),
    [dense, pattern, rows, section, visibleColumns],
  )
  const snapshotStatus = dense
    ? tableState(metadata, statusRowCount ?? displayedRows.length, pattern, activeOrder, locale, t, time, focusPreview)
    : undefined
  const contentSized = displayedRows.length < 10 && !canLoadMore
  const paging = dense && (densePageState !== "idle" || canLoadMore)
    ? <button disabled={densePageState === "loading"} onClick={densePageState === "error" ? onRetry : onLoadMore} type="button">
      {densePageState === "loading" ? "…" : densePageState === "error" ? "↻" : "+"}
    </button>
    : undefined
  const status = historyField === null ? snapshotStatus : <>{snapshotStatus}<span>{t("system.history")}</span></>
  return <div className={`pg-entity-layout mt-2 grid min-w-0 grid-cols-[minmax(0,1fr)]${contentSized ? "" : " pg-entity-fill"}`} data-content-sized={contentSized || undefined} data-pg-section={sectionName(section)} data-testid="pg-entity-layout">
    <div className={`pg-entity-main min-w-0${contentSized ? "" : " pg-stretch"}`}>
      <EntityTable accessory={accessory} className={section === "pg_stat_statements" ? "[&_.entity-cell]:text-sm" : undefined} columns={visibleColumns} contentSized={contentSized} contextLabel={activeContext?.label} empty={t("table.no_rows")} filterRows={section === "pg_locks" ? filterLockForest : undefined} requestPhase={requestPhase} finding={finding} findingField={finding === null || finding === undefined ? null : fieldNameForLocator(finding)} label={t(`pg.section.${sectionName(section)}`)} locale={locale} onContextClear={onContextClear} onNearEnd={densePageState === "idle" && canLoadMore ? onLoadMore : undefined} onOrder={onOrder} onPattern={onPattern} onSelect={(row) => { setSelected(row); onSelectedKey?.(rowKey(row)) }} order={activeOrder} pattern={pattern} rowLabel={section === "pg_locks" ? (row) => lockRowLabel(row, t) : undefined} searchRequest={searchRequest} searchSurface={section} serverSorted={dense} rows={rows} selectedKey={selectedRowKey} status={status} t={t} testId={`pg-${sectionName(section)}-table`} />
      {paging !== undefined && <div className="lens-tabs max-[760px]:w-full max-[760px]:[&>button]:min-w-0 max-[760px]:[&>button]:flex-1 max-[760px]:[&>button]:px-1" data-testid="table-paging">{paging}</div>}
    </div>
    {selected !== null && <InspectorPortal identity={`postgres:${section}:${rowKey(selected)}`} onClose={() => { setSelected(null); onSelectedKey?.(null) }} title={detailTitle(selected, section, t)}><PgDetail allRows={allRows} columns={visibleDetailColumns} cursor={cursor} historyField={selectedHistoryField} historyRevision={historyRevision} hour={Math.floor(cursor / 3_600_000_000) * 3_600_000_000} locale={locale} onCursor={onCursor} onRelated={onRelated} row={selected} section={section} segments={segments ?? NO_SEGMENTS} t={t} /></InspectorPortal>}
  </div>
}

export function tableState(
  metadata: SnapshotRows | undefined,
  rowCount: number,
  pattern: string | undefined,
  order: TableOrder | undefined,
  locale: Locale,
  t: Translate,
  time: Pick<DisplayTimeFormatter, "range" | "timestamp"> = createDisplayTimeFormatter(locale, "browser"),
  focusPreview: "loading" | "outside" | null = null,
): ReactNode {
  const count = (value: number) => new Intl.NumberFormat(locale).format(value)
  const shown = t("pg.table.shown", { "returned": count(rowCount), "eligible": count(metadata?.eligible ?? rowCount) })
  const semanticOrder = order?.column ?? null
  const serverOrder = metadata?.orderBy.join(", ") ?? null
  const intervalTimes = metadata === undefined || metadata.from === null || metadata.to === null ? null : time.range(metadata.from, metadata.to)
  const interval = intervalTimes === null
    ? t("pg.table.interval_unavailable")
    : t("pg.table.interval", intervalTimes)
  return <>
    {focusPreview !== null && <span>{t(`pg.table.focus_${focusPreview}`)}</span>}
    {focusPreview === null && <>
      <span>{interval}</span>
      <span>{pattern?.trim() ? t("pg.table.filter", { pattern: pattern.trim() }) : t("pg.table.no_filter")}</span>
      <span>{semanticOrder === null || serverOrder === null
        ? t("pg.table.order_default")
        : t("pg.table.order", { semantic: t(`pg.field.${semanticOrder}.label`), physical: serverOrder, direction: t(`pg.table.${metadata?.orderDirection ?? (order?.descending === false ? "asc" : "desc")}`) })}</span>
    </>}
    <strong>{focusPreview === null ? shown : t("pg.table.focus_exact")}</strong>
  </>
}

function visibleEntityColumns(columns: readonly EntityColumn[], rows: readonly DataRow[], rates: readonly string[]): readonly EntityColumn[] {
  return columns.filter((column) => rows.some((row) => column.available?.(row) ?? Object.hasOwn(row.values, typeof column.physicalField === "string" ? column.physicalField : column.field)))
    .map((column) => column.rate === true || rates.includes(column.field) ? { ...column, rate: true } : column)
}

function PgDetail({ allRows, columns, cursor, historyField, historyRevision, hour, locale, onCursor, onRelated, prelude, row, section, segments = NO_SEGMENTS, t }: { readonly allRows: readonly DataRow[]; readonly columns: readonly EntityColumn[]; readonly cursor: number; readonly historyField: string | null; readonly historyRevision: number; readonly hour: number; readonly locale: Locale; readonly onCursor: (timestamp: number) => void; readonly onRelated?: ((target: RelatedNavigation) => void) | undefined; readonly prelude?: ReactNode | undefined; readonly row: DataRow; readonly section: string; readonly segments?: readonly SegmentBound[] | undefined; readonly t: Translate }) {
  const entityRows = useMemo(() => allRows.filter((candidate) => sameEntity(candidate, row, section)), [allRows, row, section])
  const localHistoryRows = useMemo(() => [...entityRows.filter((candidate) => rowKey(candidate) !== rowKey(row)), row], [entityRows, row])
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  // Backend age is a slope-1 derived series, so it is not charted.
  const chartColumns = useMemo(() => columns.filter((column) => column.field !== "backend_age_ms" && chartableColumn(column)
    && (dense ? denseHistoryFields(row.typeId, column.field).length !== 0 : chartColumnAvailable(section, entityRows, column))), [columns, dense, entityRows, row.typeId, section])
  const preferredField = chartColumns.some(({ field }) => field === historyField) ? historyField : chartColumns[0]?.field ?? null
  const chartFields = chartColumns.map(({ field }) => field).join("\u0000")
  const [metricField, setMetricField] = useState(preferredField)
  useEffect(() => {
    setMetricField((current) => current !== null && chartFields.split("\u0000").includes(current) ? current : preferredField)
  }, [chartFields, preferredField])
  const activeMetricField = chartColumns.some(({ field }) => field === metricField) ? metricField : preferredField
  const historyColumn = chartColumns.find((column) => column.field === activeMetricField)
  const loadedHistory = useMemo(() => activeMetricField === null ? [] : buildMetricSamples(
    localHistoryRows,
    (candidate) => Object.hasOwn(candidate.values, activeMetricField) ? chartPointValue(value(candidate, activeMetricField), historyColumn) : undefined,
  ), [activeMetricField, historyColumn, localHistoryRows])
  const exactHistory = usePostgresMetricHistory(row, section, historyColumn, hour, historyRevision)
  const history = exactHistory.value?.length ? exactHistory.value : loadedHistory
  const textField = section === "pg_store_plans" ? "plan" : "query"
  const wholeText = useWholeText(row, section, textField)
  const exactText = wholeText?.trim() || null
  const planTarget = section === "pg_store_plans" ? statementsForPlan(row) : null
  const activityTarget = section === "pg_stat_activity" ? statementsForActivity(row) : null
  const statementTarget = section === "pg_stat_statements" ? plansForStatement(row) : null
  const backendPid = section === "pg_stat_activity" ? asNumber(value(row, "pid")) : null
  const [backendProcess, setBackendProcess] = useState<DataRow | null | undefined>(undefined)
  useEffect(() => {
    setBackendProcess(undefined)
    if (backendPid === null) return undefined
    const controller = new AbortController()
    acceptResponse(
      loadSnapshot(row.segmentId, cursor, [{ section: "os_process" }], controller.signal, undefined, { filters: { pid: String(backendPid) } }),
      controller.signal,
      (loaded) => setBackendProcess(loaded.sections.os_process?.[0] ?? null),
      () => setBackendProcess(null),
    )
    return () => controller.abort()
  }, [backendPid, cursor, row.segmentId])
  const fields = columns.filter((column) => column.field !== textField)
  const detailValue = (column: EntityColumn) => {
    if (section === "pg_stat_activity" && column.field === "query_id" && activityTarget !== null) {
      return <button aria-label={t("pg.related.open_statements", { id: activityTarget.queryId ?? "" })} className="cursor-pointer border-0 bg-transparent p-0 text-accent3 underline decoration-dotted underline-offset-2" onClick={() => onRelated?.(activityTarget)} type="button">{rawText(value(row, "query_id"))}</button>
    }
    return column.render === undefined ? display(value(row, column.field), column, locale, t) : column.render(row)
  }
  return <aside className="pg-detail" data-testid="pg-detail">
    <header className="pg-detail-head"><div className="min-w-0 flex-1"><span>{section === "pg_stat_progress_vacuum" ? section : t(`pg.section.${sectionName(section)}`)}</span><h2>{detailTitle(row, section, t)}</h2></div><div className="flex min-w-0 flex-none items-center gap-1.5">{section === "pg_stat_activity" && <button className="min-h-7 max-w-[200px] cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap rounded-[var(--radius-sm)] border border-accent-line bg-accent-soft px-2 text-left text-xs font-semibold text-accent3 transition-colors disabled:cursor-not-allowed disabled:border-line3 disabled:bg-s2 disabled:text-fg4" data-testid="pg-activity-related-statements" disabled={activityTarget === null} onClick={() => { if (activityTarget !== null) onRelated?.(activityTarget) }} title={activityTarget === null ? t("pg.activity.statements_unavailable") : t("pg.activity.open_statements", { query: activityTarget.queryId ?? "—" })} type="button">{t("pg.activity.open_statements", { query: activityTarget?.queryId ?? "—" })}</button>}{section === "pg_store_plans" && <button className="min-h-7 cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s2 px-2 text-xs font-medium text-accent3 transition-colors hover:bg-s3 disabled:cursor-not-allowed disabled:text-fg4" disabled={planTarget === null} onClick={() => { if (planTarget !== null) onRelated?.(planTarget) }} title={planTarget === null ? t("pg.plan.query_unavailable") : undefined} type="button">{t("pg.plan.open_query")}</button>}{section === "pg_stat_statements" && <button className="min-h-7 cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s2 px-2 text-xs font-medium text-accent3 transition-colors hover:bg-s3 disabled:cursor-not-allowed disabled:text-fg4" data-testid="pg-statement-related-plans" disabled={statementTarget === null} onClick={() => { if (statementTarget !== null) onRelated?.(statementTarget) }} type="button">{t("pg.statement.open_plans")}</button>}</div></header>
    {section === "pg_stat_activity" && activityTarget === null && <p className="m-0 border-b border-line2 px-2 py-1.5 text-xs text-fg3" data-testid="pg-activity-statements-unavailable">{t("pg.activity.statements_unavailable")}</p>}
    {prelude}
    {activeMetricField !== null && historyColumn !== undefined && <InspectorChartPortal identity={`pg:${section}:history`}><section className="process-history pg-metric-history grid min-w-0 gap-[7px]">
      <div aria-label={t("system.history")} className="history-selector flex max-w-full gap-[5px] overflow-x-auto p-px pb-[3px] [scrollbar-width:thin]" role="group">{chartColumns.map((column) => <button aria-pressed={activeMetricField === column.field} className="min-h-[28px] flex-none cursor-pointer rounded-[var(--radius-xs)] border border-line3 bg-s2 px-2 py-1 text-xs text-fg2 transition-colors hover:bg-s3 aria-pressed:border-accent aria-pressed:bg-accent-soft aria-pressed:text-fg" data-testid={`pg-chart-${column.field}`} key={column.field} onClick={() => setMetricField(column.field)} type="button">{t(column.label)}</button>)}</div>
      <SeriesChart cursor={cursor} durationAxis={durationKind(historyColumn.kind)} helpKey={historyColumn.help ?? "chart.metric.help"} hour={hour} labelKey={historyColumn.label} locale={locale} format={chartFormat(historyColumn.kind, columnDenominator(historyColumn, t))} onCursor={onCursor} points={history} scale={chartScale(historyColumn)} status={exactHistory.status} t={t} tickFormat={chartFormat(historyColumn.kind, columnDenominator(historyColumn, t))} unit={chartUnit(historyColumn, t("unit.per_second"))} />
    </section></InspectorChartPortal>}
    {section === "pg_store_plans"
      ? <PlanTextBlocks cursor={cursor} plan={wholeText} revision={historyRevision} row={row} segments={segments} t={t} />
      : exactText !== null && <section className="query-block"><span>{t("pg.query.label")}<button aria-label={t("common.raw")} className="inline-flex flex-none cursor-pointer items-center justify-center rounded-[var(--radius-xs)] border-0 bg-transparent p-1 text-accent3 transition-colors hover:bg-s3" onClick={() => void copyText(exactText, t("clipboard.manual"))} type="button"><Copy aria-hidden="true" size={12} /></button></span><pre data-testid="pg-exact-query">{exactText}</pre></section>}
    <DetailList>{fields.filter((column) => (column.available?.(row) ?? true) && told(value(row, column.field))).map((column) => <DetailRow key={column.field} term={column.help === undefined ? t(column.label) : <LabelHelp helpKey={column.help} labelKey={column.label} t={t} />} valueRole={detailValueRoleForColumn(column)}>{detailValue(column)}</DetailRow>)}</DetailList>
    {backendPid !== null && <InspectorRelatedPortal id="os_process" identity={`backend:${rowKey(row)}`} label={t("pg.related.process_tab")}>
      <ProcessFacts locale={locale} process={backendProcess} processTime={backendProcess?.timestamp ?? null} t={t} />
    </InspectorRelatedPortal>}
    {statementTarget !== null && onRelated !== undefined && <InspectorRelatedPortal id="pg_store_plans" identity={`statement:${rowKey(row)}`} label={t("pg.section.plans")}>
      <StatementPlansPanel cursor={cursor} expression={statementTarget.expression} locale={locale} onRelated={onRelated} segments={segments} t={t} />
    </InspectorRelatedPortal>}
    {planTarget !== null && onRelated !== undefined && <InspectorRelatedPortal id="pg_stat_statements" identity={`plan:${rowKey(row)}`} label={t("pg.section.statements")}>
      <PlanStatementPanel cursor={cursor} locale={locale} onRelated={onRelated} segments={segments} t={t} target={planTarget} />
    </InspectorRelatedPortal>}
  </aside>
}

const NO_SEGMENTS: readonly SegmentBound[] = []

function PlanTextBlocks({ cursor, plan, revision, row, segments, t }: { readonly cursor: number; readonly plan: string | null; readonly revision: number; readonly row: DataRow; readonly segments: readonly SegmentBound[]; readonly t: Translate }) {
  const query = usePlanQueryText(row, cursor, segments, revision)
  return <>
    <QueryView {...query} t={t} />
    <PlanView raw={plan} t={t} />
  </>
}

export function chartColumnAvailable(section: string, rows: readonly DataRow[], column: EntityColumn): boolean {
  if (rows.some((row) => Object.hasOwn(row.values, column.field))) return true
  if (section !== "pg_stat_activity") return false
  const source = activityDurationSource(column.field)
  return source !== null && rows.some((row) => Object.hasOwn(row.values, source))
}

export interface PostgresMetricHistoryRequest {
  readonly dense: boolean
  readonly durationField: string | null
  readonly field: string | null
  readonly fields: readonly string[]
  readonly filters: Readonly<Record<string, string>> | null
}

export function postgresMetricHistoryRequest(row: DataRow, section: string, column: EntityColumn): PostgresMetricHistoryRequest {
  const dense = section === "pg_stat_statements" || section === "pg_store_plans"
  const activity = section === "pg_stat_activity"
  const durationField = !dense && activity ? activityDurationSource(column.field) : null
  const field = dense ? null : typeof column.physicalField === "string"
    ? column.physicalField
    : column.physicalField?.[row.typeId] ?? column.field
  const metricFields = dense ? denseHistoryFields(row.typeId, column.field)
    : durationField === null ? [field!] : durationField === "query_start" || durationField === "state_change" ? ["state", durationField] : [durationField]
  const fields = activity ? ["pid", ...metricFields] : metricFields
  const identities = identityFields(section, row.typeId).map((name) => [name, rawText(value(row, name))] as const)
  return {
    dense,
    durationField,
    field,
    fields,
    filters: identities.some(([, stored]) => stored === null)
      ? null
      : Object.fromEntries(identities as readonly (readonly [string, string])[]),
  }
}

export function postgresMetricHistorySamples(rows: readonly DataRow[], row: DataRow, section: string, column: EntityColumn, request: PostgresMetricHistoryRequest): readonly ChartPoint[] {
  const activity = section === "pg_stat_activity"
  const entityHistory = rows.filter((candidate) => !activity || sameEntity(candidate, row, section))
  if (request.dense) return denseMetricHistory(entityHistory, row.typeId, column)
  if (request.durationField === null) {
    if (request.field === null) return []
    return postgresMetricHistory(entityHistory, { ...column, field: request.field }, column.rate === true)
  }
  return activityDurationHistory(entityHistory, column.field as Parameters<typeof activityDurationHistory>[1])
}

function usePostgresMetricHistory(row: DataRow, section: string, column: EntityColumn | undefined, hour: number, historyRevision: number): HistoryState<readonly ChartPoint[]> {
  const request = column === undefined ? null : postgresMetricHistoryRequest(row, section, column)
  const filters = request?.filters ?? null
  const ready = request !== null && request.fields.length !== 0 && filters !== null
  const target = ready ? JSON.stringify([hour, section, row.typeId, filters, column?.field]) : null
  return useHistoryRequest(target, historyRevision, !ready || column === undefined || request === null || filters === null ? null : async (signal) => {
    const rows = section === "pg_stat_activity"
      ? await loadSeries(hour, section, filters, request.fields, signal)
      : await loadSeries(hour, section, filters, request.fields, signal, row.typeId)
    return postgresMetricHistorySamples(rows, row, section, column, request)
  })
}

function historyFields(typeId: string, semantic: PostgresSemanticField): readonly string[] {
  if (semantic === "mean_exec_ms_per_call") {
    return uniqueText([
      physicalField(typeId, "calls_per_second"),
      physicalField(typeId, "execution_ms_per_second"),
    ])
  }
  return uniqueText([physicalField(typeId, semantic)])
}

const DENSE_DERIVED_FIELDS = new Set([
  "rows_per_call", "blocks_per_call", "hit_pct", "wal_per_call", "plan_time_pct", "cv",
  "min_exec_time_ms", "max_exec_time_ms", "mean_exec_time_ms", "stddev_exec_time_ms",
])

export function denseHistoryFields(typeId: string, field: string): readonly string[] {
  const available = new Set(registry.find((layout) => layout.typeId === typeId)?.columns ?? [])
  const present = (fields: readonly (string | null)[]) => uniqueText(fields.filter((name): name is string => name !== null && available.has(name)))
  const semantic = (...fields: readonly PostgresSemanticField[]) => present(fields.map((name) => physicalField(typeId, name)))
  if (isSemanticField(field)) return historyFields(typeId, field)
  if (field === "rows_per_call") return semantic("rows_per_second", "calls_per_second")
  if (field === "blocks_per_call") return present(["shared_blks_hit", "shared_blks_read", "local_blks_hit", "local_blks_read", ...semantic("calls_per_second")])
  if (field === "hit_pct") return present(["shared_blks_hit", "shared_blks_read"])
  if (field === "wal_per_call") return present(["wal_bytes", ...semantic("calls_per_second")])
  if (field === "plan_time_pct") return semantic("planning_ms_per_second", "execution_ms_per_second")
  if (DENSE_DERIVED_FIELDS.has(field)) return present(denseExecutionFields(typeId, field === "cv" ? ["mean", "stddev"] : [field.split("_")[0]! as "min" | "max" | "mean" | "stddev"]))
  return available.has(field) ? [field] : []
}

export function denseMetricHistory(rows: readonly DataRow[], typeId: string, column: EntityColumn): readonly ChartPoint[] {
  if (isSemanticField(column.field)) {
    const semantic = column.field
    return buildMetricSamples(postgresHistory(rows), (point) => Object.hasOwn(point, semantic) ? point[semantic] : undefined)
  }
  const ordered = rows.filter((row) => row.typeId === typeId).slice()
    .sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  const dependencies = denseHistoryFields(typeId, column.field)
  if (!DENSE_DERIVED_FIELDS.has(column.field)) {
    return postgresMetricHistory(ordered, column, column.rate === true)
  }
  return ordered.flatMap((row, index) => {
    const stored = denseDerivedValue(ordered, index, typeId, column.field, dependencies)
    return stored === undefined ? [] : [{ segmentId: row.segmentId, timestamp: row.timestamp, value: chartPointValue(stored, column) }]
  })
}

function denseDerivedValue(rows: readonly DataRow[], index: number, typeId: string, field: string, dependencies: readonly string[]): number | null | undefined {
  const row = rows[index]!
  const rate = (name: string): number | null | undefined => {
    if (!Object.hasOwn(row.values, name)) return undefined
    let earlier: DataRow | undefined
    for (let before = index - 1; before >= 0; before -= 1) {
      if (Object.hasOwn(rows[before]!.values, name)) { earlier = rows[before]; break }
    }
    return earlier === undefined || earlier.typeId !== row.typeId ? null : intervalMetric(earlier, row, name)
  }
  const gauge = (name: string): number | null | undefined => Object.hasOwn(row.values, name) ? asNumber(value(row, name)) : undefined
  const quotient = (left: number | null | undefined, right: number | null | undefined): number | null | undefined => left === undefined || right === undefined
    ? undefined : left === null || right === null || right <= 0 ? null : left / right
  const calls = physicalField(typeId, "calls_per_second")
  if (field === "rows_per_call") return quotient(rate(dependencies.find((name) => name !== calls) ?? ""), calls === null ? undefined : rate(calls))
  if (field === "blocks_per_call") {
    if (calls === null) return undefined
    const blocks = dependencies.filter((name) => name !== calls).map(rate)
    if (blocks.some((stored) => stored === undefined)) return undefined
    if (blocks.some((stored) => stored === null)) return null
    return quotient(blocks.reduce<number>((total, stored) => total + (stored ?? 0), 0), rate(calls))
  }
  if (field === "hit_pct") {
    const hit = rate("shared_blks_hit")
    const read = rate("shared_blks_read")
    if (hit === undefined || read === undefined) return undefined
    if (hit === null || read === null || hit + read <= 0) return null
    return 100 * hit / (hit + read)
  }
  if (field === "wal_per_call") return quotient(rate("wal_bytes"), calls === null ? undefined : rate(calls))
  if (field === "plan_time_pct") {
    const planning = physicalField(typeId, "planning_ms_per_second")
    const execution = physicalField(typeId, "execution_ms_per_second")
    const plan = planning === null ? undefined : rate(planning)
    const run = execution === null ? undefined : rate(execution)
    if (plan === undefined || run === undefined) return undefined
    if (plan === null || run === null || plan + run <= 0) return null
    return 100 * plan / (plan + run)
  }
  if (field === "cv") return quotient(gauge(dependencies[1] ?? ""), gauge(dependencies[0] ?? ""))
  return gauge(dependencies[0] ?? "")
}

function denseExecutionFields(typeId: string, names: readonly ("min" | "max" | "mean" | "stddev")[]): readonly string[] {
  const old = typeId === "1002001" || PG_STORE_PLANS_TYPE_IDS.includes(typeId as typeof PG_STORE_PLANS_TYPE_IDS[number])
  return names.map((name) => old ? `${name}_time` : `${name}_exec_time`)
}

function isSemanticField(field: string): field is PostgresSemanticField {
  return [
    "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second",
    "planning_ms_per_second", "shared_blk_read_ms_per_second", "shared_blk_write_ms_per_second",
    "local_blk_read_ms_per_second", "local_blk_write_ms_per_second",
    "temp_blk_read_ms_per_second", "temp_blk_write_ms_per_second",
  ].includes(field)
}

function uniqueText(values: readonly (string | null)[]): readonly string[] {
  return unique(values.filter((field): field is string => field !== null))
}

function useWholeText(row: DataRow, section: string, field: string): string | null {
  const shown = rawText(value(row, field))
  const [whole, setWhole] = useState<string | null>(null)
  useEffect(() => {
    setWhole(null)
    if (shown === null) return
    const controller = new AbortController()
    const filters = Object.fromEntries(identityFields(section, row.typeId)
      .map((name) => [name, rawText(value(row, name)) ?? ""]))
    acceptResponse(loadSnapshot(
      row.segmentId,
      row.timestamp,
      [{ section, fields: [field], typeId: row.typeId }],
      controller.signal,
      undefined,
      { filters, typeId: row.typeId, fullText: true },
    ), controller.signal, (data) => {
      const text = rawText(value(data.sections[section]?.[0] ?? row, field))
      if (text !== null) setWhole(text)
    })
    return () => controller.abort()
  }, [field, row, section, shown])
  return whole ?? shown
}

function told(cell: ReturnType<typeof value>): boolean {
  if (cell === null) return false
  return rawText(cell)?.trim() !== ""
}

function nonzeroId(cell: ReturnType<typeof value>): boolean {
  const id = rawText(cell)
  return id !== null && id !== "" && id !== "0"
}

export function sameEntity(left: DataRow, right: DataRow, section: string): boolean {
  if (section === "pg_stat_activity") {
    const pid = rawText(value(left, "pid"))
    return pid !== null && pid === rawText(value(right, "pid"))
  }
  const fields = identityFields(section, left.typeId)
  return left.typeId === right.typeId
    && fields.every((field) => rawText(value(left, field)) === rawText(value(right, field)))
}

export function selectedEntity(rows: readonly DataRow[], current: DataRow | null, section: string): DataRow | null {
  if (current !== null) {
    const advanced = rows.find((row) => sameEntity(row, current, section))
    if (advanced !== undefined) return advanced
  }
  return null
}

export function postgresDatabaseCount(rows: readonly DataRow[]): number {
  return rows.filter((row) => asNumber(value(row, "datid")) !== 0).length
}

export function postgresBlockSize(rows: readonly DataRow[], cursor: number): number | null {
  const row = snapshot(rows, cursor).find((candidate) => rawText(value(candidate, "name")) === "block_size")
  const size = row === undefined ? null : asNumber(value(row, "setting"))
  return size !== null && Number.isSafeInteger(size) && size > 0 ? size : null
}

export function postgresByteColumns(columns: readonly EntityColumn[], blockSize: number | null): readonly EntityColumn[] {
  return columns.map((column) => column.field === "blocks_per_call" || /(?:^|_)blks(?:_|$)/.test(column.field)
    ? { ...column, kind: "bytes", valueScale: blockSize }
    : column)
}

function identityFields(section: string, typeId?: string): readonly string[] {
  if (section === "pg_stat_progress_vacuum") return ["pid", "datid", "relid"]
  if (section === "pg_stat_activity" || section === "pg_locks") return ["pid"]
  if (section === "pg_stat_io") return ["backend_type", "object", "context"]
  if (section === "pg_prepared_xacts") return ["datname"]
  if ((section === "pg_stat_statements" || section === "pg_store_plans") && typeId !== undefined) return postgresIdentity(typeId)
  return registry.find((layout) => layout.typeId === typeId)?.identity ?? (section === "pg_stat_database" ? ["datid"] : [])
}

function detailTitle(row: DataRow, section: string, t: Translate): string {
  if (section === "pg_stat_activity" || section === "pg_stat_progress_vacuum" || section === "pg_locks") return t("pg.detail.pid", { pid: identifier(value(row, "pid")) })
  if (section === "pg_stat_statements") return t("pg.detail.query", { id: identifier(value(row, "queryid")) })
  if (section === "pg_store_plans") return t("pg.detail.plan", { id: identifier(value(row, "planid")) })
  if (section === "pg_stat_io") return ["backend_type", "object", "context"].flatMap((field) => rawText(value(row, field)) ?? []).join(" · ")
  if (section === "pg_prepared_xacts") return rawText(value(row, "datname")) ?? t("common.unavailable")
  return rawText(value(row, "datname")) ?? identifier(value(row, "datid"))
}

export function display(cell: ReturnType<typeof value>, column: EntityColumn, locale: Locale, t: Translate): ReactNode {
  if (cell === null) return "—"
  if (column.kind === "estimated_rows") return <EstimatedRows cell={cell} locale={locale} t={t} />
  if (column.field === "xid_age" || column.field === "mxid_age") return <span title={String(cell)}>{compact(asNumber(cell)!, locale)}</span>
  if (column.kind === "timestamp") {
    const timestamp = asNumber(cell)
    return timestamp === null ? "—" : <TimestampValue t={t} timestamp={timestamp} />
  }
  if (column.kind === "id") return rawText(cell) ?? "—"
  const per = t("unit.per_second")
  const number = asNumber(cell)
  const scaled = column.valueScale === null || column.valueScale !== undefined && number === null
    ? null
    : column.valueScale === undefined ? cell : number! * column.valueScale
  if (column.kind === "bytes") {
    const output = humanBytes(scaled, locale, columnDenominator(column, t))
    return <span title={output}>{output}</span>
  }
  if (column.kind === "kib") return unit(humanBytes(asNumber(cell) === null ? null : (asNumber(cell) ?? 0) * 1024, locale), column.rate, per)
  if (column.kind === "milliseconds" || column.kind === "duration") return humanDuration(cell, locale, "milliseconds", columnDenominator(column, t))
  if (column.kind === "microseconds") return humanDuration(cell, locale, "microseconds", columnDenominator(column, t))
  if (column.kind === "percent") return humanPercent(cell, locale, column.rate === true ? per : "")
  if (column.kind === "boolean" && typeof cell === "boolean") return locale === "ru" ? cell ? "да" : "нет" : String(cell)
  if (typeof cell === "number") return measure(cell, locale, unit("", column.rate, per))
  return rawText(cell) ?? "—"
}

function TimestampValue({ t, timestamp }: { readonly t: Translate; readonly timestamp: number }) {
  const time = useDisplayTime()
  return <span className="inline-flex items-center gap-[5px]"><span>{time.timestamp(timestamp)}</span><button aria-label={t("common.raw")} className="inline-flex cursor-pointer items-center justify-center border border-line4 bg-transparent px-[3px] py-0.5 text-accent3" onClick={() => void copyText(String(timestamp), t("clipboard.manual"))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
}

export function columnsFor(rows: readonly DataRow[]): readonly EntityColumn[] {
  const fields = [...new Set(rows.flatMap((row) => Object.keys(row.values)))].filter((field) => !isInternalField(field))
  return fields.map((field, index) => {
    const sample = rows.find((row) => value(row, field) !== null)
    const cell = sample === undefined ? null : value(sample, field)
    const kind: EntityColumn["kind"] = field === "pid" || field.endsWith("id") || field.endsWith("_id")
      ? "id"
      : isTimestampField(field)
        ? "timestamp"
        : field.endsWith("_time")
          ? "milliseconds"
          : field.endsWith("_us")
            ? "microseconds"
            : field.endsWith("_bytes")
              ? "bytes"
        : typeof cell === "number" ? "number" : typeof cell === "boolean" ? "boolean" : "text"
    return { field, label: field, kind, sticky: index === 0, width: kind === "text" ? 190 : kind === "timestamp" ? 210 : 135 }
  })
}

const INTERNAL_FIELDS = new Set(["ts", "ordinal", "segment_id", "type_id", "row_ordinal", "field_ordinal"])

function isInternalField(field: string): boolean {
  return INTERNAL_FIELDS.has(field)
}

function rowKey(row: DataRow): string { return `${row.segmentId}:${row.typeId}:${row.ordinal}` }
export function isTimestampField(field: string): boolean {
  return field.endsWith("_start") || field === "state_change" || field === "waitstart" || field === "stats_reset" || field === "stats_since"
}
function findingHistoryField(columns: readonly EntityColumn[], finding: Finding | null | undefined, fallback: string | null): string | null {
  const field = finding === null || finding === undefined ? null : fieldNameForLocator(finding)
  const semantic = finding === null || finding === undefined || field === null ? null : findingSemanticField(finding.typeId, field)
  const column = columns.find((candidate) => candidate.field === (semantic ?? field))
  return column === undefined || column.kind === "text" || column.kind === "timestamp" || column.kind === "boolean" ? fallback : column.field
}
const CHARTABLE_KINDS = new Set<EntityColumn["kind"]>(["number", "estimated_rows", "bytes", "kib", "milliseconds", "duration", "microseconds", "percent"])
export function chartableColumn(column: EntityColumn): boolean {
  return CHARTABLE_KINDS.has(column.kind)
}
export function chartScale(column: EntityColumn): "percent" | "nonnegative" {
  return column.kind === "percent" ? "percent" : "nonnegative"
}
export function chartUnit(column: EntityColumn, perSecond = "/s"): string {
  const per = column.rate === true ? perSecond : ""
  if (column.kind === "percent") return `%${per}`
  if (column.kind === "bytes" || column.kind === "kib") return ""
  if (durationKind(column.kind)) return per
  return column.rate === true ? perSecond : "count"
}
export function chartPointValue(cell: ReturnType<typeof value>, column: EntityColumn | undefined): number | null {
  const number = asNumber(cell)
  if (number === null) return null
  if (column?.valueScale === null) return null
  const scaled = column?.valueScale === undefined ? number : number * column.valueScale
  if (column?.kind === "kib") return scaled * 1024
  if (column?.kind === "microseconds") return scaled / 1_000
  return scaled
}
export function chartFormat(kind: EntityColumn["kind"], suffix = ""): ((value: number, locale: Locale) => string) | undefined {
  if (kind === "percent") return humanPercent
  if (kind === "bytes" || kind === "kib") return (value, locale) => humanBytes(value, locale, suffix)
  if (durationKind(kind)) return (value, locale) => humanDuration(value, locale, "milliseconds", suffix)
  return undefined
}

function durationKind(kind: EntityColumn["kind"]): boolean {
  return kind === "milliseconds" || kind === "duration" || kind === "microseconds"
}

function columnDenominator(column: Pick<EntityColumn, "field" | "rate">, t: Translate): string {
  if (column.field.endsWith("_per_call") || column.field === "blocks_per_call") return t("unit.per_call")
  return column.rate === true || column.field.endsWith("_per_second") ? t("unit.per_second") : ""
}
function sectionName(section: string): PostgresSection {
  if (section === "pg_stat_activity") return "activity"
  if (section === "pg_stat_progress_vacuum") return "vacuum"
  if (section === "pg_stat_statements") return "statements"
  if (section === "pg_store_plans") return "plans"
  if (section === "pg_locks") return "locks"
  return "databases"
}
function pgColumn(field: string, kind: NonNullable<EntityColumn["kind"]>, width: number, sticky = false, withHelp = true): EntityColumn {
  return { field, label: `pg.field.${field}.label`, ...(withHelp ? { help: `pg.field.${field}.help` } : {}), kind, width, sticky }
}
function text(field: string, width = 130, sticky = false, withHelp = true): EntityColumn { return pgColumn(field, "text", width, sticky, withHelp) }
function exactText(field: string, width = 130, sticky = false, withHelp = true): EntityColumn { return { ...text(field, width, sticky, withHelp), detailValueRole: "machine" } }
function number(field: string, width = 125): EntityColumn { return { ...pgColumn(field, "number", width), sortable: true } }
function id(field: string, width = 110, sticky = false, withHelp = true): EntityColumn { return { ...pgColumn(field, "id", width, sticky, withHelp), sortable: true } }
function bytes(field: string, width = 140): EntityColumn { return { ...pgColumn(field, "bytes", width), sortable: true } }
function milliseconds(field: string, width = 145): EntityColumn { return { ...pgColumn(field, "milliseconds", width), sortable: true } }
function duration(field: string, width = 145): EntityColumn { return pgColumn(field, "duration", width) }
function percent(field: string, width = 125): EntityColumn { return { ...pgColumn(field, "percent", width), sortable: true } }
function rateNumber(field: string, width = 125): EntityColumn { return { ...number(field, width), rate: true } }
function rateBytes(field: string, width = 140): EntityColumn { return { ...bytes(field, width), rate: true } }
function rateMilliseconds(field: string, width = 145): EntityColumn { return { ...milliseconds(field, width), rate: true } }
function timestamp(field: string, width = 210): EntityColumn { return { ...pgColumn(field, "timestamp", width), sortable: true } }
function boolean(field: string, width = 125): EntityColumn { return pgColumn(field, "boolean", width) }
function pgText(field: string, key: string, width = 130, sticky = false, withHelp = true): EntityColumn { return { field, label: `${key}.label`, ...(withHelp ? { help: `${key}.help` } : {}), kind: "text", width, sticky } }
function pgExactText(field: string, key: string, width = 130, sticky = false, withHelp = true): EntityColumn { return { ...pgText(field, key, width, sticky, withHelp), detailValueRole: "machine" } }
function pgNumber(field: string, key: string, width = 125): EntityColumn { return { field, label: `${key}.label`, help: `${key}.help`, kind: "number", width } }
function pgId(field: string, key: string, width = 110, sticky = false, withHelp = true): EntityColumn { return { field, label: `${key}.label`, ...(withHelp ? { help: `${key}.help` } : {}), kind: "id", width, sticky } }
