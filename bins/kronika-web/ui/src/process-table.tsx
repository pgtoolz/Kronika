import { useCallback, useEffect, useMemo, useState, type Dispatch } from "react"

import { acceptResponse, loadSeries, type Cell, type DataRow, type Finding, type SnapshotRows } from "./api"
import { buildMetricSamples } from "./chart"
import { useDisplayTime } from "./display-time-context"
import { EntityTable, type EntityColumn, type TableOrder } from "./entity-table"
import { LabelHelp, type Translate } from "./help"
import {
  asNumber,
  cores,
  humanBytes,
  humanCores,
  humanDuration,
  humanPercent,
  identifier,
  measure,
  processCommand,
  processKey,
  rawText,
  processCpuTime,
  processTty,
  stateText,
  value,
  type Lens,
  type Locale,
} from "./model"
import { canonicalSearch } from "./search"
import type { SearchRequestState } from "./search-request"
import { readingAt, SeriesChart, type ChartPoint } from "./series-chart"
import type { TableRequestPhase } from "./table-request"
import { filterProcessForest } from "./process-tree"

export interface Field {
  readonly id: string
  readonly field?: string
  readonly label: string
  readonly help: string
  readonly kind: "id" | "user" | "command" | "tree_command" | "state" | "number" | "rate" | "cores" | "kib" | "bytes" | "ns" | "percent" | "seconds" | "cpu_time" | "timestamp" | "start_time" | "tty"
  readonly size: number
  readonly sticky?: "pid" | "command"
}

const PID: Field = { id: "pid", field: "pid", label: "col.pid.label", help: "col.pid.help", kind: "id", size: 82, sticky: "pid" }
const COMMAND: Field = { id: "command", label: "col.command.label", help: "col.command.help", kind: "command", size: 340, sticky: "command" }
// ps keeps identity on the left and the full command line last, pinning nothing.
const TREE_PID: Field = { id: "pid", field: "pid", label: "col.pid.label", help: "col.pid.help", kind: "id", size: 82 }
const TREE_COMMAND: Field = { id: "command", label: "col.command.label", help: "col.command.help", kind: "tree_command", size: 620 }
const STATE: Field = { id: "state", field: "state", label: "col.state.label", help: "col.state.help", kind: "state", size: 50 }
const USER: Field = { id: "user", field: "user", label: "col.user.label", help: "col.user.help", kind: "user", size: 88 }
export const PROCESS_USER_FIELDS: readonly Field[] = [
  USER,
  { id: "effective_user", field: "effective_user", label: "col.effective_user.label", help: "col.effective_user.help", kind: "user", size: 114 },
]

export const LENS_FIELDS: Readonly<Record<Lens, readonly Field[]>> = {
  generic: [
    PID, COMMAND,
    field("id", "ppid", "col.ppid", 70), ...PROCESS_USER_FIELDS,
    field("id", "gid", "col.gid", 70), field("id", "egid", "col.egid", 70),
    field("number", "num_threads", "col.threads", 84), field("tty", "tty", "col.tty", 70),
    field("id", "exit_signal", "col.exit_signal", 70), STATE,
  ],
  cpu: [
    PID, COMMAND, field("cores", "utime", "col.utime", 84),
    field("cores", "stime", "col.stime", 84), field("ns", "rundelay_ns", "col.rundelay", 96),
    field("rate", "blkdelay_ticks", "col.blkdelay", 84), field("rate", "nvcsw", "col.nvcsw", 84),
    field("rate", "nivcsw", "col.nivcsw", 84), field("id", "curcpu", "col.curcpu", 70), field("number", "nice", "col.nice", 84),
    field("number", "prio", "col.prio", 84), field("number", "rtprio", "col.rtprio", 84), field("id", "policy", "col.policy", 70),
    STATE,
  ],
  memory: [
    PID, COMMAND, field("kib", "rmem_kb", "col.rmem", 96), field("kib", "vmem_kb", "col.vmem", 96),
    field("kib", "vswap_kb", "col.vswap", 96), field("rate", "minflt", "col.minflt", 84),
    field("rate", "majflt", "col.majflt", 84), STATE,
  ],
  disk: [
    PID, COMMAND, field("bytes", "read_bytes", "col.read_bytes", 96), field("bytes", "write_bytes", "col.write_bytes", 96),
    field("rate", "syscr", "col.syscr", 84),
    field("rate", "syscw", "col.syscw", 84), field("bytes", "rchar", "col.rchar", 96),
    field("bytes", "wchar", "col.wchar", 96), field("bytes", "cancelled_write_bytes", "col.cancelled_write", 96),
    field("rate", "blkdelay_ticks", "col.blkdelay", 84), STATE,
  ],
  // The ps axufwwww column order: identity, load, memory, terminal, state, times, command.
  tree: [
    USER, TREE_PID,
    field("percent", "cpu_percent", "col.cpu_percent", 72), field("percent", "mem_percent", "col.mem_percent", 72),
    field("kib", "vmem_kb", "col.vmem", 96), field("kib", "rmem_kb", "col.rmem", 96), field("tty", "tty", "col.tty", 70),
    field("state", "process_stat", "col.stat", 62), field("start_time", "starttime", "col.starttime", 92),
    field("cpu_time", "cpu_time_seconds", "col.cpu_time", 84),
    TREE_COMMAND,
  ],
}

type SummaryKind = "B" | "B/s" | "cores" | "count" | "ms/s" | "1/s"

export interface ProcessSummaryMetric {
  readonly field: string
  readonly help: string
  readonly key: string
  readonly kind: SummaryKind
}

const IDENTITY_SUMMARY: readonly ProcessSummaryMetric[] = [
  summaryMetric("processes", "process.summary.processes", "count"),
  summaryMetric("threads", "process.summary.threads", "count"),
  summaryMetric("runnable", "process.summary.running", "count"),
  summaryMetric("postgresql", "process.summary.postgresql", "count"),
]

export const PROCESS_SUMMARY_METRICS: Readonly<Record<Lens, readonly ProcessSummaryMetric[]>> = {
  generic: IDENTITY_SUMMARY,
  cpu: [
    summaryMetric("user_cores", "process.summary.user_time", "cores"),
    summaryMetric("system_cores", "process.summary.system_time", "cores"),
    summaryMetric("run_delay_ms_per_second", "process.summary.run_delay", "ms/s"),
    summaryMetric("context_switches_per_second", "process.summary.context_switches", "1/s"),
  ],
  memory: [
    summaryMetric("resident_kib", "process.summary.resident", "B"),
    summaryMetric("virtual_kib", "process.summary.virtual", "B"),
    summaryMetric("swap_kib", "process.summary.swap", "B"),
    summaryMetric("major_faults_per_second", "process.summary.major_faults", "1/s"),
  ],
  disk: [
    summaryMetric("read_bytes_per_second", "process.summary.read", "B/s"),
    summaryMetric("write_bytes_per_second", "process.summary.written", "B/s"),
    summaryMetric("read_calls_per_second", "process.summary.read_calls", "1/s"),
    summaryMetric("write_calls_per_second", "process.summary.write_calls", "1/s"),
  ],
  tree: IDENTITY_SUMMARY,
}

export const PROCESS_SUMMARY_FIELDS: readonly string[] = [...new Set(Object.values(PROCESS_SUMMARY_METRICS).flatMap((metrics) => metrics.map(({ field }) => field)))]

export interface ProcessSummaryState {
  readonly hour: number | null
  readonly history: readonly DataRow[]
  readonly status: "loading" | "ready" | "empty" | "error"
}

export type ProcessSummaryAction =
  | { readonly hour: number; readonly type: "loading" }
  | { readonly hour: number; readonly type: "error" }
  | { readonly hour: number; readonly type: "loaded"; readonly rows: readonly DataRow[] }

export const EMPTY_PROCESS_SUMMARY: ProcessSummaryState = { hour: null, history: [], status: "loading" }

export function processSummaryReducer(state: ProcessSummaryState, action: ProcessSummaryAction): ProcessSummaryState {
  if (action.type === "loading") {
    return action.hour === state.hour
      ? { ...state, status: "loading" }
      : { hour: action.hour, history: [], status: "loading" }
  }
  if (action.hour !== state.hour) return state
  if (action.type === "error") return { ...state, status: "error" }
  return { hour: action.hour, history: action.rows, status: action.rows.length === 0 ? "empty" : "ready" }
}

export function ProcessSummary({ cursor, dispatch, enabled, hour, lens, locale, state, t }: {
  readonly cursor: number
  readonly dispatch: Dispatch<ProcessSummaryAction>
  readonly enabled: boolean
  readonly hour: number
  readonly lens: Lens
  readonly locale: Locale
  readonly state: ProcessSummaryState
  readonly t: Translate
}) {
  const metrics = PROCESS_SUMMARY_METRICS[lens]
  const history = state.hour === hour ? state.history : []
  const status = state.hour === hour ? state.status : "loading"
  useEffect(() => {
    if (!enabled || (state.hour === hour && state.status !== "loading")) return
    const controller = new AbortController()
    dispatch({ hour, type: "loading" })
    acceptResponse(loadSeries(hour, "os_process_summary", {}, PROCESS_SUMMARY_FIELDS, controller.signal), controller.signal,
      (rows) => dispatch({ hour, type: "loaded", rows }), () => dispatch({ hour, type: "error" }))
    return () => controller.abort()
  }, [enabled, hour])
  const statusKey = status === "loading" ? "process.summary.loading" : status === "error" ? "process.summary.error" : status === "empty" ? "status.no_data" : null
  return <section aria-label={t("process.summary.title")} className="process-summary-inline flex min-w-0 flex-1 items-center gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden min-[521px]:max-[760px]:flex-wrap min-[521px]:max-[760px]:overflow-visible" data-status={status}>
    {metrics.map((metric) => {
      const output = processSummaryOutput(readingAt(processSummaryPoints(history, metric), cursor), metric, locale, t)
      return <div className="flex h-[25px] flex-none items-center gap-1.5 px-2" key={metric.field}>
        <span className="whitespace-nowrap text-xs font-medium text-fg3">{t(metric.key)}</span>
        <strong className="flex-none font-mono text-xs tabular-nums text-fg">{output}</strong>
        <LabelHelp helpKey={metric.help} iconOnly labelKey={metric.key} t={t} testId={`process-summary-help-${metric.field}`} />
      </div>
    })}
    {statusKey !== null && <p aria-live="polite" className="m-0 flex h-[25px] flex-none items-center px-2 text-xs font-medium text-fg4" data-testid="process-summary-status">{t(statusKey)}</p>}
  </section>
}

export function processSummaryPoints(rows: readonly DataRow[], metric: ProcessSummaryMetric): readonly ChartPoint[] {
  return buildMetricSamples(rows, (row) => {
    if (!Object.hasOwn(row.values, metric.field)) return undefined
    const stored = asNumber(value(row, metric.field))
    return stored === null ? null : metric.kind === "B" ? stored * 1024 : stored
  })
}

export function processSummaryOutput(reading: number | null, metric: ProcessSummaryMetric, locale: Locale, t: Translate): string {
  if (reading === null) return "—"
  if (metric.kind === "B") return humanBytes(reading, locale)
  if (metric.kind === "B/s") return humanBytes(reading, locale, t("unit.per_second"))
  if (metric.kind === "cores") return humanCores(reading, locale)
  if (metric.kind === "ms/s") return humanDuration(reading, locale, "milliseconds", t("unit.per_second"))
  return measure(reading, locale, metric.kind === "1/s" ? t("unit.per_second") : "")
}

export function processSummaryFormat(metric: ProcessSummaryMetric, t: Translate): (reading: number, locale: Locale) => string {
  return (reading, locale) => processSummaryOutput(reading, metric, locale, t)
}

export function processSummaryUnit(metric: ProcessSummaryMetric, locale: Locale, t: Translate): string {
  if (metric.kind === "B" || metric.kind === "B/s") return metric.kind
  if (metric.kind === "cores") return t("unit.cores").trim()
  if (metric.kind === "ms/s") return ""
  if (metric.kind === "1/s") return `1${t("unit.per_second")}`
  return locale === "ru" ? "количество" : "count"
}

function summaryMetric(field: string, key: string, kind: SummaryKind): ProcessSummaryMetric {
  return { field, help: `${key}.help`, key, kind }
}

export function ProcessTable({
  contextLabel,
  finding,
  findingField,
  densePageState,
  lens,
  linkedPids,
  locale,
  metadata,
  onLoadMore,
  onOrder,
  onContextClear,
  onPattern,
  onSelect,
  onRetry,
  order,
  pattern,
  requestPhase,
  rows,
  searchRequest,
  selectedKey,
  t,
  ticksPerSecond,
}: {
  readonly contextLabel?: string | undefined
  readonly finding?: Finding | null
  readonly findingField?: string | null | undefined
  readonly densePageState: "idle" | "loading" | "error"
  readonly lens: Lens
  readonly linkedPids: ReadonlySet<number>
  readonly locale: Locale
  readonly metadata?: SnapshotRows | undefined
  readonly onLoadMore: () => void
  readonly onOrder: (order: TableOrder | null) => void
  readonly onContextClear?: (() => void) | undefined
  readonly onPattern: (pattern: string) => void
  readonly order: TableOrder | null
  readonly onSelect: (row: DataRow) => void
  readonly onRetry: () => void
  readonly pattern: string
  readonly requestPhase: TableRequestPhase
  readonly rows: readonly DataRow[]
  readonly searchRequest: SearchRequestState
  readonly selectedKey: string | null
  readonly t: Translate
  readonly ticksPerSecond: number | null
}) {
  const columns = useMemo<readonly EntityColumn[]>(() => LENS_FIELDS[lens].map((field) => {
    const help = processHeaderHelp(field)
    return {
      field: field.id,
      ...(field.kind === "command" || field.kind === "tree_command" ? { filterValue: processCommand } : {}),
      ...(help === undefined ? {} : { help }),
      kind: entityKind(field.kind),
      label: field.label,
      render: (row) => <CellValue field={field} locale={locale} linked={linkedPids.has(asNumber(value(row, "pid")) ?? -1)} onSearch={onPattern} row={row} t={t} ticksPerSecond={ticksPerSecond} />,
      sortValue: (row) => sortable(row, field),
      sortable: lens !== "tree" && field.kind !== "user",
      ...(field.sticky === undefined ? {} : { sticky: `sticky-${field.sticky}` }),
      ...(field.id === "state" || field.id === "process_stat" ? { expandToHeader: false } : {}),
      width: field.size,
    }
  }), [lens, linkedPids, locale, onPattern, t, ticksPerSecond])
  const activeOrder = order ?? processTableDefaultOrder(lens)
  const filterRows = useCallback((candidates: readonly DataRow[], query: string) => filterProcessForest(candidates, query, ticksPerSecond), [ticksPerSecond])
  const canLoadMore = metadata?.hasMore === true && metadata.nextCursor !== null
  const paging = densePageState !== "idle" || canLoadMore
    ? <button disabled={densePageState === "loading"} onClick={densePageState === "error" ? onRetry : onLoadMore} type="button">
      {densePageState === "loading" ? "…" : densePageState === "error" ? "↻" : "+"}
    </button>
    : undefined
  const status = <strong>{t("pg.table.shown", {
    returned: new Intl.NumberFormat(locale).format(rows.length),
    eligible: new Intl.NumberFormat(locale).format(metadata?.eligible ?? rows.length),
  })}</strong>
  return <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
    <EntityTable
    className="process-table flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-s1"
    columns={columns}
    requestPhase={requestPhase}
    contextLabel={contextLabel}
    empty={t("table.empty")}
    finding={finding}
    findingField={findingField}
    filterRows={lens === "tree" ? filterRows : undefined}
    label={t("table.processes")}
    locale={locale}
    onNearEnd={densePageState === "idle" && canLoadMore ? onLoadMore : undefined}
    onOrder={onOrder}
    onContextClear={onContextClear}
    onPattern={onPattern}
    onSelect={onSelect}
    order={activeOrder}
    pattern={pattern}
    rowKey={processKey}
    rowLabel={(row) => processRowLabel(row, lens, t)}
    rows={rows}
    searchRequest={searchRequest}
    searchSurface="os_process"
    serverSorted
    selectedKey={selectedKey}
    status={status}
    t={t}
    testId="process-table"
    />
    {paging !== undefined && <div className="lens-tabs flex-none" data-testid="table-paging">{paging}</div>}
  </div>
}

export function processRowLabel(row: DataRow, lens: Lens, t: Translate): string {
  const pid = identifier(value(row, "pid"))
  if (lens !== "tree") return t("table.activate", { pid })
  const depth = asNumber(value(row, "process_tree_depth")) ?? 0
  const command = processCommand(row)
  if (depth === 0) return t("process.tree.row.root", { command, pid })
  return t("process.tree.row.child", {
    command,
    depth,
    parent: identifier(value(row, "process_tree_parent_pid")),
    pid,
  })
}

export function processTableDefaultOrder(lens: Lens): TableOrder {
  if (lens === "generic" || lens === "tree") return { column: "pid", descending: false }
  if (lens === "memory") return { column: "rmem_kb", descending: true }
  if (lens === "disk") return { column: "read_bytes", descending: true }
  return { column: "utime", descending: true }
}

export function formatCell(kind: Field["kind"], cell: Cell, locale: Locale, t: Translate, ticksPerSecond: number | null): string {
  switch (kind) {
    case "state": return stateText(cell)
    case "number": return measure(cell, locale)
    case "rate": return measure(cell, locale, t("unit.per_second"))
    case "cores": return cores(cell, locale, ticksPerSecond, "")
    case "kib": return humanBytes(kib(asNumber(cell)), locale)
    case "bytes": return humanBytes(cell, locale, t("unit.per_second"))
    case "ns": return humanDuration(cell, locale, "nanoseconds", t("unit.per_second"))
    case "seconds": return humanDuration(cell, locale, "seconds")
    case "cpu_time": return processCpuTime(cell)
    case "tty": return processTty(cell)
    case "percent": return humanPercent(cell, locale)
    case "id": case "user": return identifier(cell)
    case "command": case "tree_command": case "timestamp": case "start_time": return ""
  }
}

export function CellValue({ field, linked, locale, onSearch, row, t, ticksPerSecond }: { readonly field: Field; readonly linked: boolean; readonly locale: Locale; readonly onSearch?: ((pattern: string) => void) | undefined; readonly row: DataRow; readonly t: Translate; readonly ticksPerSecond: number | null }) {
  const time = useDisplayTime()
  const cell = field.field === undefined ? null : value(row, field.field)
  const isCommand = field.kind === "command" || field.kind === "tree_command"
  const treePrefix = field.kind === "tree_command" ? rawText(value(row, "process_tree_prefix")) ?? "" : ""
  const timestamp = field.kind === "timestamp" || field.kind === "start_time" ? asNumber(cell) : null
  const output = isCommand ? processCommand(row)
    : field.kind === "user" ? processUser(row, field)
    : field.kind === "start_time" ? time.startTime(timestamp)
    : field.kind === "timestamp" ? (timestamp === null ? "—" : time.timestamp(timestamp))
    : formatCell(field.kind, cell, locale, t, ticksPerSecond)
  const userSearch = field.kind === "user" ? processUserSearch(row, field) : null
  return <span className={`block overflow-hidden text-ellipsis whitespace-nowrap ${isCommand || field.kind === "user" ? "w-full text-fg" : "numeric-cell tabular-nums"}`} title={output}>{treePrefix !== "" && <span aria-hidden="true" className="process-tree-prefix">{treePrefix}</span>}{isCommand && linked && <span className="mr-1.5 inline-block border border-accent-line bg-accent-soft px-1 py-0.5 align-[1px] font-sans text-xs font-semibold text-accent2">PG</span>}{userSearch !== null && onSearch !== undefined
    ? <button className="max-w-full cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap border-0 bg-transparent p-0 text-left text-accent3 underline decoration-dotted underline-offset-2" data-testid={`process-user-filter-${field.id}`} onClick={(event) => { event.stopPropagation(); onSearch(userSearch) }} type="button">{output}</button>
    : output}</span>
}

// Use UID only when no captured username exists.
export function processUser(row: DataRow, field: Field): string {
  const name = field.field === undefined ? null : rawText(value(row, field.field))
  if (name !== null && name.trim() !== "") return name
  return identifier(value(row, field.id === "effective_user" ? "euid" : "uid"))
}

export function processUserSearch(row: DataRow, field: Field): string | null {
  if (field.kind !== "user" || field.field === undefined) return null
  const name = rawText(value(row, field.field))
  if (name === null || name.trim() === "") return null
  return canonicalSearch([{ key: field.id, value: name }], "os_process")
}

function sortable(row: DataRow, field: Field): string | number | null {
  if (field.kind === "command" || field.kind === "tree_command") return processCommand(row)
  if (field.kind === "user") return processUser(row, field)
  const cell = field.field === undefined ? null : value(row, field.field)
  if (field.kind === "state") return stateText(cell)
  if (field.kind === "id" && field.id !== "pid") return rawText(cell)
  return asNumber(cell) ?? rawText(cell)
}

function entityKind(kind: Field["kind"]): NonNullable<EntityColumn["kind"]> {
  if (kind === "id") return "id"
  if (kind === "command" || kind === "tree_command" || kind === "state" || kind === "user") return "text"
  if (kind === "tty") return "id"
  if (kind === "timestamp" || kind === "start_time") return "timestamp"
  return "number"
}

function processHeaderHelp(field: Field): string | undefined {
  return field.id === "pid" || field.id === "command" || field.id === "num_threads" ? undefined : field.help
}

function kib(number: number | null): number | null {
  return number === null ? null : number * 1024
}

function field(kind: Field["kind"], id: string, key: string, size: number): Field {
  return { id, field: id, label: `${key}.label`, help: `${key}.help`, kind, size }
}
