import type { Cell, DataRow, Finding, HourData, LanePoint, Point } from "./api"

interface FixtureTable {
  readonly columns: readonly string[]
  readonly snapshots: readonly {
    readonly rows: readonly (readonly Cell[])[]
    readonly segment_id: string
    readonly ts: string
    readonly type_id: string
  }[]
}

interface FixtureHeatmap {
  readonly columns: number
  readonly fields: readonly string[]
  readonly from: number
  readonly group: readonly string[]
  readonly records: readonly Readonly<Record<string, unknown>>[]
  readonly scope?: string
  readonly section: string
  readonly top: number
}

type FixturePoint = readonly [string, number | null, string?]

const FIXTURE_NON_TABLE_TYPE_NAMES: Readonly<Record<string, string>> = {
  "1002003": "pg_stat_statements",
  "1102001": "os_cpu",
  "2001001": "pg_log_errors",
  "2002001": "pg_log_checkpoints",
  "2003001": "pg_log_autovacuum",
  "2004001": "pg_log_slow_queries",
  "2007001": "pg_log_temp_files",
}

interface RealHourFixture {
  readonly findings: readonly {
    readonly category?: number
    readonly field_ordinal: number
    readonly kind: Finding["kind"]
    readonly row_ordinal: number
    readonly segment_id: string
    readonly t: string
    readonly type_id: string
  }[]
  readonly heatmaps?: readonly FixtureHeatmap[]
  readonly meta: {
    readonly captureFromUs: string
    readonly captureToUs: string
    readonly segments: number
  }
  readonly os: FixtureTable
  readonly pg: FixtureTable
  readonly system: {
    readonly cpuBusy: readonly FixturePoint[]
    readonly health: readonly FixturePoint[]
    readonly load1: readonly FixturePoint[]
    readonly memAvailable: readonly FixturePoint[]
    readonly minFsFree: readonly FixturePoint[]
    readonly oom: readonly FixturePoint[]
    readonly psi: Readonly<Record<string, readonly FixturePoint[]>>
  }
}

export function bundledFixtureRange(): { readonly from: number; readonly to: number } | null {
  const fixture = rawFixture()
  return fixture === null ? null : {
    from: Number(fixture.meta.captureFromUs),
    to: Number(fixture.meta.captureToUs),
  }
}

export function bundledFixtureHeatmapRecords(
  from: number,
  section: string,
  fields: readonly string[],
  columns: number,
  top: number,
  group: readonly string[] = [],
  scope = "all",
): readonly Readonly<Record<string, unknown>>[] | null {
  const heatmaps = rawFixture()?.heatmaps
  if (!Array.isArray(heatmaps)) return null
  for (const candidate of heatmaps) {
    if (candidate === null || typeof candidate !== "object"
        || !Array.isArray(candidate.fields) || !Array.isArray(candidate.group)
        || !candidate.fields.every((name: unknown) => typeof name === "string")
        || !candidate.group.every((name: unknown) => typeof name === "string")
        || !Array.isArray(candidate.records)
        || !candidate.records.every((record: unknown) => record !== null && typeof record === "object" && !Array.isArray(record))) continue
    if (candidate.from === from && candidate.section === section
        && candidate.columns === columns && candidate.top === top
        && (candidate.scope ?? "all") === scope
        && sameNames(candidate.fields, fields) && sameNames(candidate.group, group)) return candidate.records
  }
  return null
}

function sameNames(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((name, index) => name === right[index])
}

export function bundledFixtureHour(start: number): HourData | null {
  const fixture = rawFixture()
  if (fixture === null) return null
  const end = start + 3_600_000_000
  const within = (timestamp: number) => timestamp >= start && timestamp < end
  const segmentAt = segmentForTimestamp(fixture.os)
  const processes = tableRows(fixture.os, "os_process").filter((row) => within(row.timestamp))
  const activities = tableRows(fixture.pg, "pg_stat_activity").filter((row) => within(row.timestamp))
  const health = seriesRows(fixture.system["health"], "health", "0", { field: "os_health", segmentAt })
    .filter((row) => within(row.timestamp))
  const load = seriesRows(fixture.system.load1, "os_loadavg", "1105001", { field: "load1", segmentAt })
    .filter((row) => within(row.timestamp))
  const memory = seriesRows(fixture.system.memAvailable, "os_meminfo", "1104001", {
    field: "mem_available_percent",
    segmentAt,
  }).filter((row) => within(row.timestamp))
  const pressure = Object.entries(fixture.system.psi).flatMap(([resource, series]) =>
    seriesRows(series, "os_psi", "1107001", {
      field: "some_avg10",
      extra: { resource: Number(resource) },
      segmentAt,
    }),
  ).filter((row) => within(row.timestamp))
  const points = fixturePoints(fixture, segmentAt, activities).filter((point) => within(point.timestamp))
  const lanePoints = fixtureLanePoints(points, activities)
  const tableTypeNames = fixtureTableTypeNames(fixture)
  const findings = fixture["findings"].map((finding) => ({
    segmentId: finding.segment_id,
    logicalName: fixtureLogicalName(tableTypeNames, finding.type_id),
    kind: finding["kind"],
    typeId: finding.type_id,
    timestamp: Number(finding.t),
    category: finding["category"] ?? null,
    rowOrdinal: String(finding.row_ordinal),
    fieldOrdinal: finding.field_ordinal,
  })).filter((finding) => within(finding.timestamp))
  const groupedFindings = new Map<string, { readonly first: typeof findings[number]; count: number }>()
  for (const finding of findings) {
    const key = `${finding.segmentId}:${finding.typeId}`
    const group = groupedFindings.get(key)
    if (group === undefined) groupedFindings.set(key, { first: finding, count: 1 })
    else group.count += 1
  }
  const findingGroups = [...groupedFindings.values()].map(({ first, count }) => ({
    segmentId: first.segmentId,
    logicalName: first.logicalName,
    typeId: first.typeId,
    totalHits: count,
    shown: count,
    truncated: false,
  }))
  const sections = {
    os_process: processes,
    pg_stat_activity: activities,
    health,
  }
  return {
    sections,
    rateColumns: {},
    snapshotRows: [],
    availableSections: ["os_process", "pg_stat_activity", "health"],
    postgresqlConfigured: true,
    postgresqlPresent: true,
    processes,
    activities,
    load,
    memory,
    pressure,
    health,
    points,
    lanePoints,
    laneContexts: [],
    findings,
    findingGroups,
  }
}

function fixtureLanePoints(points: readonly Point[], activities: readonly DataRow[]): readonly LanePoint[] {
  const lanes: LanePoint[] = points.flatMap((point) => {
    const lane = point.series === "os_cpu_busy_percent"
      ? "cpu_busy"
      : point.series === "os_mem_available_percent" ? "memory" : null
    return lane === null ? [] : [{
      segmentId: point.segmentId,
      lane,
      timestamp: point.timestamp,
      value: lane === "memory" && point.value !== null ? 100 - point.value : point.value,
    }]
  })
  const activity = new Map<string, {
    readonly segmentId: string
    readonly timestamp: number
    running: number
    waiting: number
    oldest: number | null
  }>()
  for (const row of activities) {
    const key = `${row.segmentId}:${row.timestamp}`
    const stored = activity.get(key) ?? {
      segmentId: row.segmentId,
      timestamp: row.timestamp,
      running: 0,
      waiting: 0,
      oldest: null,
    }
    const started = cellNumber(row.values.xact_start)
    if (started !== null) {
      const age = Math.max(0, (row.timestamp - started) / 1_000_000)
      stored.oldest = Math.max(stored.oldest ?? 0, age)
    }
    const client = cellText(row.values.backend_type) === "client backend"
    const leader = row.values.leader_pid !== null && row.values.leader_pid !== undefined
    if (client && !leader && cellText(row.values.state) === "active") {
      if (row.values.wait_event_type === null || row.values.wait_event_type === undefined) stored.running += 1
      else stored.waiting += 1
    }
    activity.set(key, stored)
  }
  for (const stored of activity.values()) {
    lanes.push(
      { segmentId: stored.segmentId, lane: "pg_running", timestamp: stored.timestamp, value: stored.running },
      { segmentId: stored.segmentId, lane: "pg_waiting", timestamp: stored.timestamp, value: stored.waiting },
    )
    if (stored.oldest !== null) {
      lanes.push({
        segmentId: stored.segmentId,
        lane: "pg_oldest_xact",
        timestamp: stored.timestamp,
        value: stored.oldest,
      })
    }
  }
  return lanes.sort((left, right) => left.timestamp - right.timestamp || left.lane.localeCompare(right.lane))
}

function cellText(value: Cell | undefined): string | null {
  return typeof value === "string" || typeof value === "number" || typeof value === "boolean"
    ? String(value)
    : null
}

function cellNumber(value: Cell | undefined): number | null {
  const number = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN
  return Number.isFinite(number) ? number : null
}

function rawFixture(): RealHourFixture | null {
  const candidate = (globalThis as { readonly __KRONIKA_REAL_HOUR__?: unknown }).__KRONIKA_REAL_HOUR__
  return isFixture(candidate) ? candidate : null
}

function isFixture(value: unknown): value is RealHourFixture {
  if (value === null || typeof value !== "object") return false
  const candidate = value as Partial<RealHourFixture>
  return candidate.meta !== undefined
    && typeof candidate.meta.captureFromUs === "string"
    && typeof candidate.meta.captureToUs === "string"
    && Number.isSafeInteger(candidate.meta.segments)
    && table(candidate.os) && table(candidate.pg)
    && Array.isArray(candidate["findings"])
    && candidate.system !== undefined
    && Array.isArray(candidate.system["health"])
    && Array.isArray(candidate.system.load1)
    && Array.isArray(candidate.system.memAvailable)
    && candidate.system.psi !== null
    && typeof candidate.system.psi === "object"
}

function table(value: unknown): value is FixtureTable {
  if (value === null || typeof value !== "object") return false
  const candidate = value as Partial<FixtureTable>
  return Array.isArray(candidate["columns"]) && Array.isArray(candidate.snapshots)
}

function tableRows(table: FixtureTable, logicalName: string): readonly DataRow[] {
  const columns = new Map(table["columns"].map((name, index) => [name, index]))
  const ordinalIndex = columns.get("ordinal")
  const timestampIndex = columns.get("ts")
  return table.snapshots.flatMap((snapshot) => snapshot["rows"].map((cells, rowIndex) => {
    const values = Object.fromEntries(table["columns"].flatMap((name, index) =>
      name === "ordinal" || name === "ts" ? [] : [[name, cells[index] ?? null]],
    ))
    return {
      segmentId: snapshot.segment_id,
      logicalName,
      typeId: snapshot.type_id,
      ordinal: text(ordinalIndex === undefined ? rowIndex : cells[ordinalIndex]),
      timestamp: Number(timestampIndex === undefined ? snapshot.ts : cells[timestampIndex]),
      values,
    }
  }))
}

function seriesRows(
  series: readonly FixturePoint[],
  logicalName: string,
  typeId: string,
  options: {
    readonly field: string
    readonly extra?: Readonly<Record<string, Cell>>
    readonly segmentAt: (timestamp: number) => string
  },
): readonly DataRow[] {
  return series.map(([timestamp, value, explicitSegment], ordinal) => ({
    segmentId: explicitSegment ?? options.segmentAt(Number(timestamp)),
    logicalName,
    typeId,
    ordinal: String(ordinal),
    timestamp: Number(timestamp),
    values: { ...options.extra, [options.field]: value },
  }))
}

function fixturePoints(fixture: RealHourFixture, segmentAt: (timestamp: number) => string, activities: readonly DataRow[]): readonly Point[] {
  const series: readonly [string, string, string, readonly FixturePoint[]][] = [
    ["os_cpu", "1102001", "os_cpu_busy_percent", fixture.system.cpuBusy],
    ["health", "0", "os_health", fixture.system["health"]],
    ["os_loadavg", "1105001", "os_load1", fixture.system.load1],
    ["os_meminfo", "1104001", "os_mem_available_percent", fixture.system.memAvailable],
    ["os_mountinfo", "1112002", "os_min_filesystem_free_percent", fixture.system.minFsFree],
    ["os_vmstat", "1106001", "os_oom_kills", fixture.system.oom],
  ]
  const points = series.flatMap(([logicalName, typeId, name, values]) =>
    values.map(([timestamp, value, segmentId]) => ({
      segmentId: segmentId ?? segmentAt(Number(timestamp)),
      logicalName,
      typeId,
      series: name,
      timestamp: Number(timestamp),
      identity: {},
      value,
    })),
  )
  const active = new Map<string, { readonly segmentId: string; readonly timestamp: number; readonly typeId: string; count: number }>()
  for (const row of activities) {
    if (cellText(row.values.state) !== "active") continue
    const key = `${row.segmentId}:${row.typeId}:${row.timestamp}`
    const stored = active.get(key) ?? { segmentId: row.segmentId, timestamp: row.timestamp, typeId: row.typeId, count: 0 }
    stored.count += 1
    active.set(key, stored)
  }
  return points.concat([...active.values()].map((stored) => ({
    segmentId: stored.segmentId,
    logicalName: "pg_stat_activity",
    typeId: stored.typeId,
    series: "active_backends",
    timestamp: stored.timestamp,
    identity: {},
    value: stored.count,
  })), Object.entries(fixture.system.psi).flatMap(([resource, values]) =>
    values.map(([timestamp, value]) => ({
      segmentId: segmentAt(Number(timestamp)),
      logicalName: "os_psi",
      typeId: "1107001",
      series: "os_psi_some_avg10",
      timestamp: Number(timestamp),
      identity: { resource: Number(resource) },
      value,
    })),
  ))
}

function segmentForTimestamp(table: FixtureTable): (timestamp: number) => string {
  const snapshots = table.snapshots
    .map((snapshot) => ({ segmentId: snapshot.segment_id, timestamp: Number(snapshot.ts) }))
    .sort((left, right) => left.timestamp - right.timestamp)
  return (timestamp) => {
    let selected = snapshots[0]
    let distance = selected === undefined ? Number.POSITIVE_INFINITY : Math.abs(selected.timestamp - timestamp)
    for (const candidate of snapshots) {
      const candidateDistance = Math.abs(candidate.timestamp - timestamp)
      if (candidateDistance < distance || (candidateDistance === distance && candidate.timestamp < (selected?.timestamp ?? Number.POSITIVE_INFINITY))) {
        selected = candidate
        distance = candidateDistance
      }
    }
    return selected?.segmentId ?? "fixture"
  }
}

function fixtureTableTypeNames(fixture: RealHourFixture): ReadonlyMap<string, string> {
  const names = new Map<string, string>()
  for (const [table, logicalName] of [[fixture.os, "os_process"], [fixture.pg, "pg_stat_activity"]] as const) {
    for (const snapshot of table.snapshots) {
      const current = names.get(snapshot.type_id)
      if (current !== undefined && current !== logicalName) {
        throw new Error(`fixture type ${snapshot.type_id} has conflicting table layouts`)
      }
      names.set(snapshot.type_id, logicalName)
    }
  }
  return names
}

function fixtureLogicalName(tableTypeNames: ReadonlyMap<string, string>, typeId: string): string {
  const logicalName = tableTypeNames.get(typeId) ?? FIXTURE_NON_TABLE_TYPE_NAMES[typeId]
  if (logicalName === undefined) throw new Error(`fixture type ${typeId} is not recognized`)
  return logicalName
}

function text(value: Cell | undefined): string {
  if (typeof value === "string") return value
  if (typeof value === "number" || typeof value === "boolean") return String(value)
  return "0"
}
