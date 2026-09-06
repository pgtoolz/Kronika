import { Activity, ChevronDown, ChevronRight, Gauge, HardDrive, Hourglass, Layers, ShieldAlert } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"

import { loadSeries, type DataRow, type HourData, type LanePoint } from "./api"
import { useDisplayTime } from "./display-time-context"
import { LabelHelp, type Translate } from "./help"
import { useHistoryRequest } from "./history-request"
import { asNumber, compact, humanBytes, humanDuration, humanPercent, value, type Locale } from "./model"
import {
  anchoredSeries,
  counterGroups,
  countSeries,
  gaugeSeries,
  lastValue,
  peakPoint,
  settingAt,
  settingChanges,
  shareOfTotals,
  sumCounterVital,
  type VitalSeries,
} from "./postgres-vitals"
import { SeriesChart, readingAt, type ChartPoint } from "./series-chart"
import { SparkCell } from "./spark-cell"
import { sparkScaleMax } from "./spark"

const HOUR_US = 3_600_000_000
const IO_IDENTITY = ["backend_type", "object", "context"]
const SINGLETON_IDENTITY: readonly string[] = []

interface VitalStreams {
  readonly database: readonly DataRow[]
  readonly activity: readonly DataRow[]
  readonly wal: readonly DataRow[]
  readonly checkpointer: readonly DataRow[]
  readonly bgwriter: readonly DataRow[]
  readonly archiver: readonly DataRow[]
  readonly io: readonly DataRow[]
  readonly walStorage: readonly DataRow[]
  readonly prepared: readonly DataRow[]
  readonly vacuum: readonly DataRow[]
  readonly settings: readonly DataRow[]
  readonly lifecycle: readonly DataRow[]
}

const STREAM_FIELDS: Readonly<Record<keyof VitalStreams, readonly [string, readonly string[]]>> = {
  database: ["pg_stat_database", ["datid", "xact_commit", "xact_rollback", "tup_returned", "tup_fetched", "tup_inserted", "tup_updated", "tup_deleted", "blks_hit", "blks_read", "temp_bytes", "deadlocks", "checksum_failures", "sessions_fatal", "sessions_killed", "blk_read_time", "blk_write_time", "frozen_xid_age", "min_mxid_age"]],
  activity: ["pg_stat_activity", ["pid", "state", "backend_type", "backend_xmin_age"]],
  wal: ["pg_stat_wal", ["wal_records", "wal_fpi", "wal_bytes", "wal_buffers_full"]],
  checkpointer: ["pg_stat_checkpointer", ["num_timed", "num_requested", "buffers_written"]],
  bgwriter: ["pg_stat_bgwriter", ["checkpoints_timed", "checkpoints_req", "buffers_checkpoint", "buffers_clean", "buffers_backend"]],
  archiver: ["pg_stat_archiver", ["archived_count", "failed_count"]],
  io: ["pg_stat_io", ["backend_type", "object", "context", "reads", "writes", "extends", "evictions", "reuses", "fsyncs"]],
  walStorage: ["pg_wal_storage", ["wal_files_bytes"]],
  prepared: ["pg_prepared_xacts", ["datname", "prepared_count", "max_age_us"]],
  vacuum: ["pg_stat_progress_vacuum", ["pid", "is_autovacuum"]],
  settings: ["pg_settings", ["name", "setting", "unit"]],
  lifecycle: ["pg_log_lifecycle", ["kind", "message"]],
}

interface VitalRow {
  readonly key: string
  // Empty points hide the row because the fact was not recorded.
  readonly points: readonly ChartPoint[]
  readonly second?: readonly ChartPoint[] | undefined
  readonly kind: "share" | "rate" | "count" | "bytes"
  readonly headline: string
  readonly reading: (cursor: number) => string
  readonly limit?: number | undefined
  readonly unit?: string | undefined
  // Optional duration-aware chart formatting.
  readonly chart?: ((value: number | null, locale: Locale) => string) | undefined
}

interface VitalBand {
  readonly key: string
  readonly icon: typeof Activity
  readonly rows: readonly VitalRow[]
}

export function PostgresOverview({ cursor, data, historyRevision, hour, locale, onCursor, t }: {
  readonly cursor: number
  readonly data: HourData
  readonly historyRevision: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const present = useMemo(() => Object.entries(STREAM_FIELDS)
    .filter(([, [section]]) => data.availableSections.includes(section))
    .map(([key]) => key)
    .join(","), [data.availableSections])
  // Live-hour revisions are frequent; whole-hour values refresh at most once per minute.
  const [coarseRevision, setCoarseRevision] = useState(0)
  const lastRead = useRef(0)
  useEffect(() => {
    const now = Date.now()
    if (now - lastRead.current < 60_000) return
    lastRead.current = now
    setCoarseRevision((current) => current + 1)
  }, [historyRevision])
  const streams = useHistoryRequest<VitalStreams>(
    present === "" ? null : JSON.stringify([hour, "pg-vitals", present]),
    coarseRevision,
    present === "" ? null : async (signal) => {
      const wanted = present.split(",") as (keyof VitalStreams)[]
      const loaded = await Promise.all(wanted.map(async (key) => {
        const [section, fields] = STREAM_FIELDS[key]
        return [key, await loadSeries(hour, section, {}, fields, signal)] as const
      }))
      const empty: VitalStreams = { database: [], activity: [], wal: [], checkpointer: [], bgwriter: [], archiver: [], io: [], walStorage: [], prepared: [], vacuum: [], settings: [], lifecycle: [] }
      return { ...empty, ...Object.fromEntries(loaded) }
    },
  )
  if (present === "") return null
  return <section className="mt-2" data-testid="postgres-vitals">
    <VitalsBody cursor={cursor} data={data} hour={hour} locale={locale} onCursor={onCursor} status={streams.status} streams={streams.value} t={t} />
  </section>
}

function VitalsBody({ cursor, data, hour, locale, onCursor, status, streams, t }: {
  readonly cursor: number
  readonly data: HourData
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly status: "error" | "loading" | "ready"
  readonly streams: VitalStreams | null
  readonly t: Translate
}) {
  const bands = useMemo(() => streams === null ? [] : buildBands(streams, data.lanePoints, locale, t), [data.lanePoints, locale, streams, t])
  if (streams === null) {
    return status === "error"
      ? <p className="table-empty" role="status">{t("pg.vitals.error")}</p>
      : <p className="table-empty flex items-baseline" role="status"><span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none mr-[7px] h-[11px] w-[11px] align-[-1px]" />{t("table.loading")}</p>
  }
  const shown = bands.map((band) => ({ ...band, rows: band.rows.filter((row) => row.points.length > 0) })).filter((band) => band.rows.length > 0)
  return <div className={status === "loading" ? "animate-pulse opacity-55" : ""}>
    <Passport cursor={cursor} locale={locale} onCursor={onCursor} streams={streams} t={t} />
    {shown.length === 0 && <p className="table-empty" role="status">{t("pg.vitals.empty")}</p>}
    {shown.map((band) => <Band band={band} cursor={cursor} hour={hour} key={band.key} locale={locale} onCursor={onCursor} t={t} />)}
  </div>
}

function Band({ band, cursor, hour, locale, onCursor, t }: {
  readonly band: VitalBand
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const end = hour + HOUR_US
  const Icon = band.icon
  return <section data-testid={`vitals-band-${band.key}`}>
    <header className="flex items-center gap-2 border-b border-line2 bg-s2 px-[9px] py-[6px]">
      <Icon aria-hidden="true" className="text-fg3" size={13} />
      <span className="text-xs font-medium text-fg2">{t(`pg.vitals.band.${band.key}`)}</span>
    </header>
    {band.rows.map((row) => {
      const open = expanded.has(row.key)
      const max = Math.max(
        sparkScaleMax(row.kind, [row.points, ...(row.second === undefined ? [] : [row.second])]),
        row.limit ?? 0,
      )
      return <div className="border-b border-line" data-testid={`vital-${row.key}`} key={row.key}>
        <button
          aria-expanded={open}
          className="grid w-full cursor-pointer grid-cols-[minmax(150px,210px)_minmax(110px,auto)_minmax(0,1fr)_minmax(84px,auto)] items-center gap-2.5 border-0 bg-transparent px-[9px] py-[6px] text-left transition-colors hover:bg-s3 aria-expanded:bg-s2 max-[760px]:grid-cols-[minmax(0,1fr)_auto]"
          onClick={() => setExpanded((current) => {
            const next = new Set(current)
            if (next.has(row.key)) next.delete(row.key)
            else next.add(row.key)
            return next
          })}
          type="button"
        >
          <span className="flex min-w-0 items-center gap-1 text-xs text-fg2"><LabelHelp helpKey={`pg.vitals.row.${row.key}.help`} labelKey={`pg.vitals.row.${row.key}`} t={t} /></span>
          <strong className="whitespace-nowrap font-mono text-[13px] font-semibold tabular-nums text-fg">{row.headline}</strong>
          <span className="min-w-0 max-[760px]:hidden"><SparkCell cursor={cursor} end={end} hour={hour} limit={row.limit} max={max} points={row.points} second={row.second} /></span>
          <span className="whitespace-nowrap text-right font-mono text-xs tabular-nums text-fg3 max-[760px]:hidden">{row.reading(cursor)}</span>
        </button>
        {open && <div className="border-t border-line2 bg-s2 px-[9px] py-[7px]">
          <SeriesChart
            cursor={cursor}
            format={(number, place) => row.chart === undefined ? vitalNumber(row.kind, number, place) : row.chart(number, place)}
            helpKey={`pg.vitals.row.${row.key}.help`}
            hour={hour}
            labelKey={`pg.vitals.row.${row.key}`}
            locale={locale}
            onCursor={onCursor}
            points={row.points}
            scale="nonnegative"
            second={row.second}
            {...(row.second === undefined ? {} : {
              secondHelpKey: `pg.vitals.row.${row.key}.second.help`,
              secondLabelKey: `pg.vitals.row.${row.key}.second`,
            })}
            t={t}
            tickFormat={(number, place) => row.chart === undefined ? vitalNumber(row.kind, number, place) : row.chart(number, place)}
            unit={row.unit ?? ""}
          />
        </div>}
      </div>
    })}
  </section>
}

function Passport({ cursor, locale, onCursor, streams, t }: {
  readonly cursor: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly streams: VitalStreams
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const [changesOpen, setChangesOpen] = useState(false)
  const changes = useMemo(() => settingChanges(streams.settings), [streams.settings])
  const restarts = useMemo(() => streams.lifecycle.slice().sort((left, right) => left.timestamp - right.timestamp), [streams.lifecycle])
  const chip = (name: string, shown: string | null) => shown === null ? null : <span className="flex items-baseline gap-1.5 rounded-[var(--radius-sm)] border border-line2 bg-s1 px-2 py-1" key={name}>
    <span className="font-mono text-xs text-fg3">{name}</span>
    <strong className="font-mono text-xs font-medium tabular-nums text-fg">{shown}</strong>
  </span>
  const at = (name: string) => settingAt(streams.settings, name, cursor)
  const chips = [
    chip("server_version", at("server_version")),
    chip("max_connections", at("max_connections")),
    chip("shared_buffers", settingBytes(streams.settings, "shared_buffers", cursor, locale)),
    chip("max_wal_size", settingBytes(streams.settings, "max_wal_size", cursor, locale)),
    chip("checkpoint_timeout", at("checkpoint_timeout") === null ? null : humanDuration(Number(at("checkpoint_timeout")) * 1000, locale)),
    chip("autovacuum", at("autovacuum")),
    chip("track_io_timing", at("track_io_timing")),
  ].filter((shown) => shown !== null)
  if (chips.length === 0 && changes.length === 0 && restarts.length === 0) return null
  return <div className="border-b border-line2 px-1.5 py-2" data-testid="vitals-passport">
    <div className="flex flex-wrap items-center gap-1.5">
      {chips}
      {restarts.map((row) => <button className="flex cursor-pointer items-center gap-1.5 rounded-[var(--radius-sm)] border border-bad/40 bg-bad/10 px-2 py-1 text-xs text-bad" key={`${row.segmentId}:${row.ordinal}`} onClick={() => onCursor(row.timestamp)} type="button">
        {t(`events.lifecycle.${["crash", "shutdown", "ready"][Number(row.values.kind ?? 2)] ?? "ready"}`)}
        <strong className="font-mono tabular-nums">{time.timestamp(row.timestamp)}</strong>
      </button>)}
      {changes.length > 0 && <button aria-expanded={changesOpen} className="flex cursor-pointer items-center gap-1.5 rounded-[var(--radius-sm)] border border-warn/40 bg-warn/10 px-2 py-1 text-xs text-warn" data-testid="vitals-setting-changes" onClick={() => setChangesOpen((current) => !current)} type="button">
        {t("pg.vitals.settings_changed", { count: changes.length })}
        {changesOpen ? <ChevronDown aria-hidden="true" size={12} /> : <ChevronRight aria-hidden="true" size={12} />}
      </button>}
    </div>
    {changesOpen && changes.length > 0 && <div className="mt-1.5 grid gap-px overflow-hidden rounded-[var(--radius-sm)] border border-line2 bg-s1">
      {changes.map((change) => <button className="grid cursor-pointer grid-cols-[auto_auto_minmax(0,1fr)] items-baseline gap-2.5 px-2 py-1 text-left transition-colors hover:bg-s3" key={`${change.timestamp}:${change.name}`} onClick={() => onCursor(change.timestamp)} type="button">
        <time className="whitespace-nowrap font-mono text-xs tabular-nums text-fg3">{time.timestamp(change.timestamp)}</time>
        <span className="font-mono text-xs text-fg">{change.name}</span>
        <span className="truncate font-mono text-xs text-fg2">{change.from ?? "—"} → {change.to ?? "—"}</span>
      </button>)}
    </div>}
  </div>
}

function buildBands(streams: VitalStreams, lanePoints: readonly LanePoint[], locale: Locale, t: Translate): readonly VitalBand[] {
  const lane = (name: string): readonly ChartPoint[] => lanePoints
    .filter((point) => point.lane === name)
    .map((point) => ({ segmentId: point.segmentId, timestamp: point.timestamp, value: point.value }))
  const perSecond = t("unit.per_second")
  const counted = (points: readonly ChartPoint[]): string => {
    const peak = peakPoint(points)
    return peak === null || peak.value === null ? "—" : compact(peak.value, locale)
  }
  const readCount = (points: readonly ChartPoint[]) => (cursor: number) => {
    const stored = readingAt(points, cursor)
    return stored === null ? "—" : compact(stored, locale)
  }
  const readBytes = (points: readonly ChartPoint[]) => (cursor: number) => {
    const stored = readingAt(points, cursor)
    return stored === null ? "—" : humanBytes(stored, locale)
  }

  const clients = countSeries(streams.activity, (row) => backendType(row) === "client backend")
  const idleInTx = countSeries(streams.activity, (row) => String(row.values.state ?? "").startsWith("idle in transaction"))
  const oldestXmin = gaugeSeries(streams.activity, "backend_xmin_age", "max")
  const preparedCount = anchoredSeries(streams.prepared, streams.database, (pass) =>
    pass.reduce((sum, row) => sum + (asNumber(value(row, "prepared_count")) ?? 0), 0))
  const preparedAge = anchoredSeries(streams.prepared, streams.database, (pass) => {
    let oldest: number | null = null
    for (const row of pass) {
      const age = asNumber(value(row, "max_age_us"))
      if (age !== null && (oldest === null || age > oldest)) oldest = age
    }
    return oldest
  })

  const database = counterGroups(streams.database)
  const io = counterGroups(streams.io, IO_IDENTITY)
  const vacuumIo = counterGroups(streams.io.filter((row) => row.values.context === "vacuum"), IO_IDENTITY)
  const wal = counterGroups(streams.wal, SINGLETON_IDENTITY)
  const checkpointer = counterGroups(streams.checkpointer, SINGLETON_IDENTITY)
  const bgwriter = counterGroups(streams.bgwriter, SINGLETON_IDENTITY)
  const archiver = counterGroups(streams.archiver, SINGLETON_IDENTITY)

  const tps = sumCounterVital(database, ["xact_commit", "xact_rollback"])
  const rollback = sumCounterVital(database, ["xact_rollback"])
  const rollbackShare = shareOfTotals(rollback, tps)
  const tupRead = sumCounterVital(database, ["tup_returned"])
  const tupFetched = sumCounterVital(database, ["tup_fetched"])
  const tupWritten = sumCounterVital(database, ["tup_inserted", "tup_updated", "tup_deleted"])
  const hits = sumCounterVital(database, ["blks_hit"])
  const reads = sumCounterVital(database, ["blks_read"])
  const cacheHit = ratioPoints(hits.points, reads.points)
  const ioTime = sumCounterVital(database, ["blk_read_time", "blk_write_time"])

  const temp = sumCounterVital(database, ["temp_bytes"])
  const deadlocks = sumCounterVital(database, ["deadlocks"])
  const checksums = sumCounterVital(database, ["checksum_failures"])
  const sessionEnds = sumCounterVital(database, ["sessions_fatal", "sessions_killed"])

  const walBytes = sumCounterVital(wal, ["wal_bytes"])
  const walBuffersFull = sumCounterVital(wal, ["wal_buffers_full"])
  const checkpointsTimed = firstVital(
    sumCounterVital(checkpointer, ["num_timed"]),
    sumCounterVital(bgwriter, ["checkpoints_timed"]),
  )
  const checkpointsRequested = firstVital(
    sumCounterVital(checkpointer, ["num_requested"]),
    sumCounterVital(bgwriter, ["checkpoints_req"]),
  )
  const checkpointBuffers = firstVital(
    sumCounterVital(bgwriter, ["buffers_checkpoint"]),
    sumCounterVital(checkpointer, ["buffers_written"]),
  )
  const backendBuffers = sumCounterVital(bgwriter, ["buffers_backend"])
  const archived = sumCounterVital(archiver, ["archived_count"])
  const archiveFailed = sumCounterVital(archiver, ["failed_count"])
  const evictions = sumCounterVital(io, ["evictions"])
  const reuses = sumCounterVital(io, ["reuses"])
  const extendOps = sumCounterVital(io, ["extends"])
  const fsyncs = sumCounterVital(io, ["fsyncs"])
  const vacuumReads = sumCounterVital(vacuumIo, ["reads"])
  const walSize = gaugeSeries(streams.walStorage, "wal_files_bytes", "max")

  const xidAge = gaugeSeries(streams.database, "frozen_xid_age", "max")
  const mxidAge = gaugeSeries(streams.database, "min_mxid_age", "max")
  const vacuumWorkers = anchoredSeries(
    streams.vacuum.filter((row) => row.values.is_autovacuum === true),
    streams.database,
    (pass) => pass.length,
  )

  const maxConnections = numberSetting(streams.settings, "max_connections")
  const freezeMax = numberSetting(streams.settings, "autovacuum_freeze_max_age")
  const vacuumWorkersMax = numberSetting(streams.settings, "autovacuum_max_workers")

  return [
    {
      key: "admission",
      icon: Activity,
      rows: [
        {
          key: "backends",
          points: clients,
          kind: "count",
          headline: maxConnections === null ? counted(clients) : `${counted(clients)} / ${compact(maxConnections, locale)}`,
          reading: readCount(clients),
          limit: maxConnections ?? undefined,
        },
        {
          key: "states",
          points: lane("pg_running"),
          second: lane("pg_waiting"),
          kind: "count",
          headline: counted(lane("pg_running")),
          reading: readCount(lane("pg_running")),
        },
        { key: "idle_in_tx", points: idleInTx, kind: "count", headline: counted(idleInTx), reading: readCount(idleInTx) },
        {
          key: "oldest_xact",
          points: lane("pg_oldest_xact"),
          kind: "count",
          headline: durationHeadline(lane("pg_oldest_xact"), locale),
          reading: (cursor) => {
            const stored = readingAt(lane("pg_oldest_xact"), cursor)
            return stored === null ? "—" : humanDuration(stored * 1000, locale)
          },
          chart: (stored, place) => stored === null ? "—" : humanDuration(stored * 1000, place),
        },
        { key: "oldest_xmin", points: oldestXmin, kind: "count", headline: counted(oldestXmin), reading: readCount(oldestXmin) },
        {
          key: "prepared",
          points: preparedCount,
          kind: "count",
          headline: preparedAge.length === 0 ? counted(preparedCount) : `${counted(preparedCount)} · ${durationHeadline(preparedAge, locale, 0.001)}`,
          reading: readCount(preparedCount),
        },
      ],
    },
    {
      key: "transactions",
      icon: Gauge,
      rows: [
        {
          key: "tps",
          points: tps.points,
          second: rollback.points,
          kind: "rate",
          headline: totalHeadline(tps, (total) => rollbackShare === null ? compact(total, locale) : `${compact(total, locale)} · ${humanPercent(rollbackShare * 100, locale)}`),
          reading: readCount(tps.points),
          unit: perSecond,
        },
        {
          key: "tuples_read",
          points: tupRead.points,
          second: tupFetched.points,
          kind: "rate",
          headline: totalHeadline(tupRead, (total) => compact(total, locale)),
          reading: readCount(tupRead.points),
          unit: perSecond,
        },
        {
          key: "tuples_written",
          points: tupWritten.points,
          kind: "rate",
          headline: totalHeadline(tupWritten, (total) => compact(total, locale)),
          reading: readCount(tupWritten.points),
          unit: perSecond,
        },
        {
          key: "cache_hit",
          points: cacheHit,
          kind: "share",
          headline: hits.total === null || reads.total === null || hits.total + reads.total <= 0
            ? "—"
            : humanPercent(100 * hits.total / (hits.total + reads.total), locale),
          reading: (cursor) => {
            const stored = readingAt(cacheHit, cursor)
            return stored === null ? "—" : humanPercent(stored, locale)
          },
        },
        {
          key: "io_time",
          points: ioTime.points,
          kind: "rate",
          headline: totalHeadline(ioTime, (total) => humanDuration(total, locale)),
          reading: (cursor) => {
            const stored = readingAt(ioTime.points, cursor)
            return stored === null ? "—" : humanDuration(stored, locale)
          },
          chart: (stored, place) => stored === null ? "—" : humanDuration(stored, place),
        },
      ],
    },
    {
      key: "pressure",
      icon: ShieldAlert,
      rows: [
        {
          key: "temp",
          points: temp.points,
          kind: "bytes",
          headline: totalHeadline(temp, (total) => humanBytes(total, locale)),
          reading: readBytes(temp.points),
        },
        { key: "deadlocks", points: deadlocks.points, kind: "rate", headline: totalHeadline(deadlocks, (total) => compact(total, locale)), reading: readCount(deadlocks.points) },
        { key: "checksum_failures", points: checksums.points, kind: "rate", headline: totalHeadline(checksums, (total) => compact(total, locale)), reading: readCount(checksums.points) },
        { key: "session_ends", points: sessionEnds.points, kind: "rate", headline: totalHeadline(sessionEnds, (total) => compact(total, locale)), reading: readCount(sessionEnds.points) },
      ],
    },
    {
      key: "write",
      icon: HardDrive,
      rows: [
        {
          key: "wal",
          points: walBytes.points,
          kind: "bytes",
          headline: totalHeadline(walBytes, (total) => humanBytes(total, locale)),
          reading: readBytes(walBytes.points),
        },
        {
          key: "checkpoints",
          points: checkpointsTimed.points,
          second: checkpointsRequested.points,
          kind: "rate",
          headline: checkpointHeadline(checkpointsTimed, checkpointsRequested, locale, t),
          reading: readCount(checkpointsTimed.points),
        },
        {
          key: "buffers",
          points: checkpointBuffers.points,
          second: backendBuffers.points,
          kind: "rate",
          headline: totalHeadline(checkpointBuffers, (total) => compact(total, locale)),
          reading: readCount(checkpointBuffers.points),
        },
        {
          key: "archiver",
          points: archived.points,
          second: archiveFailed.points,
          kind: "rate",
          headline: archiverHeadline(archived, archiveFailed, locale, t),
          reading: readCount(archived.points),
        },
        { key: "wal_buffers_full", points: walBuffersFull.points, kind: "rate", headline: totalHeadline(walBuffersFull, (total) => compact(total, locale)), reading: readCount(walBuffersFull.points) },
        {
          key: "wal_size",
          points: walSize,
          kind: "bytes",
          headline: walSize.length === 0 ? "—" : humanBytes(lastValue(walSize) ?? 0, locale),
          reading: readBytes(walSize),
        },
      ],
    },
    {
      key: "buffers",
      icon: Layers,
      rows: [
        { key: "evictions", points: evictions.points, kind: "rate", headline: totalHeadline(evictions, (total) => compact(total, locale)), reading: readCount(evictions.points) },
        { key: "reuses", points: reuses.points, kind: "rate", headline: totalHeadline(reuses, (total) => compact(total, locale)), reading: readCount(reuses.points) },
        { key: "extends", points: extendOps.points, kind: "rate", headline: totalHeadline(extendOps, (total) => compact(total, locale)), reading: readCount(extendOps.points) },
        { key: "fsyncs", points: fsyncs.points, kind: "rate", headline: totalHeadline(fsyncs, (total) => compact(total, locale)), reading: readCount(fsyncs.points) },
        { key: "vacuum_reads", points: vacuumReads.points, kind: "rate", headline: totalHeadline(vacuumReads, (total) => compact(total, locale)), reading: readCount(vacuumReads.points) },
      ],
    },
    {
      key: "horizons",
      icon: Hourglass,
      rows: [
        {
          key: "xid_age",
          points: xidAge,
          kind: "count",
          headline: xidHeadline(xidAge, freezeMax, locale),
          reading: readCount(xidAge),
          limit: freezeMax ?? undefined,
        },
        { key: "mxid_age", points: mxidAge, kind: "count", headline: mxidAge.length === 0 ? "—" : compact(lastValue(mxidAge) ?? 0, locale), reading: readCount(mxidAge) },
        {
          key: "vacuum_workers",
          points: vacuumWorkers,
          kind: "count",
          headline: vacuumWorkersMax === null ? counted(vacuumWorkers) : `${counted(vacuumWorkers)} / ${compact(vacuumWorkersMax, locale)}`,
          reading: readCount(vacuumWorkers),
          limit: vacuumWorkersMax ?? undefined,
        },
      ],
    },
  ]
}

function backendType(row: DataRow): string {
  const stored = row.values.backend_type
  return typeof stored === "string" ? stored : "client backend"
}

function firstVital(preferred: VitalSeries, fallback: VitalSeries): VitalSeries {
  return preferred.points.length > 0 ? preferred : fallback
}

function totalHeadline(vital: VitalSeries, format: (total: number) => string): string {
  return vital.total === null ? "—" : format(vital.total)
}

function checkpointHeadline(timed: VitalSeries, requested: VitalSeries, locale: Locale, t: Translate): string {
  if (timed.total === null && requested.total === null) return "—"
  const total = (timed.total ?? 0) + (requested.total ?? 0)
  return t("pg.vitals.checkpoints_headline", {
    count: compact(total, locale),
    requested: compact(requested.total ?? 0, locale),
  })
}

function archiverHeadline(archived: VitalSeries, failed: VitalSeries, locale: Locale, t: Translate): string {
  if (archived.total === null && failed.total === null) return "—"
  return t("pg.vitals.archiver_headline", {
    count: compact(archived.total ?? 0, locale),
    failed: compact(failed.total ?? 0, locale),
  })
}

function xidHeadline(points: readonly ChartPoint[], freezeMax: number | null, locale: Locale): string {
  const stored = lastValue(points)
  if (stored === null) return "—"
  if (freezeMax === null || freezeMax <= 0) return compact(stored, locale)
  return `${compact(stored, locale)} · ${humanPercent(100 * stored / freezeMax, locale)}`
}

function durationHeadline(points: readonly ChartPoint[], locale: Locale, scale = 1000): string {
  const peak = peakPoint(points)
  return peak === null || peak.value === null ? "—" : humanDuration(peak.value * scale, locale)
}

function numberSetting(rows: readonly DataRow[], name: string): number | null {
  const stored = settingAt(rows, name, Number.MAX_SAFE_INTEGER)
  if (stored === null) return null
  const parsed = Number(stored)
  return Number.isFinite(parsed) ? parsed : null
}

// Settings such as `shared_buffers` use a recorded unit such as `8kB`.
function settingBytes(rows: readonly DataRow[], name: string, cursor: number, locale: Locale): string | null {
  const stored = settingAt(rows, name, cursor)
  if (stored === null) return null
  const parsed = Number(stored)
  if (!Number.isFinite(parsed)) return stored
  const unit = unitBytesOf(rows, name)
  return unit === null ? stored : humanBytes(parsed * unit, locale)
}

function unitBytesOf(rows: readonly DataRow[], name: string): number | null {
  const row = rows.find((candidate) => candidate.values.name === name && typeof candidate.values.unit === "string")
  const unit = row === undefined ? null : String(row.values.unit)
  if (unit === null || unit === "") return null
  const match = /^(\d*)(B|kB|MB|GB)$/.exec(unit)
  if (match === null) return null
  const scale = { B: 1, kB: 1024, MB: 1024 * 1024, GB: 1024 * 1024 * 1024 }[match[2] as "B" | "kB" | "MB" | "GB"]
  return (match[1] === "" ? 1 : Number(match[1])) * scale
}

function ratioPoints(hits: readonly ChartPoint[], reads: readonly ChartPoint[]): readonly ChartPoint[] {
  const readAt = new Map(reads.map((point) => [point.timestamp, point.value]))
  return hits.flatMap((point) => {
    const read = readAt.get(point.timestamp)
    if (point.value === null || read === null || read === undefined) return []
    const total = point.value + read
    return total <= 0 ? [] : [{ segmentId: point.segmentId, timestamp: point.timestamp, value: 100 * point.value / total }]
  })
}

function vitalNumber(kind: VitalRow["kind"], value: number | null, locale: Locale): string {
  if (value === null) return "—"
  if (kind === "bytes") return humanBytes(value, locale)
  if (kind === "share") return humanPercent(value, locale)
  return compact(value, locale)
}
