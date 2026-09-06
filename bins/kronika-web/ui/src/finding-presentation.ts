import { registry } from "kronika:registry"

import { fieldNameForLocator, type DataRow, type Finding } from "./api"
import type { Translate } from "./help"
import { rawText, value } from "./model"
import { findingSemanticField, postgresProjection } from "./postgres-metrics"

const LAYOUTS = new Map(registry.map((layout) => [layout.typeId, layout]))
const LOG_TYPES: Readonly<Record<string, string>> = {
  pg_log_errors: "events.type.errors",
  pg_log_checkpoints: "events.type.checkpoints",
  pg_log_autovacuum: "events.type.autovacuum",
  pg_log_slow_queries: "events.type.slow_queries",
  pg_log_lock_waits: "events.type.lock_waits",
  pg_log_lifecycle: "events.type.lifecycle",
  pg_log_temp_files: "events.type.temp_files",
  pgbouncer_events: "events.type.pgbouncer",
}

const SOURCES: Readonly<Record<string, string>> = {
  health: "events.source.health",
  os_cpu: "events.source.cpu",
  os_meminfo: "events.source.memory",
  os_loadavg: "events.source.load",
  os_mountinfo: "events.source.filesystem",
  os_vmstat: "events.source.oom",
  os_process: "events.source.process",
  pg_stat_statements: "events.source.statement",
  pg_stat_activity: "events.source.activity",
  pg_stat_database: "events.source.database",
  pg_locks: "events.source.locks",
  pg_stat_archiver: "events.source.archiver",
  os_cgroup_memory: "events.source.cgroup_memory",
}

const ERROR_CATEGORIES = [
  "lock", "constraint", "serialization", "timeout", "resource", "data_corruption",
  "system", "connection", "auth", "syntax", "other",
] as const

export function findingKey(finding: Finding): string {
  return `${finding.segmentId}:${finding.typeId}:${finding.rowOrdinal}:${finding.fieldOrdinal}:${finding.timestamp}:${finding.kind}`
}

export function findingOrder(left: Finding, right: Finding): number {
  return left.timestamp - right.timestamp
    || kindOrder(left.kind) - kindOrder(right.kind)
    || textOrder(left.segmentId, right.segmentId)
    || textOrder(left.typeId, right.typeId)
    || textOrder(left.rowOrdinal, right.rowOrdinal)
    || left.fieldOrdinal - right.fieldOrdinal
    || textOrder(left.logicalName, right.logicalName)
}

export function findingSource(finding: Finding, t: Translate): string {
  const log = LOG_TYPES[finding.logicalName]
  if (log !== undefined) {
    const category = finding.logicalName === "pg_log_errors" ? findingLogCategory(finding.category, t) : null
    return category === null ? t(log) : `${t(log)} · ${category}`
  }
  const source = SOURCES[finding.logicalName]
  if (source !== undefined) return t(source)
  return finding.logicalName
}

export function findingLogCategory(category: number | null, t: Translate): string | null {
  const name = category === null ? undefined : ERROR_CATEGORIES[category]
  return name === undefined ? null : t(`events.category.${name}`)
}

export function findingSummary(findings: readonly Finding[], t: Translate): string {
  const summary = summarizeFindings(findings)
  return [
    summary.event === 0 ? null : t("events.scope.event", { count: summary.event }),
    summary.knownBad === 0 ? null : t("events.scope.known_bad", { count: summary.knownBad }),
    summary.spike === 0 ? null : t("events.scope.spike", { count: summary.spike }),
  ].filter((part): part is string => part !== null).join(" · ")
}

export interface FindingSummary {
  readonly event: number
  readonly from: number
  readonly knownBad: number
  readonly spike: number
  readonly to: number
}

export function summarizeFindings(findings: readonly Finding[]): FindingSummary {
  const ordered = findings.slice().sort(findingOrder)
  const logRows = new Set<string>()
  let knownBad = 0
  let spike = 0
  for (const finding of ordered) {
    if (finding.kind === "event" || isEventFindingSource(finding.logicalName)) {
      logRows.add(`${finding.segmentId}:${finding.typeId}:${finding.rowOrdinal}:${finding.timestamp}`)
    } else if (finding.kind === "known_bad") {
      knownBad += 1
    } else {
      spike += 1
    }
  }
  return {
    event: logRows.size,
    from: ordered[0]?.timestamp ?? 0,
    knownBad,
    spike,
    to: ordered.at(-1)?.timestamp ?? 0,
  }
}

export function isEventFindingSource(logicalName: string): boolean {
  return logicalName.startsWith("pg_log_") || logicalName === "pgbouncer_events"
}

export function findingProjection(finding: Finding): readonly string[] {
  if (finding.logicalName === "pg_stat_statements") return postgresProjection(finding.typeId)
  const layout = LAYOUTS.get(finding.typeId)
  return layout?.columns.filter((field) => field !== "ts") ?? []
}


export interface FindingMetric {
  readonly field: string | null
  readonly helpKey: string
  readonly label: string
  readonly labelKey: string
  readonly unit: "bytes_per_second" | "count" | "milliseconds" | "milliseconds_per_call" | "number" | "percent"
  readonly boundary: string | null
}

export function findingMetric(finding: Finding, t: Translate): FindingMetric {
  if (finding.kind === "event") return metric(null, "events.metric.unavailable", "events.metric.unavailable.help", t, "number", null)
  const physical = fieldNameForLocator(finding)
  const semantic = physical === null ? null : findingSemanticField(finding.typeId, physical)
  if (finding.logicalName === "os_cpu") return metric("cpu_busy", "events.metric.cpu_busy", "system.metric.cpu_busy.help", t, "percent", t("events.boundary.cpu"))
  if (finding.logicalName === "os_meminfo") return metric("mem_available", "events.metric.memory_available", "system.metric.mem_available.help", t, "percent", t("events.boundary.memory"))
  if (finding.logicalName === "os_loadavg") return metric("load1", "events.metric.load1", "system.metric.load1.help", t, "number", t("events.boundary.load"))
  if (finding.logicalName === "os_mountinfo") return metric("free_bytes", "events.metric.filesystem_used", "events.metric.filesystem_used.help", t, "percent", t("events.boundary.filesystem"))
  if (finding.logicalName === "os_vmstat" && physical === "oom_kill") return metric(physical, "events.metric.oom_kill", "system.metric.oom_kill.help", t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "pg_stat_database" && physical === "deadlocks") return metric(physical, "events.metric.deadlocks", "pg.field.deadlocks.help", t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "pg_stat_database" && physical === "checksum_failures") return metric(physical, "pg.field.checksum_failures.label", "pg.field.checksum_failures.help", t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "pg_stat_database" && (physical === "sessions_fatal" || physical === "sessions_killed")) return metric(physical, `pg.field.${physical}.label`, `pg.field.${physical}.help`, t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "pg_stat_database" && (physical === "frozen_xid_age" || physical === "min_mxid_age")) return metric(physical, `pg.field.${physical}.label`, `pg.field.${physical}.help`, t, "count", t("events.boundary.wraparound"))
  if (finding.logicalName === "pg_stat_archiver") return metric("failed_count", "pg.field.failed_count.label", "pg.field.failed_count.help", t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "os_cgroup_memory") return metric("oom_kill", "system.metric.oom_kill.label", "system.metric.oom_kill.help", t, "count", t("events.boundary.increased"))
  if (finding.logicalName === "pg_locks") return metric("blocked_by", "pg.field.blocked_by.label", "pg.field.blocked_by.help", t, "count", t("events.boundary.contention"))
  if (finding.logicalName === "pg_log_errors") return metric("category", "events.metric.data_corruption", "events.metric.data_corruption.help", t, "count", t("events.boundary.data_corruption"))
  if (finding.logicalName === "pg_stat_activity") return metric("active_backends", "events.metric.active_backends", "events.metric.active_backends.help", t, "count", t("events.boundary.active_backends"))
  if (finding.logicalName === "pg_log_slow_queries") return metric("max_duration_ms", "events.metric.slow_query", "events.metric.slow_query.help", t, "milliseconds", t("events.boundary.slow_query"))
  if (finding.logicalName === "health") return metric(physical, "events.metric.overall_health", "lane.health.overall_health.help", t, "percent", t("events.boundary.health"))
  if (finding.logicalName === "os_process") return metric("read_bytes", "events.metric.process_read", "col.read_bytes.help", t, "bytes_per_second", null)
  if (semantic !== null) {
    return metric(semantic, `pg.field.${semantic}.label`, `pg.field.${semantic}.help`, t, semantic === "mean_exec_ms_per_call" ? "milliseconds_per_call" : "number", null)
  }
  return metric(physical, physical ?? "events.metric.unavailable", "events.metric.unavailable.help", t, unitFor(physical), null)
}





function metric(field: string | null, labelKey: string, helpKey: string, t: Translate, unit: FindingMetric["unit"], boundary: string | null): FindingMetric {
  return { field, helpKey, label: t(labelKey), labelKey, unit, boundary }
}

function unitFor(field: string | null): FindingMetric["unit"] {
  if (field?.endsWith("_bytes") === true) return "count"
  if (field?.endsWith("_ms") === true) return "milliseconds"
  return "number"
}

function kindOrder(kind: Finding["kind"]): number {
  return kind === "event" ? 0 : kind === "known_bad" ? 1 : 2
}

function textOrder(left: string, right: string): number { return left < right ? -1 : left > right ? 1 : 0 }
