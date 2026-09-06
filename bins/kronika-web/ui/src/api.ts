import { registry } from "kronika:registry"

import { bundledFixtureHeatmapRecords, bundledFixtureHour, bundledFixtureRange } from "./fixture"
import type { HeatmapBand, HeatmapSummary, HeatmapView, HeatmapViewRow } from "./heatmap"
import { rowMatchesLocator } from "./locator"
import { decoratePostgresIntervalRow, intervalMetric, PG_STAT_STATEMENTS_TYPE_IDS, PG_STORE_PLANS_TYPE_IDS, postgresIdentity, supportsPostgresDerivedOrder, unique } from "./postgres-metrics"
import { parseRelationLayout, parseRelationRow, relationGroup, relationLayoutKey, relationRateFields, relationRowKey, type RelationGroup, type RelationLayout, type RelationRow } from "./postgres-relations"
import { apiFetch } from "./session"
import { readNdjson } from "./wire"
import { canonicalSearch } from "./search"
import type { EventEntry, EventStat, EventTier } from "./events-groups"

export type Cell = null | boolean | number | string | readonly number[] | { readonly [key: string]: unknown }

const REGISTRY_BY_TYPE_ID = new Map(registry.map((layout) => [layout.typeId, layout]))
const REGISTRY_LOGICAL_NAMES = unique(registry.flatMap((layout) =>
  layout.logicalName === null ? [] : [layout.logicalName],
))

const POSTGRESQL_OVERVIEW = [
  "pg_stat_bgwriter",
  "pg_stat_checkpointer",
  "pg_stat_io",
  "pg_prepared_xacts",
  "pg_stat_statements_info",
] as const

export const PRODUCT_SECTION_GROUPS = {
  host: REGISTRY_LOGICAL_NAMES.filter((name) => name === "instance_metadata" || name.startsWith("os_")),
  postgresqlOverview: [...POSTGRESQL_OVERVIEW, "pg_wal_storage", "pg_stat_wal", "pg_stat_archiver"] as const,
  postgresqlSettings: ["pg_settings"] as const,
  postgresqlActivity: ["pg_stat_activity"] as const,
  postgresqlVacuum: ["pg_stat_progress_vacuum", "pg_stat_activity"] as const,
  postgresqlStatements: ["pg_stat_statements"] as const,
  postgresqlPlans: ["pg_store_plans", "pg_store_plans_info"] as const,
  postgresqlLocks: ["pg_locks"] as const,
  postgresqlDatabases: ["pg_stat_database"] as const,
  postgresqlTables: ["pg_stat_user_tables"] as const,
  postgresqlIndexes: ["pg_stat_user_indexes"] as const,
  events: REGISTRY_LOGICAL_NAMES.filter((name) => name.startsWith("pg_log_") || name === "pgbouncer_events"),
} as const

const UI_SECTION_NAMES = unique(Object.values(PRODUCT_SECTION_GROUPS).flat())
const UI_SECTION_NAME_SET = new Set(UI_SECTION_NAMES)

export interface SectionRequest {
  readonly section: string
  readonly fields?: readonly string[]
  readonly typeIds?: readonly string[]
  readonly fieldsByType?: Readonly<Record<string, readonly string[]>>
  readonly typeId?: string
  readonly pageSize?: number
  readonly defaultOrder?: readonly string[]
  readonly order?: Readonly<Record<string, readonly string[]>>
  readonly fallbackOrder?: readonly string[]
  readonly group?: RelationGroup
  readonly filters?: Readonly<Record<string, string>>
}

export const POSTGRESQL_CONTEXT_REQUESTS: readonly SectionRequest[] = [
  { section: "pg_settings", fields: ["name", "setting"] },
]

export const TIMELINE_REQUESTS: readonly SectionRequest[] = [{ section: "health" }]

export interface DataRow {
  readonly segmentId: string
  readonly logicalName: string
  readonly typeId: string
  readonly ordinal: string
  readonly timestamp: number
  readonly values: Readonly<Record<string, Cell>>
  readonly relation?: RelationRow
}

export interface Point {
  readonly segmentId: string
  readonly logicalName: string
  readonly typeId: string
  readonly series: string
  readonly timestamp: number
  readonly identity: Readonly<Record<string, Cell>>
  readonly value: number | null
}

export interface Finding {
  readonly segmentId: string
  readonly logicalName: string
  readonly kind: "known_bad" | "spike" | "event"
  readonly typeId: string
  readonly timestamp: number
  readonly category: number | null
  readonly rowOrdinal: string
  readonly fieldOrdinal: number
}

export interface FindingGroup {
  readonly segmentId: string
  readonly logicalName: string
  readonly typeId: string
  readonly totalHits: number
  readonly shown: number
  readonly truncated: boolean
}

interface Section {
  readonly logical_name: string | null
  readonly type_id: string
}

interface Segment {
  readonly id: string
  readonly min_ts: string
  readonly max_ts: string
  readonly cursor?: {
    readonly segment_id: string
    readonly wal_position: string
  }
  readonly sections: readonly Section[]
}

export interface HourData {
  readonly sections: Readonly<Record<string, readonly DataRow[]>>
  readonly rateColumns: Readonly<Record<string, readonly string[]>>
  readonly snapshotRows: readonly SnapshotRows[]
  readonly availableSections: readonly string[]
  readonly syntheticDemo?: boolean
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
  readonly processes: readonly DataRow[]
  readonly activities: readonly DataRow[]
  readonly load: readonly DataRow[]
  readonly memory: readonly DataRow[]
  readonly pressure: readonly DataRow[]
  readonly health: readonly DataRow[]
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly laneContexts: readonly LaneContext[]
  readonly findings: readonly Finding[]
  readonly findingGroups: readonly FindingGroup[]
}

export interface SnapshotRows {
  readonly logicalName: string
  readonly eligible: number
  /// Rows the requested statement scope kept off the page.
  readonly excluded: number
  readonly returned: number
  readonly hasMore: boolean
  readonly truncated: boolean
  readonly nextCursor: string | null
  readonly pageSize: number
  readonly orderBy: readonly string[]
  readonly orderDirection: "asc" | "desc"
  readonly from: number | null
  readonly to: number | null
  readonly group?: RelationGroup
}

export function snapshotRowKey(row: DataRow): string {
  if (row.relation !== undefined) return `${row.segmentId}:${relationRowKey(row)}`
  return `${row.segmentId}:${row.typeId}:${row.ordinal}`
}

export function appendSnapshotRows(
  current: readonly DataRow[],
  incoming: readonly DataRow[],
): readonly DataRow[] {
  const seen = new Set(current.map(snapshotRowKey))
  return [...current, ...incoming.filter((row) => {
    const key = snapshotRowKey(row)
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })]
}

export function mergeSnapshotData(current: HourData, incoming: HourData, appendSection?: string): HourData {
  const sections: Record<string, readonly DataRow[]> = { ...current.sections }
  for (const [logicalName, rows] of Object.entries(incoming.sections)) {
    sections[logicalName] = appendSnapshotRows(current.sections[logicalName] ?? [], rows)
  }
  if (appendSection !== undefined && incoming.sections[appendSection] === undefined) {
    sections[appendSection] = current.sections[appendSection] ?? []
  }
  const rateColumns: Record<string, readonly string[]> = { ...current.rateColumns }
  for (const [logicalName, columns] of Object.entries(incoming.rateColumns)) {
    rateColumns[logicalName] = unique([...(rateColumns[logicalName] ?? []), ...columns])
  }
  return hourData({
    sections,
    rateColumns,
    snapshotRows: incoming.snapshotRows.length === 0 ? current.snapshotRows : incoming.snapshotRows,
    availableSections: unique([...current.availableSections, ...incoming.availableSections]),
    syntheticDemo: current.syntheticDemo === true || incoming.syntheticDemo === true,
    postgresqlConfigured: current.postgresqlConfigured === true || incoming.postgresqlConfigured === true,
    postgresqlPresent: current.postgresqlPresent === true || incoming.postgresqlPresent === true,
    points: [],
    lanePoints: [],
    laneContexts: [],
    findings: [],
  })
}

export interface LanePoint {
  readonly segmentId: string
  readonly lane: string
  readonly timestamp: number
  readonly value: number | null
}

export interface LaneContext {
  readonly segmentId: string
  readonly postgresqlIntervalSeconds: number | null
  // instance_metadata.environment of the segment: 0 machine, 1 container.
  readonly environment: number | null
}

export type RecordedEnvironment = "machine" | "container"

export function recordedEnvironmentFromContexts(contexts: readonly LaneContext[]): RecordedEnvironment | null {
  const stated = contexts.filter((context) => context.environment !== null).at(-1)
  return stated === undefined ? null : stated.environment === 0 ? "machine" : stated.environment === 1 ? "container" : null
}

export interface TimelineLanes {
  readonly contexts: readonly LaneContext[]
  readonly points: readonly LanePoint[]
}

export interface TimelineData {
  readonly hour: number
  readonly lanePoints: readonly LanePoint[]
  readonly laneContexts: readonly LaneContext[]
  readonly lanes: Readonly<Record<string, readonly DataRow[]>>
  readonly availableHours: readonly number[]
  readonly segments: readonly SegmentBound[]
  readonly health: readonly DataRow[]
  readonly points: readonly Point[]
  readonly findings: readonly Finding[]
  readonly findingGroups: readonly FindingGroup[]
  readonly availableSections: readonly string[]
  readonly syntheticDemo?: boolean
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
}

export interface TimelineRange {
  readonly from: number
  readonly toExclusive: number
}

export interface ResolvedLocator {
  readonly logicalName: string
  readonly row: DataRow
  readonly fieldName: string | null
}

export const ACTIVITY_FIELDS = [
  "pid", "leader_pid", "datid", "datname", "usename", "application_name", "client_addr", "backend_type",
  "state", "wait_event_type", "wait_event", "query", "query_id", "backend_xid_age",
  "backend_xmin_age", "backend_start", "xact_start", "query_start", "state_change",
] as const

export function hourOf(timeline: TimelineData): HourData {
  return hourData({
    sections: timeline.lanes,
    availableSections: timeline.availableSections,
    syntheticDemo: timeline.syntheticDemo ?? false,
    postgresqlConfigured: timeline.postgresqlConfigured ?? false,
    postgresqlPresent: timeline.postgresqlPresent ?? false,
    points: timeline.points,
    lanePoints: timeline.lanePoints,
    laneContexts: timeline.laneContexts,
    findings: timeline.findings,
    findingGroups: timeline.findingGroups,
  })
}

export function sampleAt(line: readonly DataRow[], cursor: number): number | null {
  let chosen: number | null = null
  for (const row of line) {
    if (row.timestamp <= cursor && (chosen === null || row.timestamp > chosen)) chosen = row.timestamp
  }
  return chosen ?? (line.length === 0 ? null : Math.min(...line.map((row) => row.timestamp)))
}

export function viewData(timeline: HourData, current: HourData): HourData {
  return hourData({
    sections: { ...timeline.sections, ...current.sections },
    rateColumns: current.rateColumns,
    snapshotRows: current.snapshotRows,
    availableSections: timeline.availableSections,
    syntheticDemo: timeline.syntheticDemo ?? false,
    postgresqlConfigured: timeline.postgresqlConfigured ?? false,
    postgresqlPresent: timeline.postgresqlPresent ?? false,
    points: timeline.points,
    lanePoints: timeline.lanePoints,
    laneContexts: timeline.laneContexts,
    findings: timeline.findings,
    findingGroups: timeline.findingGroups,
  })
}

export interface SegmentBound {
  readonly id: string
  readonly minTs: number
  readonly maxTs: number
  readonly activeWalPosition?: string
  readonly sections: readonly SegmentSection[]
}

export interface SegmentSection {
  readonly logicalName: string
  readonly typeId: string
}

export interface SnapshotRequestGroup {
  readonly anchor: SegmentBound
  readonly requests: readonly SectionRequest[]
}

export async function loadTimeline(
  start: number | null,
  signal: AbortSignal,
  onBytes?: (received: number) => void,
  visibleRange: TimelineRange | null = null,
): Promise<TimelineData> {
  const range = bundledFixtureRange()
  const requested = floorHour(start ?? range?.from ?? 0)
  const fixture = bundledFixtureHour(requested)
  if (fixture !== null && range !== null) {
    return {
      hour: requested, availableHours: unique([floorHour(range.from), floorHour(range.to)]),
      segments: [], lanePoints: fixture.lanePoints, laneContexts: [], lanes: fixture.sections, health: fixture.health, points: fixture.points,
      findings: fixture.findings,
      findingGroups: fixture.findingGroups,
      availableSections: fixture.availableSections,
      syntheticDemo: false,
      postgresqlConfigured: fixture.availableSections.some((name) => name.startsWith("pg_")),
      postgresqlPresent: fixture.availableSections.some((name) => name.startsWith("pg_") && !name.startsWith("pg_log_")),
    }
  }
  const query = new URLSearchParams({ part: "base" })
  if (start !== null || visibleRange !== null) {
    const window = timelineWindow(requested, visibleRange)
    query.set("from", String(window.from))
    query.set("to", String(window.toExclusive - 1))
  }
  const records = await request(`/api/hour?${query}`, signal, onBytes)
  const header = records.find((record) => record.record === "hour")
  const catalog = records.find((record) => record.record === "catalog")
  const hour = header?.from === null || header?.from === undefined
    ? floorHour(Date.now() * 1_000)
    : floorHour(integer(header.from, "hour start"))
  const end = hour + 3_600_000_000
  const all = catalogSegments(records)
  const segments = all
    .filter((segment) => Number(segment.min_ts) < end && Number(segment.max_ts) >= hour)
    .map((segment) => ({
      id: segment.id,
      minTs: Number(segment.min_ts),
      maxTs: Number(segment.max_ts),
      ...(segment.cursor === undefined ? {} : {
        activeWalPosition: activePosition(segment),
      }),
      sections: segment["sections"].flatMap((section) => section.logical_name === null ? [] : [{
        logicalName: section.logical_name,
        typeId: section.type_id,
      }]),
    }))
  const points: Point[] = []
  const findings: Finding[] = []
  const findingGroups: FindingGroup[] = []
  const lanePoints: LanePoint[] = []
  const lanes: Record<string, DataRow[]> = { [HEALTH]: [] }
  const layouts = new Map<string, RowLayout>()
  let segmentId = ""
  for (const record of records) {
    const layout = layoutRecord(record)
    if (record.record === "index") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "index segment id")
    } else if (record.record === "point") {
      points.push(indexPoint(record, segmentId, HEALTH))
    } else if (isFindingRecord(record)) {
      findings.push(indexFinding(record, segmentId, HEALTH))
    } else if (record.record === "findings") {
      const typeId = requiredText(record.type_id, "finding group type_id")
      const logicalName = typeof record.logical_name === "string"
        ? record.logical_name
        : logicalNameForTypeId(typeId) ?? HEALTH
      const totalHits = integer(record.total_hits, "finding hit count")
      if (typeof record["truncated"] !== "boolean") throw new Error("finding truncation flag is invalid")
      findingGroups.push({
        segmentId,
        logicalName,
        typeId,
        totalHits,
        shown: 0,
        truncated: record["truncated"],
      })
    } else if (layout !== null) {
      layouts.set(layout.typeId, layout)
    } else if (record.record === "row") {
      const row = laneRow(record, segmentId, layouts)
      if (row !== null) (lanes[row.logicalName] ??= []).push(row)
    }
  }
  lanes[HEALTH] = healthRows(points) as DataRow[]
  const resolvedFindingGroups = findingGroups.map((group) => ({
    ...group,
    shown: findings.filter((finding) => finding.segmentId === group.segmentId && finding.typeId === group.typeId).length,
  }))
  return {
    hour,
    lanePoints,
    laneContexts: [],
    lanes,
    availableHours: ((header?.available_hours ?? []) as readonly string[])
      .map((value) => integer(value, "available hour")),
    segments,
    health: lanes[HEALTH] ?? [],
    points,
    findings,
    findingGroups: resolvedFindingGroups,
    availableSections: availableSectionNames(all),
    syntheticDemo: catalog?.demo === "synthetic",
    postgresqlConfigured: sourceConfigured(catalog, "postgresql"),
    postgresqlPresent: sourceMetricsPresent(catalog, "postgresql"),
  }
}

export async function loadTimelineLanes(
  timeline: Pick<TimelineData, "hour" | "segments">,
  signal: AbortSignal,
  visibleRange: TimelineRange | null = null,
): Promise<TimelineLanes> {
  const fixture = bundledFixtureHour(timeline.hour)
  if (fixture !== null) return { contexts: [], points: fixture.lanePoints }
  const window = timelineWindow(timeline.hour, visibleRange)
  const query = new URLSearchParams({
    from: String(window.from),
    to: String(window.toExclusive - 1),
    part: "lanes",
    segments: timeline.segments.map((segment) => segment.id).join(","),
  })
  const active = timeline.segments.filter((segment) => segment.activeWalPosition !== undefined)
  if (active.length > 1) throw new Error("hour base has more than one active segment")
  const pinned = active[0]
  if (pinned?.activeWalPosition !== undefined) {
    query.set("active", `${pinned.id},${pinned.activeWalPosition}`)
  }
  const records = await request(`/api/hour?${query}`, signal)
  const contexts: LaneContext[] = []
  const points: LanePoint[] = []
  for (const record of records) {
    if (record.record === "lane_context") {
      const seconds = record.postgresql_interval_seconds
      const environment = record.environment
      contexts.push({
        segmentId: requiredText(record.segment_id, "lane context segment id"),
        postgresqlIntervalSeconds: seconds === null ? null : integer(seconds, "PostgreSQL interval"),
        environment: typeof environment === "number" && Number.isSafeInteger(environment) ? environment : null,
      })
    } else if (record.record === "lane") {
      points.push({
        segmentId: requiredText(record.segment_id, "lane segment id"),
        lane: requiredText(record["lane"], "lane name"),
        timestamp: integer(record.ts, "lane timestamp"),
        value: record.value === null ? null : finiteNumber(record.value, "lane value"),
      })
    }
  }
  return { contexts, points }
}

function timelineWindow(hour: number, visibleRange: TimelineRange | null): TimelineRange {
  const from = Math.max(hour, visibleRange?.from ?? hour)
  const toExclusive = Math.min(hour + 3_600_000_000, visibleRange?.toExclusive ?? hour + 3_600_000_000)
  if (from >= toExclusive) throw new Error("timeline hour is outside the visible report range")
  return { from, toExclusive }
}

export function withTimelineLanes(base: HourData, lanes: TimelineLanes): HourData {
  return hourData({
    sections: { ...base.sections, [HEALTH]: healthRows(base.points, lanes.contexts) },
    rateColumns: base.rateColumns,
    snapshotRows: base.snapshotRows,
    availableSections: base.availableSections,
    syntheticDemo: base.syntheticDemo === true,
    postgresqlConfigured: base.postgresqlConfigured === true,
    postgresqlPresent: base.postgresqlPresent === true,
    points: base.points,
    lanePoints: lanes.points,
    laneContexts: lanes.contexts,
    findings: base.findings,
    findingGroups: base.findingGroups,
  })
}

export async function loadSeries(
  selectedHour: number,
  section: string,
  where: Readonly<Record<string, string>>,
  fields: readonly string[],
  signal: AbortSignal,
  typeId?: string | undefined,
  group?: RelationGroup | undefined,
  scope: StatementScope = "all",
): Promise<readonly DataRow[]> {
  signal.throwIfAborted()
  const from = floorHour(selectedHour)
  const to = from + 3_600_000_000 - 1
  if (bundledFixtureRange() !== null) {
    const rows: DataRow[] = []
    const fixture = bundledFixtureHour(from)
    if (fixture !== null) rows.push(...(fixture.sections[section] ?? []))
    return [...new Map(rows.map((row) => [`${row.segmentId}:${row.typeId}:${row.ordinal}`, row])).values()]
      .filter((row) => row.timestamp >= from && row.timestamp <= to)
      .filter((row) => row.typeId === (typeId ?? row.typeId) && fixtureMatches(row, where))
      .filter((row) => group === undefined || row.relation?.group === group)
      .map((row) => projectFixtureRow(row, fields))
  }
  const query = [
    `from=${from}`,
    `to=${to}`,
    `section=${encodeURIComponent(section)}`,
    ...fields.map((name) => `field=${encodeURIComponent(name)}`),
    ...Object.entries(where).map(([column, value]) => `where.${encodeURIComponent(column)}=${encodeURIComponent(value)}`),
    ...(typeId === undefined ? [] : [`type_id=${encodeURIComponent(typeId)}`]),
    ...(group === undefined ? [] : [`group=${group}`]),
    ...(scope === "all" ? [] : [`scope=${scope}`]),
  ].join("&")
  const records = await request(`/api/hour?${query}`, signal)
  const layouts = new Map<string, RowLayout>()
  const relationLayouts = new Map<string, RelationLayout>()
  const rows: DataRow[] = []
  let segmentId = ""
  for (const record of records) {
    const layout = layoutRecord(record)
    const relationLayout = parseRelationLayout(record)
    if (record.record === "series_segment") {
      segmentId = requiredText((record.segment as { readonly id: unknown }).id, "series segment id")
    } else if (relationLayout !== null) {
      relationLayouts.set(relationLayoutKey(relationLayout), relationLayout)
    } else if (layout !== null) {
      layouts.set(layout.typeId, layout)
    } else if (record.record === "relation") {
      const row = parseRelationRow(record, relationLayouts, segmentId, from)
      if (row === null) throw new Error("relation series row is invalid")
      rows.push(row)
    } else if (record.record === "row") {
      const row = laneRow(record, segmentId, layouts)
      if (row !== null) rows.push(row)
    }
  }
  return rows
}

export async function loadEventGroups(
  from: number,
  to: number,
  sources: readonly string[],
  signal: AbortSignal,
): Promise<{ readonly rows: readonly EventEntry[]; readonly truncated: boolean }> {
  signal.throwIfAborted()
  const query = [
    `from=${from}`,
    `to=${to}`,
    "representation=groups",
    "limit=5000",
    ...sources.map((source) => `source=${encodeURIComponent(source)}`),
  ].join("&")
  const records = await request(`/api/events?${query}`, signal)
  const [header, ...items] = records
  if (header?.record !== "events" || header["representation"] !== "groups" || typeof header["truncated"] !== "boolean") {
    throw new Error("events header is invalid")
  }
  if (items.some((record) => record.record !== "event_group")) throw new Error("events item is invalid")
  return { rows: items.map(eventGroup), truncated: header["truncated"] }
}

function eventGroup(record: Readonly<Record<string, unknown>>): EventEntry {
  const tier = record["tier"]
  if (tier !== "critical" && tier !== "notable" && tier !== "routine") throw new Error("event tier is invalid")
  const minutes = numberArray(record["minutes"], "event minutes")
  if (minutes.length !== 60) throw new Error("event minutes are invalid")
  const detailRef = record["detail_ref"]
  if (typeof detailRef !== "string" || detailRef === "") throw new Error("event detail reference is invalid")
  return {
    key: requiredText(record["key"], "event key"),
    section: requiredText(record["section"], "event section"),
    tier: tier satisfies EventTier,
    label: nullableText(record["label"], "event label"),
    count: finiteNumber(record["count"], "event count"),
    firstTs: integer(record["firstTs"], "event first timestamp"),
    lastTs: integer(record["lastTs"], "event last timestamp"),
    minutes,
    stat: eventStat(record["stat"]),
    detailRef,
    representativeTs: integer(record["representativeTs"], "event representative timestamp"),
  }
}

export interface EventRowDetail {
  readonly section: string
  readonly at: number
  readonly fields: Readonly<Record<string, Cell>>
}

export async function loadEventRowDetail(detailRef: string, signal: AbortSignal): Promise<EventRowDetail> {
  signal.throwIfAborted()
  const path = `/api/row-detail?detail_ref=${encodeURIComponent(detailRef)}`
  const response = await apiFetch(path, { headers: { Accept: "application/x-ndjson" }, signal })
  if (!response.ok) throw new Error(`HTTP ${response.status} for ${path}`)
  const records = await readNdjson(response, path, signal)
  const [record] = records
  if (records.length !== 1 || record?.record !== "row_detail") throw new Error("event row detail is invalid")
  const fields = strictRecord(record["fields"], "event row detail fields")
  if (!Object.values(fields).every(validCell)) throw new Error("event row detail field is invalid")
  return {
    section: requiredText(record["section"], "event row detail section"),
    at: integer(record["at"], "event row detail timestamp"),
    fields: fields as Readonly<Record<string, Cell>>,
  }
}

function eventStat(value: unknown): EventStat {
  const stat = strictRecord(value, "event stat")
  const kind = stat["kind"]
  if (kind === "pg.errors") return {
    kind,
    severity: finiteNumber(stat["severity"], "error severity"),
    category: nullableNumber(stat["category"], "error category"),
    sqlstate: nullableText(stat["sqlstate"], "error SQLSTATE"),
    database: nullableText(stat["database"], "error database"),
    username: nullableText(stat["username"], "error username"),
  }
  if (kind === "pg.slow") return {
    kind,
    maxMs: finiteNumber(stat["maxMs"], "slow maximum"),
    totalMs: finiteNumber(stat["totalMs"], "slow total"),
    thresholdMs: nullableNumber(stat["thresholdMs"], "slow threshold"),
  }
  if (kind === "pg.autovacuum") return {
    kind,
    analyze: requiredBoolean(stat["analyze"], "autovacuum analyze"),
    runs: integer(stat["runs"], "autovacuum runs"),
    totalMs: nullableNumber(stat["totalMs"], "autovacuum total"),
    tuplesRemoved: nullableNumber(stat["tuplesRemoved"], "autovacuum removed tuples"),
    tuplesDead: nullableNumber(stat["tuplesDead"], "autovacuum dead tuples"),
  }
  if (kind === "pg.checkpoints") return {
    kind,
    completes: integer(stat["completes"], "checkpoint completes"),
    timed: integer(stat["timed"], "timed checkpoints"),
    requested: integer(stat["requested"], "requested checkpoints"),
    maxSyncMs: nullableNumber(stat["maxSyncMs"], "checkpoint maximum sync"),
    buffers: nullableNumber(stat["buffers"], "checkpoint buffers"),
  }
  if (kind === "pg.checkpoint_warning") return {
    kind,
    secondsApart: nullableNumber(stat["secondsApart"], "checkpoint warning interval"),
  }
  if (kind === "pg.locks") return {
    kind,
    holders: nullableText(stat["holders"], "lock holders"),
    acquired: requiredBoolean(stat["acquired"], "lock acquired"),
    waiters: integer(stat["waiters"], "lock waiters"),
    maxMs: nullableNumber(stat["maxMs"], "lock maximum"),
    targets: textArray(stat["targets"], "lock targets"),
  }
  if (kind === "pg.lifecycle") return {
    kind,
    lifecycle: finiteNumber(stat["lifecycle"], "lifecycle kind"),
    pid: nullableNumber(stat["pid"], "lifecycle pid"),
    signal: nullableNumber(stat["signal"], "lifecycle signal"),
    mode: nullableText(stat["mode"], "lifecycle mode"),
  }
  if (kind === "pgbouncer.events") return {
    kind,
    level: finiteNumber(stat["level"], "PgBouncer level"),
    database: optionalNullableText(stat["database"], "PgBouncer database"),
    username: optionalNullableText(stat["username"], "PgBouncer username"),
    host: optionalNullableText(stat["host"], "PgBouncer host"),
    sourceFile: optionalNullableText(stat["sourceFile"], "PgBouncer source file"),
  }
  throw new Error("event stat kind is invalid")
}

function strictRecord(value: unknown, name: string): Readonly<Record<string, unknown>> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error(`${name} is invalid`)
  return value as Readonly<Record<string, unknown>>
}

function nullableText(value: unknown, name: string): string | null {
  if (value === null) return null
  if (typeof value !== "string") throw new Error(`${name} is invalid`)
  return value
}

function optionalNullableText(value: unknown, name: string): string | null {
  return nullableText(value ?? null, name)
}

function nullableNumber(value: unknown, name: string): number | null {
  return value === null ? null : finiteNumber(value, name)
}

function requiredBoolean(value: unknown, name: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${name} is invalid`)
  return value
}

function numberArray(value: unknown, name: string): readonly number[] {
  if (!Array.isArray(value)) throw new Error(`${name} is invalid`)
  return value.map((item) => finiteNumber(item, name))
}

function textArray(value: unknown, name: string): readonly string[] {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) throw new Error(`${name} is invalid`)
  return value
}

function validCell(value: unknown): value is Cell {
  if (value === null || typeof value === "boolean" || typeof value === "string") return true
  if (typeof value === "number") return Number.isFinite(value)
  if (Array.isArray(value)) return value.every((item) => typeof item === "number" && Number.isFinite(item))
  return typeof value === "object"
}

export async function loadHeatmap(
  selectedHour: number,
  section: string,
  fields: readonly string[],
  columns: number,
  top: number,
  signal: AbortSignal,
  group?: readonly string[],
  scope: StatementScope = "all",
): Promise<HeatmapView> {
  signal.throwIfAborted()
  const from = floorHour(selectedHour)
  const to = from + 3_600_000_000 - 1
  const query = [
    `from=${from}`,
    `to=${to}`,
    `section=${encodeURIComponent(section)}`,
    ...fields.map((name) => `field=${encodeURIComponent(name)}`),
    `columns=${columns}`,
    `top=${top}`,
    ...(group ?? []).map((name) => `group=${encodeURIComponent(name)}`),
    ...(scope === "all" ? [] : [`scope=${scope}`]),
  ].join("&")
  const fixtureRange = bundledFixtureRange()
  const fixture = fixtureRange === null
    ? null
    : bundledFixtureHeatmapRecords(from, section, fields, columns, top, group, scope)
  if (fixtureRange !== null && fixture === null) throw new Error("bundled fixture has no Rust heatmap result")
  const path = `/api/heatmap?${query}`
  let records = fixture ?? await request(path, signal)
  if (fixture === null && records.some((record) => record.record === "heatmap" && record["summary"] === undefined)) {
    records = await request(path, signal, undefined, "reload")
  }
  const cells = (stored: unknown): (number | null)[] => Array.isArray(stored)
    ? stored.map((cell) => typeof cell === "number" && Number.isFinite(cell) ? cell : null)
    : []
  const total = (stored: unknown): number | null => typeof stored === "number" && Number.isFinite(stored) ? stored : null
  const texts = (stored: unknown): (string | null)[] => Array.isArray(stored)
    ? stored.map((entry) => entry === null || entry === undefined ? null : typeof entry === "object" ? null : String(entry))
    : []
  let cumulative = true
  let summary: HeatmapSummary | null = null
  let intervals: { start: number; end: number }[] = []
  let labelNames: string[] = []
  let entityCount = 0
  let othersCount = 0
  const rows: HeatmapViewRow[] = []
  let totalsBand: HeatmapBand = { total: null, cells: [] }
  let othersBand: HeatmapBand = { total: null, cells: [] }
  for (const record of records) {
    if (record.record === "heatmap") {
      cumulative = record["class"] === "cumulative"
      const mode = record["summary"]
      if (mode !== "mean" && mode !== "sum" && mode !== "max") throw new Error("heatmap summary is invalid")
      summary = mode
      entityCount = Number(record["entity_count"] ?? 0)
      othersCount = Number(record["others_count"] ?? 0)
      const names = Array.isArray(record["labels"])
        ? record["labels"].map((name) => requiredText(name, "heatmap label name"))
        : []
      if (new Set(names).size !== names.length) throw new Error("heatmap label names are not unique")
      labelNames = names
      intervals = Array.isArray(record["intervals"])
        ? record["intervals"].map((interval) => ({
          start: integer((interval as { readonly start: unknown }).start, "heatmap interval start"),
          end: integer((interval as { readonly end: unknown }).end, "heatmap interval end"),
        }))
        : []
    } else if (record.record === "heatmap_row") {
      const members = typeof record["members"] === "number" ? record["members"] : null
      rows.push({
        typeId: requiredText(record.type_id, "heatmap row type_id"),
        identity: texts(record["identity"]),
        labels: members === null ? namedCells(labelNames, record["labels"]) : {},
        members,
        total: total(record["total"]),
        cells: cells(record["cells"]),
      })
    } else if (record.record === "heatmap_band") {
      const band = { total: total(record["total"]), cells: cells(record["cells"]) }
      if (record["band"] === "totals") totalsBand = band
      else othersBand = band
    }
  }
  if (summary === null) throw new Error("heatmap header is missing")
  return { cumulative, summary, intervals, rows, totals: totalsBand, others: othersBand, othersCount, entityCount }
}

function namedCells(names: readonly string[], stored: unknown): Readonly<Record<string, Cell>> {
  if (!Array.isArray(stored) || stored.length !== names.length) throw new Error("heatmap row labels are invalid")
  const values = stored
  return Object.fromEntries(names.map((name, index) => [name, (values[index] ?? null) as Cell]))
}

export function acceptResponse<T>(promise: Promise<T>, signal: AbortSignal, apply: (value: T) => void, reject?: () => void): void {
  void promise.then((value) => { if (!signal.aborted) apply(value) }).catch(() => { if (!signal.aborted) reject?.() })
}

function laneRow(
  record: Record<string, unknown>,
  segmentId: string,
  layouts: ReadonlyMap<string, RowLayout>,
): DataRow | null {
  const typeId = requiredText(record.type_id, "row type_id")
  const layout = layouts.get(typeId)
  const logicalName = layout?.logicalName ?? logicalNameForTypeId(typeId)
  const values = record.values
  if (layout === undefined || logicalName === null || !Array.isArray(values)) return null
  return {
    segmentId,
    logicalName,
    typeId,
    ordinal: requiredText(record.ordinal, "row ordinal"),
    timestamp: integer(record.timestamp, "row timestamp"),
    values: rowValues(layout.columns, values),
  }
}

export function healthRows(points: readonly Point[], contexts: readonly LaneContext[] = []): readonly DataRow[] {
  const healthPoints = new Map<string, Point>()
  for (const point of points) {
    if (!HEALTH_SERIES.has(point.series)) continue
    const key = `${point.segmentId}:${point.timestamp}:${point.series}`
    if (!healthPoints.has(key) || point.value === null) healthPoints.set(key, point)
  }
  const stored = [...healthPoints.values()]
  const segmentOrder = new Map<string, number>()
  for (const point of stored) {
    if (!segmentOrder.has(point.segmentId)) segmentOrder.set(point.segmentId, segmentOrder.size)
  }
  const postgresIntervals = new Map(contexts.flatMap((context) => {
    const interval = context.postgresqlIntervalSeconds === null
      ? Number.NaN
      : context.postgresqlIntervalSeconds * 1_000_000
    return Number.isSafeInteger(interval) && interval >= 0 ? [[context.segmentId, interval] as const] : []
  }))
  const postgresSegments = new Set(stored.filter((point) => point.series === "postgres_health").map((point) => point.segmentId))
  const osSegments = new Set(stored.filter((point) => point.series === "os_health").map((point) => point.segmentId))
  const postgres = stored.filter((point) => point.series === "postgres_health")
    .sort((left, right) => left.timestamp - right.timestamp
      || (segmentOrder.get(left.segmentId) ?? 0) - (segmentOrder.get(right.segmentId) ?? 0))
  const evaluation = new Map<string, Point>()
  for (const point of stored) {
    if (point.series !== "overall_health") continue
    evaluation.set(`${point.segmentId}:${point.timestamp}`, point)
  }
  return [...evaluation.values()]
    .sort((left, right) => left.timestamp - right.timestamp
      || (segmentOrder.get(left.segmentId) ?? 0) - (segmentOrder.get(right.segmentId) ?? 0))
    .map((overall) => {
      const { segmentId, timestamp } = overall
      const at = (series: string) => healthPoints.get(`${segmentId}:${timestamp}:${series}`)
      const os = at("os_health")
      const values: Record<string, Cell> = {}
      values.overall_health = overall.value
      if (osSegments.has(segmentId)) values.os_health = os?.value ?? null
      if (postgresSegments.has(segmentId)) {
        const exact = at("postgres_health")
        const currentOrder = segmentOrder.get(segmentId) ?? Number.MAX_SAFE_INTEGER
        const latest = postgres.filter((point) => point.timestamp <= timestamp
          && (segmentOrder.get(point.segmentId) ?? Number.MAX_SAFE_INTEGER) <= currentOrder).at(-1)
        const interval = postgresIntervals.get(segmentId)
        const fresh = latest !== undefined && interval !== undefined && timestamp - latest.timestamp <= interval
        values.postgres_health = exact === undefined
          ? overall.value !== null || (os?.value === null && fresh) ? latest?.value ?? null : null
          : exact.value
      }
      return {
        segmentId,
        logicalName: HEALTH,
        typeId: overall.typeId,
        ordinal: `${segmentId}:${timestamp}`,
        timestamp,
        values,
      }
    })
}

const HEALTH_SERIES = new Set(["overall_health", "os_health", "postgres_health"])

export function segmentAt(segments: readonly SegmentBound[], at: number): string | null {
  return segmentBoundAt(segments, at)?.id ?? null
}

export function segmentBoundAt(segments: readonly SegmentBound[], at: number): SegmentBound | null {
  return newestSegment(segments.filter((segment) => segment.minTs <= at && segment.maxTs >= at))
    ?? newestSegment(segments.filter((segment) => segment.maxTs <= at))
}

export function snapshotRequestGroups(
  segments: readonly SegmentBound[],
  at: number,
  requests: readonly SectionRequest[],
): readonly SnapshotRequestGroup[] {
  const eligible = segments.filter((segment) => segment.minTs <= at)
  const grouped = new Map<string, { anchor: SegmentBound; requests: SectionRequest[] }>()
  const add = (anchor: SegmentBound, resolved: readonly SectionRequest[]) => {
    if (resolved.length === 0) return
    const group = grouped.get(anchor.id)
    if (group === undefined) grouped.set(anchor.id, { anchor, requests: [...resolved] })
    else group.requests.push(...resolved)
  }
  for (const request of requests) {
    const matching = eligible.flatMap((segment) => segment.sections
      .filter((section) => section.logicalName === request.section && requestAcceptsLayout(request, section.typeId))
      .map((section) => ({ segment, typeId: section.typeId })))
    if (matching.length === 0) continue
    if (request.pageSize !== undefined || request.group !== undefined) {
      const anchor = newestSegment(matching.map(({ segment }) => segment))
      if (anchor !== null) add(anchor, requestsForSegment([request], anchor))
      continue
    }
    const anchors = new Map<string, { anchor: SegmentBound; typeIds: string[] }>()
    for (const typeId of unique(matching.map((candidate) => candidate.typeId))) {
      const anchor = newestSegment(matching
        .filter((candidate) => candidate.typeId === typeId)
        .map((candidate) => candidate.segment))
      if (anchor === null) continue
      const assigned = anchors.get(anchor.id)
      if (assigned === undefined) anchors.set(anchor.id, { anchor, typeIds: [typeId] })
      else assigned.typeIds.push(typeId)
    }
    if (anchors.size === 1) {
      const assigned = anchors.values().next().value
      if (assigned !== undefined) add(assigned.anchor, requestsForSegment([request], assigned.anchor))
      continue
    }
    for (const { anchor, typeIds } of anchors.values()) {
      for (const typeId of typeIds) {
        add(anchor, requestsForSegment([{ ...request, typeId }], anchor))
      }
    }
  }
  return [...grouped.values()]
}

function requestAcceptsLayout(request: SectionRequest, typeId: string): boolean {
  if (request.typeId !== undefined && request.typeId !== typeId) return false
  if (request.typeIds !== undefined && !request.typeIds.includes(typeId)) return false
  const fields = request.fieldsByType?.[typeId] ?? request.fields
  if (fields === undefined) return request.fieldsByType === undefined
  const physical = new Set(REGISTRY_BY_TYPE_ID.get(typeId)?.columns ?? [])
  return fields.some((field) => physical.has(field))
}

function newestSegment(segments: readonly SegmentBound[]): SegmentBound | null {
  let newest: SegmentBound | null = null
  for (const segment of segments) {
    if (newest === null || compareSegmentIds(segment.id, newest.id) > 0) newest = segment
  }
  return newest
}

function compareSegmentIds(left: string, right: string): number {
  if (/^-?\d+$/.test(left) && /^-?\d+$/.test(right)) {
    const leftId = BigInt(left)
    const rightId = BigInt(right)
    return leftId < rightId ? -1 : leftId > rightId ? 1 : 0
  }
  return left.localeCompare(right, undefined, { numeric: true })
}

export function requestsForSegment(
  requests: readonly SectionRequest[],
  segment: SegmentBound,
): readonly SectionRequest[] {
  return requests.flatMap((request) => {
    const typeIds = segment.sections
      .filter((section) => section.logicalName === request.section)
      .map((section) => section.typeId)
      .filter((typeId) => request.typeId === undefined || request.typeId === typeId)
      .filter((typeId) => request.typeIds === undefined || request.typeIds.includes(typeId))
    if (typeIds.length === 0) return []
    if (request.group !== undefined) return [request]
    if (request.fields === undefined && request.fieldsByType === undefined) return [request]
    if (request.fields !== undefined
      && request.fieldsByType === undefined
      && request.typeIds === undefined
      && batchableSnapshotSection(request)) {
      const physical = new Set(typeIds.flatMap((typeId) => REGISTRY_BY_TYPE_ID.get(typeId)?.columns ?? []))
      const fields = unique(request.fields.filter((field) => physical.has(field)))
      return fields.length === 0 ? [] : [{ section: request.section, fields }]
    }
    const global = request.pageSize !== undefined && request.fieldsByType !== undefined
    const { fieldsByType: _fieldsByType, typeIds: _typeIds, ...base } = request
    const groups = global ? [unique(typeIds)] : unique(typeIds).map((typeId) => [typeId])
    return groups.flatMap((types) => {
      const physical = new Set(types.flatMap((typeId) => REGISTRY_BY_TYPE_ID.get(typeId)?.columns ?? []))
      const projection = types.flatMap((typeId) => request.fieldsByType?.[typeId] ?? request.fields ?? [])
      const fields = unique(projection.filter((field) => physical.has(field)))
      // An empty projection would request every column.
      if (fields.length === 0) return []
      const keep = (candidates: readonly string[]) => candidates.filter((field) => field.startsWith("derived.")
        ? types.some((typeId) => supportsPostgresDerivedOrder(typeId, field))
        : physical.has(field))
      const order = request.order === undefined
        ? undefined
        : Object.fromEntries(Object.entries(request.order).map(([name, candidates]) => [name, keep(candidates)]))
      return [{
        ...base,
        ...(global ? {} : { typeId: types[0] }),
        fields,
        ...(request.defaultOrder === undefined ? {} : { defaultOrder: keep(request.defaultOrder) }),
        ...(order === undefined ? {} : { order }),
        ...(request.fallbackOrder === undefined ? {} : { fallbackOrder: keep(request.fallbackOrder) }),
      }]
    })
  })
}

export function sectionRows(data: HourData, logicalName: string): readonly DataRow[] {
  return data.sections[logicalName] ?? []
}

export function logicalNameForTypeId(typeId: string): string | null {
  if (typeId === "0") return "health"
  return REGISTRY_BY_TYPE_ID.get(typeId)?.logicalName ?? null
}

export function fieldNameForLocator(locator: Pick<Finding, "typeId" | "fieldOrdinal">): string | null {
  if (locator.typeId === "0") {
    if (locator.fieldOrdinal === 0) return "os_health"
    if (locator.fieldOrdinal === 1) return "overall_health"
    return null
  }
  return REGISTRY_BY_TYPE_ID.get(locator.typeId)?.columns[locator.fieldOrdinal] ?? null
}

export function resolveLoadedRow(
  data: Pick<HourData, "sections">,
  locator: Pick<Finding, "segmentId" | "typeId" | "rowOrdinal" | "timestamp">,
): DataRow | null {
  const logicalName = logicalNameForTypeId(locator.typeId)
  if (logicalName === null) return null
  return (data.sections[logicalName] ?? []).find((row) => rowMatchesLocator(row, locator)) ?? null
}

export function resolveLocator(data: Pick<HourData, "sections">, finding: Finding): ResolvedLocator | null {
  const fieldName = fieldNameForLocator(finding)
  const row = finding.typeId === "0"
    ? (data.sections.health ?? []).find((candidate) => candidate.segmentId === finding.segmentId
      && candidate.timestamp === finding.timestamp
      && fieldName !== null
      && Object.hasOwn(candidate.values, fieldName)) ?? null
    : resolveLoadedRow(data, finding)
  if (row === null) return null
  return {
    logicalName: row.logicalName,
    row,
    fieldName,
  }
}

function hourData(input: {
  readonly sections: Readonly<Record<string, readonly DataRow[]>>
  readonly rateColumns?: Readonly<Record<string, readonly string[]>>
  readonly snapshotRows?: readonly SnapshotRows[]
  readonly availableSections: readonly string[]
  readonly syntheticDemo?: boolean
  readonly postgresqlConfigured?: boolean
  readonly postgresqlPresent?: boolean
  readonly points: readonly Point[]
  readonly lanePoints: readonly LanePoint[]
  readonly laneContexts?: readonly LaneContext[]
  readonly findings: readonly Finding[]
  readonly findingGroups?: readonly FindingGroup[]
}): HourData {
  const rows = (name: string) => input.sections[name] ?? []
  return {
    ...input,
    syntheticDemo: input.syntheticDemo ?? false,
    postgresqlConfigured: input.postgresqlConfigured ?? false,
    postgresqlPresent: input.postgresqlPresent ?? false,
    rateColumns: input.rateColumns ?? {},
    snapshotRows: input.snapshotRows ?? [],
    laneContexts: input.laneContexts ?? [],
    findingGroups: input.findingGroups ?? [],
    processes: rows("os_process"),
    activities: rows("pg_stat_activity"),
    load: rows("os_loadavg"),
    memory: rows("os_meminfo"),
    pressure: rows("os_psi"),
    health: rows("health"),
  }
}

const CELL_TEXT = 160
const RELATED_STATEMENT_TEXT_PAGE_SIZE = 1
const RELATED_STATEMENT_TEXT_FIELDS = ["query"] as const
const RELATED_STATEMENT_TEXT_FIELDS_BY_TYPE = Object.fromEntries(
  PG_STAT_STATEMENTS_TYPE_IDS.map((typeId) => [typeId, RELATED_STATEMENT_TEXT_FIELDS]),
)

// The Plans panel on a selected statement: identity plus the whole-snapshot
// counters the Plans table's totals lens shows for the same rows.
const RELATED_PLAN_FIELDS = ["planid", "calls", "total_time"] as const
const RELATED_PLAN_FIELDS_BY_TYPE = Object.fromEntries(
  PG_STORE_PLANS_TYPE_IDS.map((typeId) => [typeId, RELATED_PLAN_FIELDS]),
)
const RELATED_PLAN_PAGE_SIZE = 20

// The Statement panel on a selected plan: the text plus the counters every
// pg_stat_statements layout carries under one name or the other.
const RELATED_STATEMENT_FIELDS_BY_TYPE = Object.fromEntries(PG_STAT_STATEMENTS_TYPE_IDS.map((typeId) => [
  typeId,
  typeId === "1002001"
    ? ["query", "calls", "rows", "total_time", "mean_time"]
    : ["query", "calls", "rows", "total_exec_time", "mean_exec_time"],
]))

/** Which statements a `pg_stat_statements` result includes; `workload` omits the collector's own. */
export type StatementScope = "all" | "workload"

export interface SnapshotOptions {
  readonly filters?: Readonly<Record<string, string>>
  readonly typeId?: string
  readonly rowOrdinal?: string
  readonly fullText?: boolean
  readonly cursor?: string
  readonly search?: string
  readonly firstMatch?: boolean
  readonly scope?: StatementScope
}

export interface SnapshotOrder {
  readonly column: string
  readonly descending: boolean
}

export async function loadSnapshot(
  segmentId: string,
  at: number,
  sections: readonly (SectionRequest | string)[],
  signal: AbortSignal,
  order?: SnapshotOrder | undefined,
  options?: SnapshotOptions | Readonly<Record<string, string>> | undefined,
): Promise<HourData> {
  const requests = sections.map((section) => typeof section === "string" ? { section } : section)
  const chosen = snapshotOptions(options)
  if (requests.length === 0) return emptyHour()
  if ((chosen.filters !== undefined || chosen.typeId !== undefined || chosen.rowOrdinal !== undefined
      || chosen.cursor !== undefined || chosen.search !== undefined || chosen.firstMatch === true)
    && requests.length !== 1) {
    throw new Error("a filtered, searched, paged, or exact snapshot needs one section")
  }
  if (requests.some((request) => request.pageSize !== undefined) && requests.length !== 1) {
    throw new Error("a paged snapshot needs one section")
  }
  if (chosen.rowOrdinal !== undefined && chosen.typeId === undefined && requests[0]?.typeId === undefined) {
    throw new Error("an exact snapshot row needs typeId")
  }
  if (chosen.rowOrdinal !== undefined && chosen.filters !== undefined) {
    throw new Error("an exact snapshot row cannot carry filters")
  }
  if (chosen.rowOrdinal !== undefined && (chosen.cursor !== undefined || chosen.search !== undefined)) {
    throw new Error("an exact snapshot row cannot carry paging or search")
  }
  if (chosen.cursor !== undefined && requests[0]?.pageSize === undefined) {
    throw new Error("a snapshot cursor needs a paged section")
  }
  for (const section of requests) {
    if (section.fieldsByType !== undefined) {
      throw new Error(`the ${section.section} projection needs the cursor segment`)
    }
    if (section.typeId !== undefined && chosen.typeId !== undefined && section.typeId !== chosen.typeId) {
      throw new Error(`the ${section.section} snapshot has conflicting type IDs`)
    }
    if (section.fields !== undefined && section.fields.length === 0) {
      throw new Error(`the ${section.section} projection has no physical fields`)
    }
  }
  signal.throwIfAborted()
  const fixture = fixtureSnapshot(segmentId, at, requests, chosen, order)
  if (fixture !== null) return fixture
  const whole = requests.filter((section) => batchableSnapshotSection(section) && section.fields === undefined)
  const projected = requests.filter((section) => batchableSnapshotSection(section) && section.fields !== undefined)
  const individual = requests.filter((section) => !batchableSnapshotSection(section))
  const batches = chosen.filters !== undefined || chosen.typeId !== undefined || chosen.rowOrdinal !== undefined
    ? requests.map((section) => [section] as const)
    : [
        ...(whole.length === 0 ? [] : [whole]),
        ...(projected.length === 0 ? [] : [projected]),
        ...individual.map((section) => [section] as const),
      ]
  const responses = await Promise.all(batches.map(async (batch) => {
    const query = snapshotQuery(at, batch, order, chosen)
    return request(`/api/segments/${encodeURIComponent(segmentId)}/snapshot?${query}`, signal)
  }))
  const records = responses.flat()
  const layouts = new Map<string, { readonly logicalName: string; readonly columns: readonly string[] }>()
  const relationLayouts = new Map<string, RelationLayout>()
  const grouped: Record<string, DataRow[]> = {}
  const rateColumns: Record<string, readonly string[]> = {}
  const snapshotRows: SnapshotRows[] = []
  for (const section of requests) grouped[section.section] = []
  for (const record of records) {
    const layout = layoutRecord(record)
    const relationLayout = parseRelationLayout(record)
    if (relationLayout !== null) {
      relationLayouts.set(relationLayoutKey(relationLayout), relationLayout)
      rateColumns[relationLayout.logicalName] = unique([
        ...(rateColumns[relationLayout.logicalName] ?? []),
        ...relationRateFields(relationLayout),
      ])
    } else if (layout !== null) {
      const logicalName = requiredText(layout.logicalName, "logical name")
      layouts.set(layout.typeId, {
        logicalName,
        columns: layout.columns,
      })
      if (Array.isArray(record.rates)) {
        rateColumns[logicalName] = unique([
          ...(rateColumns[logicalName] ?? []),
          ...record.rates.map((name) => requiredText(name, "rate column")),
        ])
      }
    } else if (record.record === "relation") {
      const row = parseRelationRow(record, relationLayouts, segmentId, at)
      if (row === null) throw new Error("relation record is invalid")
      const rows = grouped[row.logicalName] ?? []
      rows.push(row)
      grouped[row.logicalName] = rows
    } else if (record.record === "row") {
      const typeId = requiredText(record.type_id, "row type_id")
      const layout = layouts.get(typeId)
      const values = record.values
      if (layout === undefined || !Array.isArray(values)) {
        throw new Error(`row for layout ${typeId} arrived before its layout`)
      }
      const { columns, logicalName } = layout
      const rows = grouped[logicalName] ?? []
      rows.push({
        segmentId: requiredText(record.segment_id, "row segment id"),
        logicalName,
        typeId,
        ordinal: requiredText(record.ordinal, "row ordinal"),
        timestamp: record.timestamp === null ? at : integer(record.timestamp, "row timestamp"),
        values: rowValues(columns, values),
      })
      grouped[logicalName] = rows
    } else if (record.record === "snapshot_page") {
      const logicalName = requiredText(record.logical_name, "snapshot page logical name")
      if ((record.order_direction !== "asc" && record.order_direction !== "desc")
        || typeof record.has_more !== "boolean"
        || typeof record["truncated"] !== "boolean"
        || !Array.isArray(record.order_by)
        || (record.next_cursor !== null && typeof record.next_cursor !== "string")) {
        throw new Error(`snapshot page for ${logicalName} is invalid`)
      }
      snapshotRows.push({
        logicalName,
        eligible: integer(record["eligible"], "eligible row count"),
        excluded: record["excluded"] === undefined ? 0 : integer(record["excluded"], "excluded row count"),
        returned: integer(record["returned"], "returned row count"),
        hasMore: record.has_more,
        truncated: record["truncated"],
        nextCursor: record.next_cursor,
        pageSize: integer(record.page_size, "snapshot page size"),
        orderBy: record.order_by.map((field) => requiredText(field, "snapshot order field")),
        orderDirection: record.order_direction,
        from: record.from === null ? null : integer(record.from, "snapshot interval start"),
        to: record.to === null ? null : integer(record.to, "snapshot interval end"),
        ...(record.group === undefined ? {} : { group: relationGroup(record.group) }),
      })
    }
  }
  return hourData({
    sections: grouped,
    rateColumns,
    snapshotRows,
    availableSections: unique(requests.map((section) => section.section)),
    points: [],
    lanePoints: [],
    findings: [],
    findingGroups: [],
  })
}

export async function loadSnapshotGroups(
  groups: readonly SnapshotRequestGroup[],
  at: number,
  signal: AbortSignal,
  order?: SnapshotOrder | undefined,
): Promise<HourData> {
  const snapshots = await Promise.all(groups.map((group) => loadSnapshot(
    group.anchor.id,
    at,
    group.requests,
    signal,
    order,
  )))
  return snapshots.reduce((current, incoming) => mergeSnapshotData(current, incoming), emptyHour())
}

// The recorded plans matching a statement's identity expression, from the
// newest snapshot at or before the moment. One page is the whole answer: a
// statement with more distinct plans than the page holds is itself the story,
// and the Plans view shows the rest.
export async function loadRelatedPlanRows(
  segments: readonly SegmentBound[],
  at: number,
  search: string,
  signal: AbortSignal,
): Promise<readonly DataRow[]> {
  const [group] = snapshotRequestGroups(segments, at, [{
    section: "pg_store_plans",
    typeIds: PG_STORE_PLANS_TYPE_IDS,
    fieldsByType: RELATED_PLAN_FIELDS_BY_TYPE,
    pageSize: RELATED_PLAN_PAGE_SIZE,
  }])
  const [request] = group?.requests ?? []
  if (group === undefined || request === undefined) return []
  signal.throwIfAborted()
  const page = await loadSnapshot(group.anchor.id, at, [request], signal, undefined, { search })
  return page.sections.pg_store_plans ?? []
}

// The one statement matching a plan's identity expression. An ordinary paged
// search, not `first_match`: the server pins that shortcut to a text-only
// projection, and this panel wants the counters too.
export async function loadRelatedStatementRow(
  segments: readonly SegmentBound[],
  at: number,
  search: string,
  signal: AbortSignal,
): Promise<DataRow | null> {
  const [group] = snapshotRequestGroups(segments, at, [{
    section: "pg_stat_statements",
    typeIds: PG_STAT_STATEMENTS_TYPE_IDS,
    fieldsByType: RELATED_STATEMENT_FIELDS_BY_TYPE,
    pageSize: RELATED_STATEMENT_TEXT_PAGE_SIZE,
  }])
  const [request] = group?.requests ?? []
  if (group === undefined || request === undefined) return null
  signal.throwIfAborted()
  const page = await loadSnapshot(group.anchor.id, at, [request], signal, undefined, { fullText: true, search })
  return page.sections.pg_stat_statements?.[0] ?? null
}

export async function loadRelatedStatementTextRow(
  segments: readonly SegmentBound[],
  at: number,
  queryId: string,
  signal: AbortSignal,
): Promise<DataRow | null> {
  const search = canonicalSearch([{ key: "query_id", value: queryId }], "pg_stat_statements")
  if (search === null) return null
  const [group] = snapshotRequestGroups(segments, at, [{
    section: "pg_stat_statements",
    typeIds: PG_STAT_STATEMENTS_TYPE_IDS,
    fieldsByType: RELATED_STATEMENT_TEXT_FIELDS_BY_TYPE,
    pageSize: RELATED_STATEMENT_TEXT_PAGE_SIZE,
  }])
  const [request] = group?.requests ?? []
  if (group === undefined || request === undefined) return null

  signal.throwIfAborted()
  const page = await loadSnapshot(
    group.anchor.id,
    at,
    [request],
    signal,
    undefined,
    { firstMatch: true, fullText: true, search },
  )
  return page.sections.pg_stat_statements?.[0] ?? null
}

function rowValues(columns: readonly string[], cells: readonly unknown[]): Readonly<Record<string, Cell>> {
  return Object.fromEntries(columns.flatMap((name, index) => name === "ts"
    ? []
    : [[name, index < cells.length ? cells[index] as Cell : null]]))
}

function fixtureSnapshot(
  segmentId: string,
  at: number,
  requests: readonly SectionRequest[],
  options: SnapshotOptions,
  order: SnapshotOrder | undefined,
): HourData | null {
  const fixture = bundledFixtureHour(floorHour(at))
  if (fixture === null) return null
  const grouped: Record<string, readonly DataRow[]> = {}
  const snapshotRows: SnapshotRows[] = []
  for (const request of requests) {
    const typeId = request.typeId ?? options.typeId
    const sourceRows = fixture.sections[request.section] ?? []
    let rows = sourceRows
      .filter((row) => row.segmentId === segmentId)
      .filter((row) => typeId === undefined || row.typeId === typeId)
      .filter((row) => fixtureMatches(row, options.filters ?? {}))
      .filter((row) => options.rowOrdinal === undefined
        || (row.ordinal === options.rowOrdinal && row.timestamp === at))
    if (options.rowOrdinal === undefined && rows.length !== 0) {
      const before = rows.filter((row) => row.timestamp <= at)
      rows = before.length === 0
        ? []
        : rows.filter((row) => row.timestamp === Math.max(...before.map((row) => row.timestamp)))
    }
    const eligible = rows.length
    const orderFields = snapshotOrder(request, order)
    const orderField = orderFields.find((field) => field.startsWith("derived.") || rows.some((row) => row.values[field] !== undefined))
    if (orderField !== undefined) {
      const value = (row: DataRow) => orderField.startsWith("derived.")
        ? fixtureDerivedOrderValue(row, sourceRows, orderField.slice("derived.".length))
        : row.values[orderField]
      rows = rows.slice().sort((left, right) => fixtureOrder(value(right), value(left)))
    }
    if (request.pageSize !== undefined) rows = rows.slice(0, request.pageSize)
    if (request.pageSize !== undefined && orderField !== undefined) {
      if ((request.typeId ?? options.typeId ?? rows[0]?.typeId) !== undefined) snapshotRows.push({
        logicalName: request.section,
        eligible,
        excluded: 0,
        returned: rows.length,
        hasMore: eligible > rows.length,
        truncated: eligible > rows.length,
        nextCursor: null,
        pageSize: request.pageSize,
        orderBy: [orderField.replace(/^derived\./, "")],
        orderDirection: "desc",
        from: null,
        to: rows[0]?.timestamp ?? null,
      })
    }
    const fields = request.fields
    const selected = fields === undefined
      ? rows
      : rows.map((row) => projectFixtureRow(row, fields))
    grouped[request.section] = [...(grouped[request.section] ?? []), ...selected]
  }
  return hourData({
    sections: grouped,
    rateColumns: {},
    snapshotRows,
    availableSections: unique(requests.map((request) => request.section)),
    points: [],
    lanePoints: [],
    findings: [],
    findingGroups: [],
  })
}

export function fixtureDerivedOrderValue(row: DataRow, rows: readonly DataRow[], field: string): Cell {
  if (field === "cv") return decoratePostgresIntervalRow(row).values.cv ?? null
  const identity = postgresIdentity(row.typeId)
  const before = rows
    .filter((candidate) => candidate.typeId === row.typeId && candidate.timestamp < row.timestamp
      && identity.every((name) => JSON.stringify(candidate.values[name]) === JSON.stringify(row.values[name])))
    .sort((left, right) => left.timestamp - right.timestamp)
    .at(-1)
  if (before === undefined) return null
  const values = Object.fromEntries(Object.keys(row.values).map((name) => [name, intervalMetric(before, row, name)]))
  return decoratePostgresIntervalRow({ ...row, values }).values[field] ?? null
}

function fixtureMatches(row: DataRow, filters: Readonly<Record<string, string>>): boolean {
  return Object.entries(filters).every(([field, expected]) => fixtureText(row.values[field]) === expected)
}

function fixtureText(cell: Cell | undefined): string | null {
  return typeof cell === "string" || typeof cell === "number" || typeof cell === "boolean"
    ? String(cell)
    : null
}

function fixtureOrder(left: Cell | undefined, right: Cell | undefined): number {
  const leftNumber = typeof left === "number" ? left : typeof left === "string" ? Number(left) : Number.NaN
  const rightNumber = typeof right === "number" ? right : typeof right === "string" ? Number(right) : Number.NaN
  if (Number.isFinite(leftNumber) && Number.isFinite(rightNumber)) return leftNumber - rightNumber
  return (fixtureText(left) ?? "").localeCompare(fixtureText(right) ?? "")
}

function projectFixtureRow(row: DataRow, fields: readonly string[]): DataRow {
  return {
    ...row,
    values: Object.fromEntries(fields.flatMap((field) => Object.hasOwn(row.values, field)
      ? [[field, row.values[field] ?? null]]
      : [])),
  }
}

function batchableSnapshotSection(section: SectionRequest): boolean {
  return section.typeId === undefined
    && section.pageSize === undefined
    && section.defaultOrder === undefined
    && section.order === undefined
    && section.fallbackOrder === undefined
    && section.group === undefined
}

function snapshotQuery(
  at: number,
  sections: readonly SectionRequest[],
  order: SnapshotOrder | undefined,
  options: SnapshotOptions,
): string {
  const section = sections.length === 1 ? sections[0] : undefined
  const typeId = section?.typeId ?? options.typeId
  const fields = section?.fields ?? unique(sections.flatMap((request) => request.fields ?? []))
  const ordered = section === undefined || options.rowOrdinal !== undefined
    ? []
    : snapshotOrder(section, order)
  const requestedOrder = section === undefined || order === undefined ? undefined : requestedSnapshotOrder(section, order)
  return [
    `at=${at}`,
    ...sections.map((request) => `section=${encodeURIComponent(request.section)}`),
    ...fields.map((field) => `field=${encodeURIComponent(field)}`),
    ...ordered.map((field) => `by=${encodeURIComponent(field)}`),
    ...(section?.group === undefined ? [] : [`group=${section.group}`]),
    ...(requestedOrder === undefined || order?.descending !== false ? [] : ["direction=asc"]),
    ...(section?.pageSize === undefined || options.rowOrdinal !== undefined ? [] : [`page_size=${section.pageSize}`]),
    ...(options.fullText === true ? [] : [`text=${CELL_TEXT}`]),
    ...(options.cursor === undefined ? [] : [`cursor=${encodeURIComponent(options.cursor)}`]),
    ...(options.search === undefined ? [] : [`search=${encodeURIComponent(options.search)}`]),
    ...(options.firstMatch === true ? ["first_match=1"] : []),
    ...(options.scope === undefined || options.scope === "all" ? [] : [`scope=${options.scope}`]),
    ...Object.entries(options.filters ?? {}).map(([column, value]) =>
      `where.${encodeURIComponent(column)}=${encodeURIComponent(value)}`),
    ...(typeId === undefined ? [] : [`type_id=${encodeURIComponent(typeId)}`]),
    ...(options.rowOrdinal === undefined ? [] : [`row_ordinal=${encodeURIComponent(options.rowOrdinal)}`]),
  ].join("&")
}

function snapshotOrder(section: SectionRequest, chosen: SnapshotOrder | undefined): readonly string[] {
  const requested = chosen === undefined ? section.defaultOrder : requestedSnapshotOrder(section, chosen)
  if (requested !== undefined && requested.length > 0) return unique(requested)
  if (chosen !== undefined && section.defaultOrder !== undefined && section.defaultOrder.length > 0) {
    return unique(section.defaultOrder)
  }
  return unique(section.fallbackOrder ?? [])
}

function requestedSnapshotOrder(section: SectionRequest, chosen: SnapshotOrder): readonly string[] | undefined {
  return section.order === undefined
    ? section.fields?.includes(chosen.column) === true ? [chosen.column] : undefined
    : section.order[chosen.column]
}

function snapshotOptions(
  value: SnapshotOptions | Readonly<Record<string, string>> | undefined,
): SnapshotOptions {
  if (value === undefined) return {}
  if ("filters" in value || "typeId" in value || "rowOrdinal" in value || "fullText" in value
    || "cursor" in value || "search" in value || "firstMatch" in value || "scope" in value) {
    return value as SnapshotOptions
  }
  return { filters: value as Readonly<Record<string, string>> }
}

function emptyHour(): HourData {
  return hourData({
    sections: {},
    rateColumns: {},
    availableSections: [],
    points: [],
    lanePoints: [],
    findings: [],
  })
}

const HEALTH = "health"

function isFindingRecord(record: Record<string, unknown>): boolean {
  return record.record === "finding"
    && (record["kind"] === "known_bad" || record["kind"] === "spike" || record["kind"] === "event")
}

function indexPoint(record: Record<string, unknown>, segmentId: string, logicalName: string): Point {
  const typeId = requiredText(record.type_id, "point type_id")
  return {
    segmentId,
    logicalName: logicalNameForTypeId(typeId) ?? logicalName,
    typeId,
    series: requiredText(record["series"], "point series"),
    timestamp: integer(record.ts, "point timestamp"),
    identity: cellRecord(record.identity),
    value: record.value === null ? null : finiteNumber(record.value, "point value"),
  }
}

function indexFinding(record: Record<string, unknown>, segmentId: string, logicalName: string): Finding {
  const typeId = requiredText(record.type_id, "finding type_id")
  return {
    segmentId,
    logicalName: typeof record.logical_name === "string"
      ? record.logical_name
      : logicalNameForTypeId(typeId) ?? logicalName,
    kind: record["kind"] as Finding["kind"],
    typeId,
    timestamp: integer(record.ts, "finding timestamp"),
    category: typeof record["category"] === "number" ? record["category"] : null,
    rowOrdinal: requiredText(record.row_ordinal, "finding row ordinal"),
    fieldOrdinal: integer(record.field_ordinal, "finding field ordinal"),
  }
}

interface RowLayout {
  readonly typeId: string
  readonly logicalName: string | null
  readonly columns: readonly string[]
}

function layoutRecord(record: Record<string, unknown>): RowLayout | null {
  if (record.record !== "layout") return null
  const layout = record.layout as {
    readonly type_id: unknown
    readonly logical_name?: unknown
    readonly columns?: readonly { readonly name: unknown }[]
  }
  const typeId = requiredText(layout.type_id, "layout type_id")
  if (!Array.isArray(layout["columns"])) throw new Error(`layout ${typeId} has no columns`)
  return {
    typeId,
    logicalName: typeof layout.logical_name === "string" ? layout.logical_name : null,
    columns: layout["columns"].map((column) => requiredText(column.name, "column name")),
  }
}

async function request(path: string, signal: AbortSignal, onBytes?: (received: number) => void, cache: RequestCache = "default"): Promise<readonly Record<string, unknown>[]> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    let response: Response
    try {
      response = await apiFetch(path, { headers: { Accept: "application/x-ndjson" }, signal, cache })
    } catch (error) {
      signal.throwIfAborted()
      if (attempt === 0 && error instanceof TypeError) continue
      throw error
    }
    if (!response.ok) {
      throw new Error(`HTTP ${response.status} for ${path}`)
    }
    try {
      return await readNdjson(response, path, signal, onBytes)
    } catch (error) {
      signal.throwIfAborted()
      if (attempt === 0 && error instanceof TypeError) continue
      throw error
    }
  }
  throw new Error(`HTTP read failed for ${path}`)
}

function catalogSegments(records: readonly Record<string, unknown>[]): readonly Segment[] {
  return records.filter(
    (record) => record.record === "finished_segment" || record.record === "active_segment",
  ) as unknown as readonly Segment[]
}

function activePosition(segment: Segment): string {
  const cursor = segment.cursor
  if (cursor === undefined || cursor.segment_id !== segment.id
    || typeof cursor.wal_position !== "string" || !/^\d+$/.test(cursor.wal_position)) {
    throw new Error(`active segment ${segment.id} cursor is invalid`)
  }
  return cursor.wal_position
}

function sourceConfigured(header: Record<string, unknown> | undefined, name: string): boolean {
  const families = header?.source_families
  return Array.isArray(families) && families.some((family) => family !== null
    && typeof family === "object"
    && (family as { readonly name?: unknown }).name === name
    && (family as { readonly configured?: unknown }).configured === true)
}

function sourceMetricsPresent(header: Record<string, unknown> | undefined, name: string): boolean {
  const families = header?.source_families
  return Array.isArray(families) && families.some((family) => family !== null
    && typeof family === "object"
    && (family as { readonly name?: unknown }).name === name
    && (family as { readonly metrics_present?: unknown }).metrics_present === true)
}

function segmentSectionNames(segment: Segment): string[] {
  const present = new Set(segment["sections"].flatMap((section) =>
    section.logical_name !== null && UI_SECTION_NAME_SET.has(section.logical_name) ? [section.logical_name] : [],
  ))
  return UI_SECTION_NAMES.filter((name) => present.has(name))
}

function availableSectionNames(segments: readonly Segment[]): string[] {
  const present = new Set(segments.flatMap(segmentSectionNames))
  if ([...present].some((name) => name.startsWith("os_"))) present.add("health")
  return [...UI_SECTION_NAMES, "health"].filter((name) => present.has(name))
}

function requiredText(value: unknown, name: string): string {
  if ((typeof value === "string" && value !== "") || typeof value === "number") return String(value)
  throw new Error(`${name} is invalid`)
}

function integer(value: unknown, name: string): number {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN
  if (!Number.isSafeInteger(parsed)) throw new Error(`${name} is invalid`)
  return parsed
}

function finiteNumber(value: unknown, name: string): number {
  const parsed = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN
  if (!Number.isFinite(parsed)) throw new Error(`${name} is invalid`)
  return parsed
}

function cellRecord(value: unknown): Readonly<Record<string, Cell>> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return {}
  return value as Readonly<Record<string, Cell>>
}

function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}
