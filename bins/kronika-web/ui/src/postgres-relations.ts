import type { Cell, DataRow, SectionRequest, SnapshotOptions } from "./api"
import { intervalMetric } from "./postgres-metrics"

export const RELATION_SECTIONS = ["pg_stat_user_tables", "pg_stat_user_indexes"] as const
export const RELATION_GROUPS = ["database", "schema", "tablespace", "object"] as const
export const TABLE_LENSES = ["access", "changes", "maintenance", "size_buffers", "freeze"] as const
export const INDEX_LENSES = ["usage", "low_activity", "size_buffers", "state"] as const

export type RelationSection = typeof RELATION_SECTIONS[number]
export type RelationGroup = typeof RELATION_GROUPS[number]
export type TableLens = typeof TABLE_LENSES[number]
export type IndexLens = typeof INDEX_LENSES[number]
export type RelationLens = TableLens | IndexLens

export interface RelationWireColumn {
  readonly name: string
  readonly kind: "text" | "id" | "number" | "timestamp" | "bool"
  readonly unit: "none" | "count" | "bytes" | "per_second" | "percent" | "milliseconds"
  readonly nullable: boolean
}

export interface RelationLayout {
  readonly logicalName: RelationSection
  readonly group: RelationGroup
  readonly columns: readonly RelationWireColumn[]
}

/** An explicit logical row. Aggregate rows deliberately have no physical locator. */
export interface RelationRow {
  readonly group: RelationGroup
}

export interface RelationRequest extends SectionRequest {
  readonly group: RelationGroup
  readonly filters?: Readonly<Record<string, string>>
}

export interface RelationDetailTarget {
  readonly at: number
  readonly request: SectionRequest
  readonly options: SnapshotOptions
}

export interface RelationNavigation {
  readonly section: RelationSection
  readonly group: RelationGroup
  readonly filters: Readonly<Record<string, string>>
  readonly selectedKey: string | null
}

interface LensSpec {
  readonly object: readonly string[]
  readonly aggregate: readonly string[]
  readonly order: string
}

interface DisplayLensSpec {
  readonly object: readonly string[]
  readonly aggregate: readonly string[]
}

const scanTimes = timestamps("last_seq_scan", "last_idx_scan")
const maintenanceTimes = timestamps("last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum")
const indexScanTime = timestamps("last_idx_scan")

const tableLenses: Readonly<Record<TableLens, LensSpec>> = {
  access: lens(
    ["tuple_throughput", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", "last_seq_scan", "last_seq_scan_never", "last_idx_scan", "last_idx_scan_never"],
    ["tuple_throughput", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", ...scanTimes],
    "tuple_throughput",
  ),
  changes: lens(
    ["dml_total", "insert_share_pct", "update_share_pct", "delete_share_pct", "hot_pct", "new_page_pct", "dead_pct", "n_mod_since_analyze", "n_ins_since_vacuum"],
    undefined,
    "dml_total",
  ),
  maintenance: lens(
    ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", "last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum", "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", ...maintenanceTimes, "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    "autovacuum_count",
  ),
  size_buffers: lens(
    ["displayed_storage_bytes", "main_fork_bytes", "toast_bytes", "toast_share_pct", "reltuples", "toast_n_live_tup", "toast_n_dead_tup", "toast_dead_pct", "buffer_hit_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct", "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit"],
    undefined,
    "displayed_storage_bytes",
  ),
  freeze: lens(
    ["xid_age", "mxid_age", "n_ins_since_vacuum", "last_vacuum", "last_autovacuum"],
    ["xid_age", "mxid_age", "n_ins_since_vacuum", ...timestamps("last_vacuum", "last_autovacuum")],
    "xid_age",
  ),
}

const indexLenses: Readonly<Record<IndexLens, LensSpec>> = {
  usage: lens(
    ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", "last_idx_scan", "last_idx_scan_never"],
    ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", ...indexScanTime],
    "idx_scan",
  ),
  low_activity: lens(
    ["no_scans", "idx_scan", "last_idx_scan", "last_idx_scan_never", "main_fork_bytes"],
    ["no_scan_count", "known_scan_count", "idx_scan", ...indexScanTime, "main_fork_bytes"],
    "main_fork_bytes",
  ),
  size_buffers: lens(
    ["main_fork_bytes", "idx_blks_read", "idx_blks_hit", "buffer_hit_pct"],
    undefined,
    "main_fork_bytes",
  ),
  state: lens(
    ["state_severity", "indisvalid", "indisready", "indisunique", "indisprimary", "indisexclusion"],
    ["state_severity", "invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"],
    "state_severity",
  ),
}

const TABLE_HISTORY_DEPENDENCIES: Readonly<Record<string, readonly string[]>> = {
  sequential_share_pct: ["seq_scan", "idx_scan"],
  tuple_throughput: ["seq_tup_read", "idx_tup_fetch"],
  seq_tuples_per_scan: ["seq_tup_read", "seq_scan"],
  idx_tuples_per_scan: ["idx_tup_fetch", "idx_scan"],
  dml_total: ["n_tup_ins", "n_tup_upd", "n_tup_del"],
  insert_share_pct: ["n_tup_ins", "n_tup_upd", "n_tup_del"],
  update_share_pct: ["n_tup_ins", "n_tup_upd", "n_tup_del"],
  delete_share_pct: ["n_tup_ins", "n_tup_upd", "n_tup_del"],
  dead_pct: ["n_live_tup", "n_dead_tup"],
  hot_pct: ["n_tup_hot_upd", "n_tup_upd"],
  new_page_pct: ["n_tup_newpage_upd", "n_tup_upd"],
  displayed_storage_bytes: ["main_fork_bytes", "toast_bytes"],
  toast_share_pct: ["main_fork_bytes", "toast_bytes"],
  toast_dead_pct: ["toast_n_live_tup", "toast_n_dead_tup"],
  heap_buffer_hit_pct: ["heap_blks_read", "heap_blks_hit"],
  index_buffer_hit_pct: ["idx_blks_read", "idx_blks_hit"],
  toast_buffer_hit_pct: ["toast_blks_read", "toast_blks_hit"],
  tidx_buffer_hit_pct: ["tidx_blks_read", "tidx_blks_hit"],
  buffer_hit_pct: [
    "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit",
    "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit",
  ],
  vacuum_mean_ms: ["total_vacuum_time", "vacuum_count"],
  autovacuum_mean_ms: ["total_autovacuum_time", "autovacuum_count"],
  analyze_mean_ms: ["total_analyze_time", "analyze_count"],
  autoanalyze_mean_ms: ["total_autoanalyze_time", "autoanalyze_count"],
}

const INDEX_HISTORY_DEPENDENCIES: Readonly<Record<string, readonly string[]>> = {
  tuples_per_scan: ["idx_tup_read", "idx_scan"],
  fetches_per_scan: ["idx_tup_fetch", "idx_scan"],
  buffer_hit_pct: ["idx_blks_read", "idx_blks_hit"],
}

const TABLE_COUNTERS = new Set([
  "seq_scan", "seq_tup_read", "idx_scan", "idx_tup_fetch", "n_tup_ins", "n_tup_upd", "n_tup_del",
  "n_tup_hot_upd", "n_tup_newpage_upd", "vacuum_count", "autovacuum_count", "analyze_count",
  "autoanalyze_count", "total_vacuum_time", "total_autovacuum_time", "total_analyze_time",
  "total_autoanalyze_time", "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit",
  "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit",
])

const INDEX_COUNTERS = new Set(["idx_scan", "idx_tup_read", "idx_tup_fetch", "idx_blks_read", "idx_blks_hit"])

const tableDisplayLenses: Readonly<Record<TableLens, DisplayLensSpec>> = {
  access: {
    object: ["tuple_throughput", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", "tablespace", "last_seq_scan", "last_idx_scan"],
    aggregate: ["tuple_throughput", "table_count", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", "last_seq_scan_never_count", "last_idx_scan_never_count", "last_seq_scan_oldest", "last_seq_scan_latest", "last_idx_scan_oldest", "last_idx_scan_latest"],
  },
  changes: {
    object: ["dml_total", "insert_share_pct", "update_share_pct", "delete_share_pct", "hot_pct", "new_page_pct", "dead_pct", "n_mod_since_analyze", "n_ins_since_vacuum", "tablespace"],
    aggregate: ["dml_total", "table_count", "insert_share_pct", "update_share_pct", "delete_share_pct", "hot_pct", "new_page_pct", "dead_pct", "n_mod_since_analyze", "n_ins_since_vacuum"],
  },
  maintenance: {
    object: ["autovacuum_count", "vacuum_count", "autoanalyze_count", "analyze_count", "autovacuum_mean_ms", "vacuum_mean_ms", "autoanalyze_mean_ms", "analyze_mean_ms", "tablespace", "last_autovacuum", "last_vacuum", "last_autoanalyze", "last_analyze", "toast_last_autovacuum"],
    aggregate: ["autovacuum_count", "table_count", "vacuum_count", "autoanalyze_count", "analyze_count", "autovacuum_mean_ms", "vacuum_mean_ms", "autoanalyze_mean_ms", "analyze_mean_ms", "last_autovacuum_never_count", "last_vacuum_never_count", "last_autoanalyze_never_count", "last_analyze_never_count", "toast_last_autovacuum_never_count", "last_autovacuum_oldest", "last_autovacuum_latest", "last_vacuum_oldest", "last_vacuum_latest", "last_autoanalyze_oldest", "last_autoanalyze_latest", "last_analyze_oldest", "last_analyze_latest", "toast_last_autovacuum_oldest", "toast_last_autovacuum_latest"],
  },
  size_buffers: {
    object: ["displayed_storage_bytes", "buffer_hit_pct", "main_fork_bytes", "toast_share_pct", "reltuples", "toast_dead_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct", "tablespace"],
    aggregate: ["displayed_storage_bytes", "table_count", "buffer_hit_pct", "main_fork_bytes", "toast_share_pct", "reltuples", "toast_dead_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct"],
  },
  freeze: {
    object: ["xid_age", "mxid_age", "n_ins_since_vacuum", "tablespace", "last_autovacuum", "last_vacuum"],
    aggregate: ["xid_age", "table_count", "mxid_age", "n_ins_since_vacuum", "last_autovacuum_never_count", "last_vacuum_never_count", "last_autovacuum_oldest", "last_autovacuum_latest", "last_vacuum_oldest", "last_vacuum_latest"],
  },
}

const indexDisplayLenses: Readonly<Record<IndexLens, DisplayLensSpec>> = {
  usage: {
    object: ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", "amname", "tablespace", "last_idx_scan"],
    aggregate: ["idx_scan", "index_count", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", "last_idx_scan_never_count", "last_idx_scan_oldest", "last_idx_scan_latest"],
  },
  low_activity: {
    object: ["main_fork_bytes", "idx_scan", "amname", "tablespace", "no_scans", "last_idx_scan"],
    aggregate: ["main_fork_bytes", "index_count", "no_scan_count", "known_scan_count", "idx_scan", "last_idx_scan_never_count", "last_idx_scan_oldest", "last_idx_scan_latest"],
  },
  size_buffers: {
    object: ["main_fork_bytes", "buffer_hit_pct", "amname", "tablespace"],
    aggregate: ["main_fork_bytes", "index_count", "buffer_hit_pct"],
  },
  state: {
    object: ["amname", "tablespace", "indisvalid", "indisready", "indisprimary", "indisunique", "indisexclusion"],
    aggregate: ["invalid_count", "index_count", "unready_count", "primary_count", "unique_count", "exclusion_count"],
  },
}

export function relationFields(section: RelationSection, lensName: RelationLens, group: RelationGroup): readonly string[] {
  const spec = lensSpec(section, lensName)
  return [...identityFields(section, group), ...(group === "object" ? spec.object : spec.aggregate)]
}

export function relationDisplayFields(section: RelationSection, lensName: RelationLens, group: RelationGroup): readonly string[] {
  if (!isRelationLens(section, lensName)) invalid()
  const identity = group === "database"
    ? ["datname"]
    : group === "schema"
      ? ["schemaname", "datname"]
      : group === "tablespace"
        ? ["tablespace"]
      : section === "pg_stat_user_tables"
        ? ["relname", "datname", "schemaname"]
        : ["indexrelname", "datname", "schemaname", "relname"]
  const spec = section === "pg_stat_user_tables"
    ? tableDisplayLenses[lensName as TableLens]
    : indexDisplayLenses[lensName as IndexLens]
  if (group === "tablespace") return [...identity, spec.aggregate[1]!, spec.aggregate[0]!, ...spec.aggregate.slice(2)]
  return [...identity, ...(group === "object" ? spec.object : spec.aggregate)]
}

export function relationHelpKey(section: RelationSection, field: string): string | undefined {
  if (/(?:^|_)blks_(?:read|hit)$/.test(field)) return "pg.help.relation.buffer_bytes"
  if (["seq_scan", "idx_scan"].includes(field)) return "pg.help.relation.scan_rate"
  if (field === "idx_tup_read") return "pg.help.relation.index_entries_rate"
  if (field === "idx_tup_fetch") return "pg.help.relation.rows_fetched_rate"
  if (["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count"].includes(field)) return "pg.help.relation.maintenance_rate"
  if (["seq_tuples_per_scan", "idx_tuples_per_scan", "tuples_per_scan", "fetches_per_scan"].includes(field)) return "pg.help.relation.per_scan"
  if (["insert_share_pct", "update_share_pct", "delete_share_pct"].includes(field)) return "pg.help.relation.dml_share"
  if (["n_mod_since_analyze", "n_ins_since_vacuum"].includes(field)) return "pg.help.relation.row_estimate"
  if (["vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"].includes(field)) return "pg.help.relation.maintenance_mean"
  if (["buffer_hit_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct"].includes(field)) return "pg.help.relation.buffer_hit"
  if (["last_seq_scan", "last_idx_scan"].includes(field)) return "pg.help.relation.last_scan"
  if (["last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum"].includes(field)) return "pg.help.relation.last_maintenance"
  if (field.endsWith("_oldest")) return "pg.help.relation.oldest_timestamp"
  if (field.endsWith("_latest")) return "pg.help.relation.latest_timestamp"
  if (field.endsWith("_never_count")) return field.startsWith("toast_") ? "pg.help.relation.toast_never_count" : "pg.help.relation.never_count"
  if (["invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"].includes(field)) return "pg.help.relation.index_state_count"
  const fixed: Readonly<Record<string, string>> = {
    tuple_throughput: "pg.help.relation.tuple_throughput",
    sequential_share_pct: "pg.help.relation.sequential_share",
    dml_total: "pg.help.relation.dml_total",
    hot_pct: "pg.help.relation.hot_share",
    new_page_pct: "pg.help.relation.new_page_share",
    dead_pct: "pg.help.relation.dead_share",
    displayed_storage_bytes: "pg.help.relation.displayed_storage",
    main_fork_bytes: section === "pg_stat_user_tables" ? "pg.help.relation.table_data" : "pg.help.relation.index_data",
    toast_share_pct: "pg.help.relation.toast_share",
    reltuples: "pg.help.relation.reltuples",
    toast_dead_pct: "pg.help.relation.toast_dead_share",
    xid_age: "pg.help.relation.xid_age",
    mxid_age: "pg.help.relation.mxid_age",
    no_scans: "pg.help.relation.no_scans",
    no_scan_count: "pg.help.relation.no_scan_count",
    known_scan_count: "pg.help.relation.known_scan_count",
    indisvalid: "pg.help.relation.indisvalid",
    indisready: "pg.help.relation.indisready",
    indisprimary: "pg.help.relation.indisprimary",
    indisunique: "pg.help.relation.indisunique",
    indisexclusion: "pg.help.relation.indisexclusion",
  }
  return fixed[field]
}

export function relationDefaultOrder(section: RelationSection, lensName: RelationLens): string {
  return lensSpec(section, lensName).order
}

export function relationHistoryField(section: RelationSection, lensName: RelationLens): string {
  return relationDefaultOrder(section, lensName)
}

export function relationHistoryFields(section: RelationSection, field: string, physicalFields: readonly string[]): readonly string[] {
  const dependencies = (section === "pg_stat_user_tables" ? TABLE_HISTORY_DEPENDENCIES : INDEX_HISTORY_DEPENDENCIES)[field] ?? [field]
  return dependencies.every((name) => physicalFields.includes(name)) ? dependencies : []
}

export function relationSortToken(field: string): string {
  return field.endsWith("_pct") || field.endsWith("_per_scan") || field.endsWith("_mean_ms") || ["state_severity", "tuple_throughput", "dml_total", "displayed_storage_bytes"].includes(field) ? `derived.${field}` : field
}

const ESTIMATED_ROWS = new Set(["reltuples", "n_live_tup", "n_dead_tup", "toast_n_live_tup", "toast_n_dead_tup", "n_mod_since_analyze", "n_ins_since_vacuum"])

export function relationFieldKind(field: string): "id" | "text" | "boolean" | "timestamp" | "bytes" | "milliseconds" | "percent" | "estimated_rows" | "number" {
  if (ESTIMATED_ROWS.has(field)) return "estimated_rows"
  if (field === "no_scans" || field.startsWith("indis") || field.endsWith("_never")) return "boolean"
  if (field.endsWith("_never_count")) return "number"
  if (field.endsWith("id")) return "id"
  if (field.endsWith("name") || field === "tablespace" || field === "indexdef") return "text"
  if (field.includes("last_") || field.endsWith("_oldest") || field.endsWith("_latest")) return "timestamp"
  if (field.endsWith("_bytes")) return "bytes"
  if (field.endsWith("_mean_ms") || field.startsWith("total_") && field.endsWith("_time")) return "milliseconds"
  return field.endsWith("_pct") ? "percent" : "number"
}

export function relationRequest(section: RelationSection, lensName: RelationLens, group: RelationGroup): RelationRequest {
  const fields = relationFields(section, lensName, group)
  const order = Object.fromEntries(fields.filter((field) => {
    const kind = relationFieldKind(field)
    return kind !== "text" && kind !== "boolean" && !isRelationId(field)
  }).map((field) => [field, [relationSortToken(field)]]))
  const chosen = relationDefaultOrder(section, lensName)
  return {
    section,
    group,
    fields,
    pageSize: 200,
    defaultOrder: [relationSortToken(chosen)],
    order,
    ...(section === "pg_stat_user_indexes" && lensName === "low_activity" ? { filters: { no_scans: "true" } } : {}),
  }
}

export function isRelationSection(value: string): value is RelationSection {
  return (RELATION_SECTIONS as readonly string[]).includes(value)
}

export function isRelationGroup(value: unknown): value is RelationGroup {
  return typeof value === "string" && (RELATION_GROUPS as readonly string[]).includes(value)
}

export function isRelationLens(section: RelationSection, value: string): value is RelationLens {
  return (section === "pg_stat_user_tables" ? TABLE_LENSES : INDEX_LENSES as readonly RelationLens[]).includes(value as RelationLens)
}

export function parseRelationLayout(record: Readonly<Record<string, unknown>>): RelationLayout | null {
  if (record.record !== "relation_layout") return null
  const logicalName = relationSection(record.logical_name)
  const group = relationGroup(record.group)
  if (!Array.isArray(record.columns)) invalid()
  const seen = new Set<string>()
  const columns = record.columns.map((stored) => {
    const column = object(stored)
    const name = text(column.name)
    const kind = column.kind
    const unit = column.unit
    if (typeof kind !== "string" || !/^(text|id|number|timestamp|bool)$/.test(kind)
      || typeof unit !== "string" || !/^(none|count|bytes|per_second|percent|milliseconds)$/.test(unit)) invalid()
    if (typeof column.nullable !== "boolean" || seen.has(name)) invalid()
    seen.add(name)
    if (group !== "object" && name === "indexdef") invalid("aggregate index definition")
    return { name, kind, unit, nullable: column.nullable } as RelationWireColumn
  })
  return { logicalName, group, columns }
}

export function relationRateFields(layout: RelationLayout): readonly string[] {
  return layout.columns.flatMap((column) => column.unit === "per_second" ? [column.name] : [])
}

export function relationLayoutKey(layout: Pick<RelationLayout, "logicalName" | "group">): string {
  return `${layout.logicalName}:${layout.group}`
}

export function parseRelationRow(
  record: Readonly<Record<string, unknown>>,
  layouts: ReadonlyMap<string, RelationLayout>,
  segmentId: string,
  at = 0,
): DataRow | null {
  if (record.record !== "relation") return null
  const logicalName = relationSection(record.logical_name)
  const group = relationGroup(record.group)
  const layout = layouts.get(relationLayoutKey({ logicalName, group }))
  if (layout === undefined) invalid()
  const key = relationKey(logicalName, group, object(record.key))
  const stored = object(record.values)
  const expected = new Set(layout.columns.map(({ name }) => name))
  if (Object.keys(stored).some((name) => !expected.has(name)) || layout.columns.some(({ name }) => !Object.hasOwn(stored, name))) {
    invalid("relation values")
  }
  const metrics = Object.fromEntries(layout.columns.map((column) => {
    const value = stored[column.name] as Cell
    if (value === null && !column.nullable) invalid("relation null")
    return [column.name, value]
  }))
  if (Object.keys(key).some((name) => Object.hasOwn(metrics, name))) invalid()
  const source = record.source === null ? null : object(record.source)
  if ((group === "object") !== (source !== null)) invalid("relation source")
  const sampleFrom = moment(record.sample_from)
  const sampleTo = moment(record.sample_to)
  if (sampleFrom !== null && sampleTo !== null && sampleFrom > sampleTo) invalid("relation sample interval")
  return {
    segmentId: source === null ? segmentId : text(source.segment_id),
    logicalName,
    typeId: source === null ? "" : decimal(source.type_id),
    ordinal: source === null ? "" : decimal(source.ordinal),
    timestamp: source === null ? sampleTo ?? at : moment(source.timestamp) ?? invalid(),
    values: { ...key, ...metrics },
    relation: { group },
  }
}

export function relationRowKey(row: DataRow): string {
  const group = row.relation!.group
  return relationKeyString(row.logicalName as RelationSection, group, row.values)
}

export function isRelationId(field: string): boolean {
  return field === "datid" || field === "relid" || field === "indexrelid" || field === "tablespace_oid"
}

export function relationDetailTarget(row: DataRow): RelationDetailTarget {
  if (row.logicalName !== "pg_stat_user_indexes" || row.relation?.group !== "object" || row.typeId === "" || row.ordinal === "") {
    invalid("index definition source")
  }
  return {
    at: row.timestamp,
    request: { section: row.logicalName, typeId: row.typeId, fields: ["indexdef"] },
    options: {
      typeId: row.typeId,
      rowOrdinal: row.ordinal,
      fullText: true,
    },
  }
}

export function relationDrill(row: DataRow): RelationNavigation | null {
  const relation = row.relation
  if (relation === undefined || relation.group === "object") return null
  const filters = relation.group === "tablespace"
    ? { tablespace_oid: scalarText(row.values.tablespace_oid) }
    : relation.group === "database"
      ? { datid: scalarText(row.values.datid) }
      : { datid: scalarText(row.values.datid), schemaname: scalarText(row.values.schemaname) }
  return { section: row.logicalName as RelationSection, group: relation.group === "database" ? "schema" : "object", filters, selectedKey: null }
}

export function linkedRelation(row: DataRow): RelationNavigation | null {
  if (row.relation?.group !== "object") return null
  const datid = scalarText(row.values.datid)
  const relid = scalarText(row.values.relid)
  const section = row.logicalName === "pg_stat_user_tables" ? "pg_stat_user_indexes" : "pg_stat_user_tables"
  return {
    section,
    group: "object",
    filters: { datid, relid },
    selectedKey: section === "pg_stat_user_tables" ? relationKeyString(section, "object", row.values) : null,
  }
}

/** Builds exact gauges, reset-safe rates, and their fixed object-level DBA derivations. */
export function relationHistory(rows: readonly DataRow[], field: string): readonly { readonly segmentId: string; readonly timestamp: number; readonly value: number | null }[] {
  const ordered = [...rows].sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  return ordered.flatMap((row, index) => {
    const candidate = ordered[index - 1]
    const before = candidate !== undefined && sameHistoryObject(candidate, row) ? candidate : null
    const stored = historyValue(row, before, field)
    return stored === undefined ? [] : [{ segmentId: row.segmentId, timestamp: row.timestamp, value: stored }]
  })
}

function historyValue(row: DataRow, before: DataRow | null, field: string): number | null | undefined {
  if (row.logicalName === "pg_stat_user_tables") return tableHistoryValue(row, before, field)
  if (row.logicalName === "pg_stat_user_indexes") return indexHistoryValue(row, before, field)
  return undefined
}

function tableHistoryValue(row: DataRow, before: DataRow | null, field: string): number | null | undefined {
  switch (field) {
    case "sequential_share_pct": return rateRatio(row, before, ["seq_scan"], ["seq_scan", "idx_scan"], 100, true)
    case "tuple_throughput": return rateSum(row, before, ["seq_tup_read", "idx_tup_fetch"], true)
    case "seq_tuples_per_scan": return rateRatio(row, before, ["seq_tup_read"], ["seq_scan"])
    case "idx_tuples_per_scan": return rateRatio(row, before, ["idx_tup_fetch"], ["idx_scan"])
    case "dml_total": return rateSum(row, before, ["n_tup_ins", "n_tup_upd", "n_tup_del"])
    case "insert_share_pct": return rateRatio(row, before, ["n_tup_ins"], ["n_tup_ins", "n_tup_upd", "n_tup_del"], 100)
    case "update_share_pct": return rateRatio(row, before, ["n_tup_upd"], ["n_tup_ins", "n_tup_upd", "n_tup_del"], 100)
    case "delete_share_pct": return rateRatio(row, before, ["n_tup_del"], ["n_tup_ins", "n_tup_upd", "n_tup_del"], 100)
    case "dead_pct": return gaugeRatio(row, ["n_dead_tup"], ["n_live_tup", "n_dead_tup"], 100)
    case "hot_pct": return rateRatio(row, before, ["n_tup_hot_upd"], ["n_tup_upd"], 100)
    case "new_page_pct": return rateRatio(row, before, ["n_tup_newpage_upd"], ["n_tup_upd"], 100)
    case "displayed_storage_bytes": return gaugeSum(row, ["main_fork_bytes", "toast_bytes"], true)
    case "toast_share_pct": return gaugeRatio(row, ["toast_bytes"], ["main_fork_bytes", "toast_bytes"], 100, true)
    case "toast_dead_pct": return gaugeRatio(row, ["toast_n_dead_tup"], ["toast_n_live_tup", "toast_n_dead_tup"], 100)
    case "heap_buffer_hit_pct": return rateRatio(row, before, ["heap_blks_hit"], ["heap_blks_read", "heap_blks_hit"], 100)
    case "index_buffer_hit_pct": return rateRatio(row, before, ["idx_blks_hit"], ["idx_blks_read", "idx_blks_hit"], 100)
    case "toast_buffer_hit_pct": return rateRatio(row, before, ["toast_blks_hit"], ["toast_blks_read", "toast_blks_hit"], 100)
    case "tidx_buffer_hit_pct": return rateRatio(row, before, ["tidx_blks_hit"], ["tidx_blks_read", "tidx_blks_hit"], 100)
    case "buffer_hit_pct": return rateRatio(row, before,
      ["heap_blks_hit", "idx_blks_hit", "toast_blks_hit", "tidx_blks_hit"],
      ["heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit"], 100, true)
    case "vacuum_mean_ms": return rateRatio(row, before, ["total_vacuum_time"], ["vacuum_count"])
    case "autovacuum_mean_ms": return rateRatio(row, before, ["total_autovacuum_time"], ["autovacuum_count"])
    case "analyze_mean_ms": return rateRatio(row, before, ["total_analyze_time"], ["analyze_count"])
    case "autoanalyze_mean_ms": return rateRatio(row, before, ["total_autoanalyze_time"], ["autoanalyze_count"])
    default: return rawHistoryValue(row, before, field, TABLE_COUNTERS)
  }
}

function indexHistoryValue(row: DataRow, before: DataRow | null, field: string): number | null | undefined {
  if (field === "tuples_per_scan") return rateRatio(row, before, ["idx_tup_read"], ["idx_scan"])
  if (field === "fetches_per_scan") return rateRatio(row, before, ["idx_tup_fetch"], ["idx_scan"])
  if (field === "buffer_hit_pct") return rateRatio(row, before, ["idx_blks_hit"], ["idx_blks_read", "idx_blks_hit"], 100)
  return rawHistoryValue(row, before, field, INDEX_COUNTERS)
}

function rawHistoryValue(row: DataRow, before: DataRow | null, field: string, counters: ReadonlySet<string>): number | null | undefined {
  if (!Object.hasOwn(row.values, field)) return undefined
  if (field === "reltuples" && numeric(row.values[field]) === -1) return null
  if (!counters.has(field)) return numeric(row.values[field])
  return before === null || !Object.hasOwn(before.values, field) ? null : intervalMetric(before, row, field)
}

function rateRatio(row: DataRow, before: DataRow | null, numerator: readonly string[], denominator: readonly string[], scale = 1, neutral = false): number | null | undefined {
  return quotient(rateSum(row, before, numerator, neutral), rateSum(row, before, denominator, neutral), scale)
}

function gaugeRatio(row: DataRow, numerator: readonly string[], denominator: readonly string[], scale = 1, neutral = false): number | null | undefined {
  return quotient(gaugeSum(row, numerator, neutral), gaugeSum(row, denominator, neutral), scale)
}

function quotient(numerator: number | null | undefined, denominator: number | null | undefined, scale: number): number | null | undefined {
  if (numerator === undefined || denominator === undefined) return undefined
  if (numerator === null || denominator === null || denominator <= 0) return null
  const output = numerator / denominator * scale
  return Number.isFinite(output) ? output : null
}

function rateSum(row: DataRow, before: DataRow | null, fields: readonly string[], neutral = false): number | null | undefined {
  if (before === null) return fields.some((field) => !Object.hasOwn(row.values, field)) ? undefined : null
  return sumHistoryValues(fields.map((field) => {
    if (!Object.hasOwn(row.values, field)) return undefined
    if (!Object.hasOwn(before.values, field)) return null
    const current = row.values[field]
    const previous = before.values[field]
    if (neutral && current === null && previous === null) return 0
    return current === null || previous === null ? null : intervalMetric(before, row, field)
  }))
}

function gaugeSum(row: DataRow, fields: readonly string[], neutral = false): number | null | undefined {
  return sumHistoryValues(fields.map((field) => {
    if (!Object.hasOwn(row.values, field)) return undefined
    const value = numeric(row.values[field])
    return neutral && value === null ? 0 : value
  }))
}

function sumHistoryValues(values: readonly (number | null | undefined)[]): number | null | undefined {
  if (values.some((value) => value === undefined)) return undefined
  if (values.some((value) => value === null)) return null
  return (values as readonly number[]).reduce((total, value) => total + value, 0)
}

function sameHistoryObject(left: DataRow, right: DataRow): boolean {
  const object = right.logicalName === "pg_stat_user_tables" ? "relid" : "indexrelid"
  return left.typeId === right.typeId && left.logicalName === right.logicalName
    && JSON.stringify(left.values.datid) === JSON.stringify(right.values.datid)
    && JSON.stringify(left.values[object]) === JSON.stringify(right.values[object])
}

function timestamps(...names: readonly string[]): readonly string[] {
  return names.flatMap((name) => [`${name}_oldest`, `${name}_latest`, `${name}_never_count`])
}

function lens(objectFields: readonly string[], aggregateFields: readonly string[] | undefined, order: string): LensSpec {
  return { object: objectFields, aggregate: aggregateFields ?? objectFields, order }
}

function identityFields(section: RelationSection, group: RelationGroup): readonly string[] {
  const count = section === "pg_stat_user_tables" ? "table_count" : "index_count"
  if (group === "database") return ["datname", "datid", count]
  if (group === "schema") return ["schemaname", "datname", "datid", count]
  if (group === "tablespace") return ["tablespace", "tablespace_oid", count]
  return section === "pg_stat_user_tables"
    ? ["relname", "schemaname", "datname", "datid", "relid", "tablespace"]
    : ["indexrelname", "relname", "schemaname", "datname", "datid", "indexrelid", "relid", "tablespace", "amname"]
}

function lensSpec(section: RelationSection, value: RelationLens): LensSpec {
  if (!isRelationLens(section, value)) invalid()
  return section === "pg_stat_user_tables" ? tableLenses[value as TableLens] : indexLenses[value as IndexLens]
}

function relationSection(value: unknown): RelationSection {
  if (typeof value !== "string" || !isRelationSection(value)) invalid()
  return value
}

export function relationGroup(value: unknown): RelationGroup {
  if (!isRelationGroup(value)) invalid("relation group")
  return value
}

function relationKey(section: RelationSection, group: RelationGroup, stored: Readonly<Record<string, unknown>>): Readonly<Record<string, string>> {
  const names = relationKeyNames(section, group)
  if (Object.keys(stored).length !== names.length || names.some((name) => !Object.hasOwn(stored, name))) invalid("relation key")
  return Object.fromEntries(names.map((name) => [name, name.endsWith("id") ? decimal(stored[name]) : text(stored[name])]))
}

function relationKeyNames(section: RelationSection, group: RelationGroup): readonly string[] {
  return group === "database"
    ? ["datid", "datname"]
    : group === "schema"
      ? ["datid", "datname", "schemaname"]
      : group === "tablespace"
        ? ["tablespace_oid"]
      : section === "pg_stat_user_tables"
        ? ["datid", "datname", "schemaname", "relid", "relname"]
        : ["datid", "datname", "schemaname", "relid", "relname", "indexrelid", "indexrelname"]
}

function relationKeyString(section: RelationSection, group: RelationGroup, values: Readonly<Record<string, Cell>>): string {
  return JSON.stringify([section, group, ...relationKeyNames(section, group).map((name) => values[name])])
}

function moment(value: unknown): number | null {
  if (value === null) return null
  if (typeof value !== "string" || !/^-?\d+$/.test(value)) invalid()
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed)) invalid()
  return parsed
}

function object(value: unknown): Readonly<Record<string, unknown>> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) invalid()
  return value as Readonly<Record<string, unknown>>
}

function text(value: unknown): string {
  if (typeof value !== "string") invalid()
  return value
}

function scalarText(value: Cell | undefined): string {
  if (typeof value !== "string" && typeof value !== "number") invalid()
  return String(value)
}

function decimal(value: unknown): string {
  const stored = text(value)
  if (!/^\d+$/.test(stored)) invalid()
  return stored
}

function numeric(value: Cell | undefined): number | null {
  const stored = typeof value === "number" ? value : typeof value === "string" && value.trim() !== "" ? Number(value) : NaN
  return Number.isFinite(stored) ? stored : null
}

function invalid(message = "invalid relation response"): never {
  throw new Error(message)
}
