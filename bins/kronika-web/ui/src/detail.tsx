import { Copy } from "lucide-react"
import { useMemo, useState, type ReactNode } from "react"

import { copyText } from "./clipboard"
import type { Cell, DataRow } from "./api"
import { buildMetricSamples } from "./chart"
import { DetailList, DetailRow, type DetailValueRole } from "./detail-list"
import { useDisplayTime } from "./display-time-context"
import { LabelHelp, type Translate } from "./help"
import { InspectorChartPortal } from "./inspector"
import type { HistoryStatus } from "./history-request"
import {
  asNumber,
  humanBytes,
  humanCores,
  humanDuration,
  identifier,
  measure,
  processCommand,
  rawText,
  value,
  type Lens,
  type Locale,
} from "./model"
import { activityDurationMs, backendAgeMs, stateDurationMs, transactionDurationMs } from "./postgres-activity"
import { CellValue, formatCell, LENS_FIELDS, PROCESS_USER_FIELDS, type Field } from "./process-table"
import { SeriesChart, type ChartPoint } from "./series-chart"

interface HistoryField {
  readonly field: string
  readonly key: string
  readonly kind: Field["kind"]
  readonly counter?: true
  readonly scale?: "nonnegative" | "signed"
  readonly unit?: string
}

export interface ProcessHistorySeries extends HistoryField {
  readonly points: readonly ChartPoint[]
}

const CPU_HISTORY: readonly HistoryField[] = [
  { counter: true, field: "utime", key: "col.utime", kind: "cores" },
  { counter: true, field: "stime", key: "col.stime", kind: "cores" },
  { counter: true, field: "rundelay_ns", key: "col.rundelay", kind: "ns" },
  { counter: true, field: "blkdelay_ticks", key: "col.blkdelay", kind: "rate" },
  { counter: true, field: "nvcsw", key: "col.nvcsw", kind: "rate" },
  { counter: true, field: "nivcsw", key: "col.nivcsw", kind: "rate" },
  { counter: true, field: "minflt", key: "col.minflt", kind: "rate" },
  { counter: true, field: "majflt", key: "col.majflt", kind: "rate" },
]
const MEMORY_HISTORY: readonly HistoryField[] = [
  { field: "rmem_kb", key: "col.rmem", kind: "kib" },
  { field: "vmem_kb", key: "col.vmem", kind: "kib" },
  { field: "vswap_kb", key: "col.vswap", kind: "kib" },
  { counter: true, field: "minflt", key: "col.minflt", kind: "rate" },
  { counter: true, field: "majflt", key: "col.majflt", kind: "rate" },
]
const DISK_HISTORY: readonly HistoryField[] = [
  { counter: true, field: "read_bytes", key: "col.read_bytes", kind: "bytes" },
  { counter: true, field: "write_bytes", key: "col.write_bytes", kind: "bytes" },
  { counter: true, field: "cancelled_write_bytes", key: "col.cancelled_write", kind: "bytes" },
  { counter: true, field: "syscr", key: "col.syscr", kind: "rate" },
  { counter: true, field: "syscw", key: "col.syscw", kind: "rate" },
  { counter: true, field: "rchar", key: "col.rchar", kind: "bytes" },
  { counter: true, field: "wchar", key: "col.wchar", kind: "bytes" },
  { counter: true, field: "blkdelay_ticks", key: "col.blkdelay", kind: "rate" },
]

const PROCESS_HISTORY: Readonly<Record<Lens, readonly HistoryField[]>> = {
  generic: [{ field: "num_threads", key: "col.threads", kind: "number" }],
  tree: [...CPU_HISTORY, ...MEMORY_HISTORY.filter((field) => field.field !== "minflt" && field.field !== "majflt"), ...DISK_HISTORY],
  cpu: CPU_HISTORY,
  memory: MEMORY_HISTORY,
  disk: DISK_HISTORY,
}

export const PROCESS_HISTORY_FIELDS: readonly string[] = [
  "pid",
  ...new Set(Object.values(PROCESS_HISTORY).flatMap((lens) => lens.map((field) => field.field))),
]

export function DetailDock({
  activity,
  cursor,
  hour,
  lens,
  locale,
  onCursor,
  process,
  processHistory,
  processHistoryStatus,
  t,
  ticksPerSecond,
}: {
  readonly activity: DataRow | null
  readonly cursor: number
  readonly hour: number
  readonly lens: Lens
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly process: DataRow
  readonly processHistory: readonly DataRow[]
  readonly processHistoryStatus: HistoryStatus
  readonly ticksPerSecond: number | null
  readonly t: Translate
}) {
  const commandCell = value(process, "cmdline")
  const pid = identifier(value(process, "pid"))
  const commandPath = rawText(commandCell)?.trim() ? `/proc/${pid}/cmdline` : `/proc/${pid}/comm`
  const history = useMemo(
    () => processLensHistory(processHistory, lens),
    [lens, processHistory],
  )
  const availableHistory = useMemo(
    () => history.filter((series) => series.points.some((point) => point.value !== null && Number.isFinite(point.value))),
    [history],
  )
  const selectableHistory = availableHistory.length === 0 ? history : availableHistory
  const [selectedHistoryField, setSelectedHistoryField] = useState<string | null>(null)
  const selectedHistory = selectableHistory.find((series) => series.field === selectedHistoryField) ?? selectableHistory[0] ?? null
  const selectedHistoryPoints = useMemo(
    () => selectedHistory === null ? [] : processChartPoints(selectedHistory, ticksPerSecond),
    [selectedHistory, ticksPerSecond],
  )
  return (
    <aside
      aria-label={t("detail.process.title")}
      className="process-detail-dock min-h-0 bg-s2 p-3"
      data-testid={activity === null ? "process-dock" : "pg-linked-dock"}
    >
      <section className="mt-2 flex items-center gap-1.5 border-y border-line2 bg-s1 px-1.5 py-[5px]" title={commandPath}>
        <code className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-sm leading-[1.5] text-fg font-mono hover:overflow-visible hover:whitespace-normal hover:[text-overflow:clip] hover:[overflow-wrap:anywhere]" data-testid="process-cmdline">{processCommand(process)}</code>
        <button aria-label={t("common.raw")} className="inline-flex flex-none cursor-pointer items-center justify-center rounded-[var(--radius-xs)] border-0 bg-transparent p-1 text-accent3 transition-colors hover:bg-s3" onClick={() => void copyText(processCommand(process), t("clipboard.manual"))} type="button"><Copy aria-hidden="true" size={12} /></button>
      </section>
      <DetailList>
        <DetailField help="col.pid.help" label="col.pid.label" t={t} value={identifier(value(process, "pid"))} valueRole="machine" />
        {processDetailFields(lens, process).map((field) => <DetailField help={field.help} key={field.id} label={field.label} t={t} value={<CellValue field={field} linked={false} locale={locale} row={process} t={t} ticksPerSecond={ticksPerSecond} />} valueRole={processDetailValueRole(field)} />)}
      </DetailList>
      <InspectorChartPortal identity={`process:${identifier(value(process, "pid"))}`}>
      <section aria-label={t(`lens.${lens}`)} className="process-history grid min-w-0 gap-[7px]" data-testid="process-history">
        <div aria-label={t(`lens.${lens}`)} className="history-selector flex max-w-full gap-[5px] overflow-x-auto p-px pb-[3px] [scrollbar-width:thin]" role="group">
          {selectableHistory.map((series) => (
            <button
              aria-pressed={series.field === selectedHistory?.field}
              className="min-h-[28px] flex-none cursor-pointer border border-line3 bg-s2 px-[7px] py-1 text-xs text-fg2 aria-pressed:border-accent aria-pressed:bg-accent-soft aria-pressed:text-fg"
              data-testid={`process-history-metric-${series.field}`}
              key={series.field}
              onClick={() => setSelectedHistoryField(series.field)}
              type="button"
            >
              {t(`${series.key}.label`)}
            </button>
          ))}
        </div>
        {selectedHistory !== null && (
          <SeriesChart
            cursor={cursor}
            helpKey={`${selectedHistory.key}.help`}
            hour={hour}
            key={selectedHistory.field}
            labelKey={`${selectedHistory.key}.label`}
            locale={locale}
            format={(reading, place) => formatProcessChartValue(selectedHistory.kind, reading, place, t, ticksPerSecond)}
            onCursor={onCursor}
            points={selectedHistoryPoints}
            scale={selectedHistory.scale ?? "nonnegative"}
            status={processHistoryStatus}
            t={t}
            unit={selectedHistory.unit ?? processChartUnit(selectedHistory.kind, t, ticksPerSecond)}
          />
        )}
      </section>
      </InspectorChartPortal>

    </aside>
  )
}

export function processDetailFields(lens: Lens, process: DataRow): readonly Field[] {
  const fields = lens === "generic" ? LENS_FIELDS[lens] : [...PROCESS_USER_FIELDS, ...LENS_FIELDS[lens]]
  return fields.filter((field) => field.id !== "command" && field.id !== "pid" && field.field !== undefined && (field.kind === "user" || value(process, field.field) !== null))
}

export function processDetailValueRole(field: Pick<Field, "kind">): DetailValueRole {
  return ["id", "user", "command", "tree_command", "state", "timestamp", "start_time", "tty"].includes(field.kind) ? "machine" : "semantic"
}

export function processChartPoints(
  series: ProcessHistorySeries,
  ticksPerSecond: number | null,
): readonly ChartPoint[] {
  const divisor = series.kind === "cores" && ticksPerSecond !== null && ticksPerSecond > 0
    ? ticksPerSecond
    : series.kind === "ns" ? 1_000_000 : 1
  const multiplier = series.kind === "kib" ? 1024 : 1
  if (divisor === 1 && multiplier === 1) return series.points
  return series.points.map((point) => ({
    ...point,
    value: point.value === null ? null : point.value * multiplier / divisor,
  }))
}

export function processChartUnit(kind: Field["kind"], t: Translate, ticksPerSecond: number | null): string {
  if (kind === "cores") return ticksPerSecond !== null && ticksPerSecond > 0 ? t("unit.cores").trim() : `ticks${t("unit.per_second")}`
  if (kind === "ns") return ""
  if (kind === "kib") return "B"
  if (kind === "bytes") return `B${t("unit.per_second")}`
  if (kind === "rate") return `#${t("unit.per_second")}`
  return "#"
}

function formatProcessChartValue(
  kind: Field["kind"],
  reading: number,
  locale: Locale,
  t: Translate,
  ticksPerSecond: number | null,
): string {
  if (kind === "cores" && ticksPerSecond !== null && ticksPerSecond > 0) return humanCores(reading, locale, t("unit.cores"))
  if (kind === "cores") return measure(reading, locale)
  if (kind === "ns") return humanDuration(reading, locale, "milliseconds", t("unit.per_second"))
  if (kind === "kib") return humanBytes(reading, locale)
  if (kind === "bytes") return humanBytes(reading, locale, t("unit.per_second"))
  return formatCell(kind, reading, locale, t, ticksPerSecond)
}

function DetailField({ help, label, t, value: output, valueRole }: { readonly help: string; readonly label: string; readonly t: Translate; readonly value: ReactNode; readonly valueRole: DetailValueRole }) {
  return <DetailRow term={<LabelHelp helpKey={help} labelKey={label} t={t} />} valueRole={valueRole}>{output}</DetailRow>
}

function Timestamp({ cell, raw, t }: { readonly cell?: Cell; readonly raw?: number; readonly t: Translate }) {
  const time = useDisplayTime()
  const timestamp = raw ?? asNumber(cell ?? null)
  if (timestamp === null || timestamp === undefined) return <>—</>
  return <span className="inline-flex items-center gap-[5px]"><span>{time.timestamp(timestamp)}</span><button aria-label={t("common.raw")} className="inline-flex cursor-pointer items-center justify-center border border-line4 bg-transparent px-[3px] py-0.5 text-accent3" onClick={() => void copyText(String(timestamp), t("clipboard.manual"))} type="button"><Copy aria-hidden="true" size={12} /></button></span>
}

function formatActivity(cell: Cell, kind: string, locale: Locale, t: Translate): ReactNode {
  if (kind === "id") return identifier(cell)
  if (kind === "number") return measure(cell, locale)
  if (kind === "time") return <Timestamp cell={cell} t={t} />
  return rawText(cell) ?? "—"
}


export function processLensHistory(
  rows: readonly DataRow[],
  lens: Lens,
): readonly ProcessHistorySeries[] {
  const selected = rows
    .slice()
    .sort((left, right) => left.timestamp - right.timestamp || left.ordinal.localeCompare(right.ordinal))
  return PROCESS_HISTORY[lens].map((field) => ({
    ...field,
    points: historyPoints(selected, field.field, field.counter === true),
  }))
}

function historyPoints(
  rows: readonly DataRow[],
  field: string,
  counter: boolean,
): readonly ChartPoint[] {
  let earlier: { readonly value: number; readonly timestamp: number } | null = null
  return buildMetricSamples(rows, (row) => Object.hasOwn(row.values, field)
    ? asNumber(value(row, field))
    : undefined).map((sample) => {
    const drawn = counter ? rate(earlier, sample.value, sample.timestamp) : sample.value
    if (counter) earlier = sample.value === null ? null : { value: sample.value, timestamp: sample.timestamp }
    return {
      segmentId: sample.segmentId,
      timestamp: sample.timestamp,
      value: drawn,
    }
  })
}

function rate(
  earlier: { readonly value: number; readonly timestamp: number } | null,
  number: number | null,
  timestamp: number,
): number | null {
  if (earlier === null || number === null) return null
  const seconds = (timestamp - earlier.timestamp) / 1_000_000
  if (seconds <= 0) return null
  const delta = number - earlier.value
  return delta < 0 ? null : delta / seconds
}
