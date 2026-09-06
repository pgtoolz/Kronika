import { registry } from "kronika:registry"

import type { Cell, DataRow } from "./api"

export const PG_STAT_STATEMENTS_TYPE_IDS = [
  "1002001", "1002002", "1002003", "1002004", "1002005", "1002006",
] as const

export const PG_STORE_PLANS_TYPE_IDS = ["1003001", "1004001", "1018001"] as const

export type StatementLens = "load" | "per_call" | "io" | "resources" | "stability"
export type PlanLens = "load" | "timing" | "io" | "identity"

export type PostgresSemanticField =
  | "calls_per_second"
  | "execution_ms_per_second"
  | "mean_exec_ms_per_call"
  | "rows_per_second"
  | "planning_ms_per_second"
  | "shared_blk_read_ms_per_second"
  | "shared_blk_write_ms_per_second"
  | "local_blk_read_ms_per_second"
  | "local_blk_write_ms_per_second"
  | "temp_blk_read_ms_per_second"
  | "temp_blk_write_ms_per_second"

const SEMANTIC_FIELDS: readonly PostgresSemanticField[] = [
  "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second",
  "planning_ms_per_second", "shared_blk_read_ms_per_second", "shared_blk_write_ms_per_second",
  "local_blk_read_ms_per_second", "local_blk_write_ms_per_second",
  "temp_blk_read_ms_per_second", "temp_blk_write_ms_per_second",
]

type PhysicalAliases = Readonly<Partial<Record<PostgresSemanticField, string>>>

interface LayoutKind {
  readonly section: "pg_stat_statements" | "pg_store_plans" | "pg_store_plans_info"
  readonly aliases: PhysicalAliases
  readonly curated: readonly string[]
}

const BLOCK_COUNTERS = [
  "shared_blks_hit", "shared_blks_read", "shared_blks_dirtied", "shared_blks_written",
  "local_blks_hit", "local_blks_read", "local_blks_dirtied", "local_blks_written",
  "temp_blks_read", "temp_blks_written",
] as const

const DIRECT_ORDER_FIELDS = [
  ...BLOCK_COUNTERS, "plans", "wal_bytes", "wal_records", "wal_fpi", "wal_buffers_full",
  "slow_log_calls", "stats_since", "first_call", "last_call", "queryid", "queryid_stat_statements", "planid",
] as const

const STATEMENT_FIELDS = [
  "queryid", "userid", "dbid", "toplevel", "datname", "usename", "query",
  "calls", "rows", "plans", "total_time", "total_exec_time", "total_plan_time",
  ...BLOCK_COUNTERS,
  "blk_read_time", "blk_write_time", "shared_blk_read_time", "shared_blk_write_time",
  "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time",
  "wal_records", "wal_fpi", "wal_bytes", "wal_buffers_full",
  "min_time", "max_time", "mean_time", "stddev_time",
  "min_exec_time", "max_exec_time", "mean_exec_time", "stddev_exec_time", "stats_since",
] as const

const PLAN_FIELDS = [
  "userid", "dbid", "queryid", "planid", "queryid_stat_statements", "datname", "usename",
  "plan", "relids", "cmd_type", "calls", "slow_log_calls", "total_time", "rows", ...BLOCK_COUNTERS,
  "blk_read_time", "blk_write_time", "shared_blk_read_time", "shared_blk_write_time",
  "local_blk_read_time", "local_blk_write_time", "temp_blk_read_time", "temp_blk_write_time",
  "total_plan_time", "min_time", "max_time", "mean_time", "stddev_time", "first_call", "last_call",
] as const

const BASE_ALIASES = {
  calls_per_second: "calls",
  rows_per_second: "rows",
} as const

function statementAliases(
  execution: "total_time" | "total_exec_time",
  shared: "blk" | "shared",
  local: boolean,
  temp: boolean,
  planning: boolean,
): PhysicalAliases {
  return {
    ...BASE_ALIASES,
    execution_ms_per_second: execution,
    ...(planning ? { planning_ms_per_second: "total_plan_time" } : {}),
    shared_blk_read_ms_per_second: shared === "blk" ? "blk_read_time" : "shared_blk_read_time",
    shared_blk_write_ms_per_second: shared === "blk" ? "blk_write_time" : "shared_blk_write_time",
    ...(local ? {
      local_blk_read_ms_per_second: "local_blk_read_time",
      local_blk_write_ms_per_second: "local_blk_write_time",
    } : {}),
    ...(temp ? {
      temp_blk_read_ms_per_second: "temp_blk_read_time",
      temp_blk_write_ms_per_second: "temp_blk_write_time",
    } : {}),
  }
}

function planAliases(splitTimings: boolean, planning: boolean): PhysicalAliases {
  return {
    ...BASE_ALIASES,
    execution_ms_per_second: "total_time",
    ...(planning ? { planning_ms_per_second: "total_plan_time" } : {}),
    shared_blk_read_ms_per_second: splitTimings ? "shared_blk_read_time" : "blk_read_time",
    shared_blk_write_ms_per_second: splitTimings ? "shared_blk_write_time" : "blk_write_time",
    ...(splitTimings ? {
      local_blk_read_ms_per_second: "local_blk_read_time",
      local_blk_write_ms_per_second: "local_blk_write_time",
      temp_blk_read_ms_per_second: "temp_blk_read_time",
      temp_blk_write_ms_per_second: "temp_blk_write_time",
    } : {}),
  }
}

const LAYOUT_KINDS: Readonly<Record<string, LayoutKind>> = {
  "1002001": { section: "pg_stat_statements", aliases: statementAliases("total_time", "blk", false, false, false), curated: STATEMENT_FIELDS },
  "1002002": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "blk", false, false, true), curated: STATEMENT_FIELDS },
  "1002003": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "blk", false, false, true), curated: STATEMENT_FIELDS },
  "1002004": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "blk", false, true, true), curated: STATEMENT_FIELDS },
  "1002005": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "shared", true, true, true), curated: STATEMENT_FIELDS },
  "1002006": { section: "pg_stat_statements", aliases: statementAliases("total_exec_time", "shared", true, true, true), curated: STATEMENT_FIELDS },
  "1003001": { section: "pg_store_plans", aliases: planAliases(true, false), curated: PLAN_FIELDS },
  "1004001": { section: "pg_store_plans", aliases: planAliases(false, true), curated: PLAN_FIELDS },
  "1018001": { section: "pg_store_plans", aliases: planAliases(true, false), curated: PLAN_FIELDS },
  "1016001": { section: "pg_store_plans_info", aliases: {}, curated: ["dealloc", "stats_reset"] },
}

const REGISTRY_BY_TYPE_ID = new Map(registry.map((layout) => [layout.typeId, layout]))

export interface PostgresLayout {
  readonly typeId: string
  readonly section: LayoutKind["section"]
  readonly identity: readonly string[]
  readonly fields: readonly string[]
  readonly aliases: PhysicalAliases
}

export function postgresLayout(typeId: string): PostgresLayout | null {
  const kind = LAYOUT_KINDS[typeId]
  const layout = REGISTRY_BY_TYPE_ID.get(typeId)
  if (kind === undefined || layout === undefined || layout.logicalName !== kind.section) return null
  const available = new Set(layout.columns)
  return {
    typeId,
    section: kind.section,
    identity: layout.identity,
    fields: unique([...layout.identity, ...kind.curated]).filter((field) => available.has(field)),
    aliases: kind.aliases,
  }
}

export function postgresIdentity(typeId: string): readonly string[] {
  return postgresLayout(typeId)?.identity ?? []
}

export function postgresProjection(typeId: string): readonly string[] {
  return postgresLayout(typeId)?.fields ?? []
}

export function physicalField(typeId: string, semantic: PostgresSemanticField): string | null {
  if (semantic === "mean_exec_ms_per_call") return null
  const layout = postgresLayout(typeId)
  const physical = layout?.aliases[semantic]
  return physical !== undefined && layout?.fields.includes(physical) ? physical : null
}

export function physicalFields(
  typeIds: readonly string[],
  semantic: PostgresSemanticField,
): Readonly<Record<string, string>> {
  return Object.fromEntries(typeIds.flatMap((typeId) => {
    const physical = semantic === "mean_exec_ms_per_call"
      ? physicalField(typeId, "execution_ms_per_second")
      : physicalField(typeId, semantic)
    return physical === null ? [] : [[typeId, physical]]
  }))
}

export function findingSemanticField(typeId: string, physical: string): PostgresSemanticField | null {
  const execution = physicalField(typeId, "execution_ms_per_second")
  // Statement spikes use total-time/calls, but the locator names total time.
  if (physical === execution) return "mean_exec_ms_per_call"
  const layout = postgresLayout(typeId)
  if (layout === null) return null
  for (const [semantic, candidate] of Object.entries(layout.aliases)) {
    if (candidate === physical) return semantic as PostgresSemanticField
  }
  return null
}

export function supportsPostgresDerivedOrder(typeId: string, token: string): boolean {
  const field = token.startsWith("derived.") ? token.slice("derived.".length) : null
  if (field === null || !["mean_exec_ms_per_call", "rows_per_call", "blocks_per_call", "hit_pct", "wal_per_call", "plan_time_pct", "cv"].includes(field)) return false
  const layout = postgresLayout(typeId)
  if (layout === null) return false
  const has = (...names: string[]) => names.every((name) => layout.fields.includes(name))
  const semantic = (name: PostgresSemanticField) => physicalField(typeId, name) !== null
  if (field === "mean_exec_ms_per_call") return semantic("execution_ms_per_second") && semantic("calls_per_second")
  if (field === "rows_per_call") return semantic("rows_per_second") && semantic("calls_per_second")
  if (field === "blocks_per_call") return has("shared_blks_hit", "shared_blks_read", "local_blks_hit", "local_blks_read") && semantic("calls_per_second")
  if (field === "hit_pct") return has("shared_blks_hit", "shared_blks_read")
  if (field === "wal_per_call") return has("wal_bytes") && semantic("calls_per_second")
  if (field === "plan_time_pct") return semantic("planning_ms_per_second") && semantic("execution_ms_per_second")
  return has(...rowExecutionStats(typeId, ["mean", "stddev"]))
}

function fieldsByType(typeIds: readonly string[], wanted?: readonly string[]): Readonly<Record<string, readonly string[]>> {
  return Object.fromEntries(typeIds.map((typeId) => {
    const projection = postgresProjection(typeId)
    if (wanted === undefined) return [typeId, projection]
    const layout = postgresLayout(typeId)
    const physical = new Set(wanted.flatMap((field) => {
      if (isPostgresSemanticField(field)) {
        return unique([
          physicalField(typeId, field),
          ...(field === "mean_exec_ms_per_call" ? [physicalField(typeId, "calls_per_second")] : []),
        ].filter((candidate): candidate is string => candidate !== null))
      }
      return physicalDependencies(typeId, field)
    }))
    return [typeId, projection.filter((field) => layout?.identity.includes(field) === true || physical.has(field))]
  }))
}

function orderCandidates(typeIds: readonly string[], semantic: PostgresSemanticField): readonly string[] {
  if (semantic === "mean_exec_ms_per_call") return ["derived.mean_exec_ms_per_call"]
  return unique(typeIds.flatMap((typeId) => {
    const physical = physicalField(typeId, semantic)
    return physical === null ? [] : [physical]
  }))
}

function orderMap(typeIds: readonly string[]): Readonly<Record<string, readonly string[]>> {
  const semantic = Object.fromEntries(SEMANTIC_FIELDS.map((field) => [field, orderCandidates(typeIds, field)]))
  const derived = (field: string) => [`derived.${field}`]
  const executionStats = (names: readonly ("min" | "max" | "mean" | "stddev")[]) => unique(
    typeIds.flatMap((typeId) => rowExecutionStats(typeId, names)),
  )
  return {
    ...Object.fromEntries(DIRECT_ORDER_FIELDS.map((field) => [field, [field]])),
    ...semantic,
    rows_per_call: derived("rows_per_call"),
    blocks_per_call: derived("blocks_per_call"),
    hit_pct: derived("hit_pct"),
    wal_per_call: derived("wal_per_call"),
    plan_time_pct: derived("plan_time_pct"),
    cv: derived("cv"),
    min_exec_time_ms: executionStats(["min"]),
    max_exec_time_ms: executionStats(["max"]),
    mean_exec_time_ms: executionStats(["mean"]),
    stddev_exec_time_ms: executionStats(["stddev"]),
  }
}

function physicalDependencies(typeId: string, field: string): readonly string[] {
  const semantic = (...fields: PostgresSemanticField[]) => unique(fields.flatMap((candidate) => {
    const physical = physicalField(typeId, candidate)
    return physical === null ? [] : [physical]
  }))
  if (field === "mean_exec_ms_per_call") return semantic("execution_ms_per_second", "calls_per_second")
  if (field === "rows_per_call") return semantic("rows_per_second", "calls_per_second")
  if (field === "blocks_per_call") return ["shared_blks_hit", "shared_blks_read", "local_blks_hit", "local_blks_read", ...semantic("calls_per_second")]
  if (field === "hit_pct") return ["shared_blks_hit", "shared_blks_read"]
  if (field === "wal_per_call") return ["wal_bytes", ...semantic("calls_per_second")]
  if (field === "plan_time_pct") return semantic("planning_ms_per_second", "execution_ms_per_second")
  if (field === "cv") return rowExecutionStats(typeId, ["mean", "stddev"])
  if (field === "min_exec_time_ms") return rowExecutionStats(typeId, ["min"])
  if (field === "max_exec_time_ms") return rowExecutionStats(typeId, ["max"])
  if (field === "mean_exec_time_ms") return rowExecutionStats(typeId, ["mean"])
  if (field === "stddev_exec_time_ms") return rowExecutionStats(typeId, ["stddev"])
  return [field]
}

function rowExecutionStats(typeId: string, names: readonly ("min" | "max" | "mean" | "stddev")[]): readonly string[] {
  const old = typeId === "1002001" || PG_STORE_PLANS_TYPE_IDS.includes(typeId as typeof PG_STORE_PLANS_TYPE_IDS[number])
  return names.map((name) => old ? `${name}_time` : `${name}_exec_time`)
}

export interface PostgresSectionRequest {
  readonly section: LayoutKind["section"]
  readonly typeIds: readonly string[]
  readonly fieldsByType: Readonly<Record<string, readonly string[]>>
  readonly pageSize?: number
  readonly defaultOrder?: readonly string[]
  readonly order?: Readonly<Record<string, readonly string[]>>
  readonly fallbackOrder?: readonly string[]
}

const STATEMENT_LENS_FIELDS: Readonly<Record<StatementLens, readonly string[]>> = {
  load: ["queryid", "dbid", "userid", "toplevel", "datname", "usename", "query", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"],
  per_call: ["queryid", "dbid", "userid", "toplevel", "datname", "usename", "query", "calls_per_second", "execution_ms_per_second", "rows_per_second", "shared_blks_hit", "shared_blks_read", "local_blks_hit", "local_blks_read"],
  io: ["queryid", "dbid", "userid", "toplevel", "datname", "usename", "query", "calls_per_second", "shared_blks_read", "shared_blks_hit", "shared_blks_dirtied", "shared_blks_written", "local_blks_hit", "local_blks_read", "temp_blks_read", "temp_blks_written"],
  resources: ["queryid", "dbid", "userid", "toplevel", "datname", "usename", "query", "calls_per_second", "execution_ms_per_second", "planning_ms_per_second", "temp_blks_written", "wal_bytes", "wal_records", "wal_fpi", "wal_buffers_full"],
  stability: ["queryid", "dbid", "userid", "toplevel", "datname", "usename", "query", "calls_per_second", "mean_time", "stddev_time", "min_time", "max_time", "mean_exec_time", "stddev_exec_time", "min_exec_time", "max_exec_time"],
}

const PLAN_LENS_FIELDS: Readonly<Record<PlanLens, readonly string[]>> = {
  load: ["userid", "dbid", "queryid", "queryid_stat_statements", "planid", "datname", "usename", "plan", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_second"],
  timing: ["userid", "dbid", "queryid", "queryid_stat_statements", "planid", "datname", "usename", "plan", "calls_per_second", "min_time", "max_time", "mean_time", "stddev_time", "first_call", "last_call"],
  io: ["userid", "dbid", "queryid", "queryid_stat_statements", "planid", "datname", "usename", "plan", "calls_per_second", "shared_blks_read", "shared_blks_hit", "shared_blks_dirtied", "local_blks_hit", "local_blks_read", "temp_blks_read"],
  identity: ["userid", "dbid", "queryid", "planid", "datname", "usename", "plan", "calls", "calls_per_second", "cmd_type", "relids", "queryid_stat_statements"],
}

export function statementRequest(lens: StatementLens): PostgresSectionRequest {
  return denseLensRequest("pg_stat_statements", PG_STAT_STATEMENTS_TYPE_IDS, STATEMENT_LENS_FIELDS[lens], statementDefaultOrder(lens))
}

export function planRequest(lens: PlanLens): PostgresSectionRequest {
  return denseLensRequest("pg_store_plans", PG_STORE_PLANS_TYPE_IDS, PLAN_LENS_FIELDS[lens], planDefaultOrder(lens))
}

function denseLensRequest(
  section: "pg_stat_statements" | "pg_store_plans",
  typeIds: readonly string[],
  wanted: readonly string[],
  defaultSemantic: string,
): PostgresSectionRequest {
  const order = orderMap(typeIds)
  return {
    section,
    typeIds,
    fieldsByType: fieldsByType(typeIds, wanted),
    pageSize: 200,
    defaultOrder: order[defaultSemantic] ?? [],
    order,
    fallbackOrder: ["calls"],
  }
}

export function statementDefaultOrder(lens: StatementLens): string {
  if (lens === "per_call") return "calls_per_second"
  if (lens === "io") return "shared_blks_read"
  if (lens === "resources") return "wal_bytes"
  if (lens === "stability") return "calls_per_second"
  return "execution_ms_per_second"
}

export function planDefaultOrder(lens: PlanLens): string {
  if (lens === "io") return "shared_blks_read"
  if (lens === "load") return "execution_ms_per_second"
  return "calls_per_second"
}

function isPostgresSemanticField(field: string): field is PostgresSemanticField {
  return SEMANTIC_FIELDS.includes(field as PostgresSemanticField)
}

export interface PostgresIntervalValues extends Readonly<Record<PostgresSemanticField, number | null>> {
  readonly calls_per_second: number | null
  readonly execution_ms_per_second: number | null
  readonly mean_exec_ms_per_call: number | null
  readonly rows_per_second: number | null
  readonly planning_ms_per_second: number | null
  readonly shared_blk_read_ms_per_second: number | null
  readonly shared_blk_write_ms_per_second: number | null
  readonly local_blk_read_ms_per_second: number | null
  readonly local_blk_write_ms_per_second: number | null
  readonly temp_blk_read_ms_per_second: number | null
  readonly temp_blk_write_ms_per_second: number | null
}

export function decoratePostgresIntervalRow(row: DataRow): DataRow {
  const aliases = postgresLayout(row.typeId)?.aliases ?? {}
  const values: Record<string, Cell> = { ...row.values }
  for (const semantic of SEMANTIC_FIELDS) {
    if (semantic === "mean_exec_ms_per_call") continue
    const physical = aliases[semantic]
    if (Object.hasOwn(row.values, semantic)) values[semantic] = finiteRate(row.values[semantic])
    else if (physical !== undefined && Object.hasOwn(row.values, physical)) values[semantic] = finiteRate(row.values[physical])
  }
  const calls = finiteRate(values.calls_per_second)
  const execution = finiteRate(values.execution_ms_per_second)
  if (Object.hasOwn(values, "calls_per_second") && Object.hasOwn(values, "execution_ms_per_second")) {
    values.mean_exec_ms_per_call = calls !== null && calls > 0 && execution !== null && execution >= 0
      ? execution / calls
      : null
  }
  if (Object.hasOwn(values, "rows_per_second") && Object.hasOwn(values, "calls_per_second")) values.rows_per_call = ratio(values.rows_per_second, calls)
  const blockFields = ["shared_blks_hit", "shared_blks_read", "local_blks_hit", "local_blks_read"]
  const blockRates = blockFields.map((field) => finiteRate(values[field])).filter((cell): cell is number => cell !== null)
  if (Object.hasOwn(values, "calls_per_second") && blockFields.some((field) => Object.hasOwn(values, field))) {
    values.blocks_per_call = blockRates.length === 0 ? null : ratio(blockRates.reduce((total, cell) => total + cell, 0), calls)
  }
  const hits = finiteRate(values.shared_blks_hit)
  const reads = finiteRate(values.shared_blks_read)
  if (Object.hasOwn(values, "shared_blks_hit") && Object.hasOwn(values, "shared_blks_read")) values.hit_pct = hits === null || reads === null || hits + reads <= 0 ? null : (100 * hits) / (hits + reads)
  if (Object.hasOwn(values, "wal_bytes") && Object.hasOwn(values, "calls_per_second")) values.wal_per_call = ratio(values.wal_bytes, calls)
  const planning = finiteRate(values.planning_ms_per_second)
  if (Object.hasOwn(values, "planning_ms_per_second") && Object.hasOwn(values, "execution_ms_per_second")) values.plan_time_pct = planning === null || execution === null || planning + execution <= 0 ? null : (100 * planning) / (planning + execution)
  const oldNames = row.typeId === "1002001" || row.logicalName === "pg_store_plans"
  const executionStats = [["min_exec_time_ms", oldNames ? "min_time" : "min_exec_time"], ["max_exec_time_ms", oldNames ? "max_time" : "max_exec_time"], ["mean_exec_time_ms", oldNames ? "mean_time" : "mean_exec_time"], ["stddev_exec_time_ms", oldNames ? "stddev_time" : "stddev_exec_time"]] as const
  for (const [semantic, physical] of executionStats) if (Object.hasOwn(values, physical)) values[semantic] = finiteRate(values[physical])
  if (Object.hasOwn(values, "stddev_exec_time_ms") && Object.hasOwn(values, "mean_exec_time_ms")) values.cv = ratio(values.stddev_exec_time_ms, finiteRate(values.mean_exec_time_ms))
  return { ...row, values }
}

function ratio(numerator: Cell | undefined, denominator: number | null): number | null {
  const left = finiteRate(numerator)
  return left === null || denominator === null || denominator <= 0 ? null : left / denominator
}

export function exactCounterDelta(earlier: Cell, later: Cell): bigint | null {
  const before = exactInteger(earlier)
  const after = exactInteger(later)
  if (before === null || after === null || after < before) return null
  return after - before
}

export function intervalMetric(earlier: DataRow, later: DataRow, physical: string): number | null {
  const elapsed = later.timestamp - earlier.timestamp
  if (elapsed <= 0) return null
  const delta = nonnegativeDelta(earlier.values[physical] ?? null, later.values[physical] ?? null)
  if (delta === null) return null
  const rate = numeric(delta) / (elapsed / 1_000_000)
  return Number.isFinite(rate) ? rate : null
}

export function postgresInterval(earlier: DataRow, later: DataRow): PostgresIntervalValues {
  const layout = earlier.typeId === later.typeId ? postgresLayout(later.typeId) : null
  const rate = (semantic: PostgresSemanticField): number | null => {
    const physical = semantic === "mean_exec_ms_per_call" ? undefined : layout?.aliases[semantic]
    return physical === undefined ? null : intervalMetric(earlier, later, physical)
  }
  const callsPhysical = layout?.aliases.calls_per_second
  const executionPhysical = layout?.aliases.execution_ms_per_second
  const elapsed = later.timestamp - earlier.timestamp
  const calls = callsPhysical === undefined ? null : exactCounterDelta(
    earlier.values[callsPhysical] ?? null,
    later.values[callsPhysical] ?? null,
  )
  const execution = executionPhysical === undefined ? null : nonnegativeDelta(
    earlier.values[executionPhysical] ?? null,
    later.values[executionPhysical] ?? null,
  )
  const mean = elapsed > 0 && calls !== null && calls > 0n && execution !== null
    ? numeric(execution) / Number(calls)
    : null
  return {
    calls_per_second: rate("calls_per_second"),
    execution_ms_per_second: rate("execution_ms_per_second"),
    mean_exec_ms_per_call: mean !== null && Number.isFinite(mean) ? mean : null,
    rows_per_second: rate("rows_per_second"),
    planning_ms_per_second: rate("planning_ms_per_second"),
    shared_blk_read_ms_per_second: rate("shared_blk_read_ms_per_second"),
    shared_blk_write_ms_per_second: rate("shared_blk_write_ms_per_second"),
    local_blk_read_ms_per_second: rate("local_blk_read_ms_per_second"),
    local_blk_write_ms_per_second: rate("local_blk_write_ms_per_second"),
    temp_blk_read_ms_per_second: rate("temp_blk_read_ms_per_second"),
    temp_blk_write_ms_per_second: rate("temp_blk_write_ms_per_second"),
  }
}

export interface PostgresHistoryPoint extends PostgresIntervalValues {
  readonly timestamp: number
  readonly segmentId: string
}

export function postgresHistory(rows: readonly DataRow[]): readonly PostgresHistoryPoint[] {
  const ordered = [...rows].sort((left, right) => left.timestamp - right.timestamp)
  return ordered.map((row, index) => ({
    timestamp: row.timestamp,
    segmentId: row.segmentId,
    ...(index === 0 ? emptyInterval() : postgresInterval(ordered[index - 1] as DataRow, row)),
  }))
}

function emptyInterval(): PostgresIntervalValues {
  return Object.fromEntries(SEMANTIC_FIELDS.map((field) => [field, null])) as unknown as PostgresIntervalValues
}

function finiteRate(cell: Cell | undefined): number | null {
  return typeof cell === "number" && Number.isFinite(cell) ? cell : null
}

function exactInteger(cell: Cell): bigint | null {
  if (typeof cell === "string" && /^-?\d+$/.test(cell)) {
    try {
      return BigInt(cell)
    } catch {
      return null
    }
  }
  return typeof cell === "number" && Number.isSafeInteger(cell) ? BigInt(cell) : null
}

function nonnegativeDelta(earlier: Cell, later: Cell): bigint | number | null {
  const exactBefore = exactInteger(earlier)
  const exactAfter = exactInteger(later)
  if (exactBefore !== null && exactAfter !== null) {
    return exactAfter < exactBefore ? null : exactAfter - exactBefore
  }
  if (typeof earlier !== "number" || typeof later !== "number") return null
  const delta = later - earlier
  return delta >= 0 && Number.isFinite(delta) ? delta : null
}

function numeric(value: bigint | number): number {
  return typeof value === "bigint" ? Number(value) : value
}

export function unique<T>(values: readonly T[]): T[] {
  return [...new Set(values)]
}
