import { ChevronRight, CircleAlert, HardDrive, Lock, Network, Power, Recycle, Timer, type LucideIcon } from "lucide-react"
import { useEffect, useRef, useState } from "react"

import { loadEventRowDetail, type Cell, type EventRowDetail } from "./api"
import { useDisplayTime } from "./display-time-context"
import { EVENT_TIERS, type EventEntry, type EventTier } from "./events-groups"
import { categoryLabel } from "./events-format"
import type { Translate } from "./help"
import { compact, humanDuration, type Locale } from "./model"

const MINUTE_US = 60_000_000
const TIER_ROWS = 100

const TIER_DOT: Readonly<Record<EventTier, string>> = {
  critical: "bg-bad",
  notable: "bg-warn",
  routine: "bg-fg3",
}

const SECTION_LABELS: Readonly<Record<string, string>> = {
  pg_log_errors: "events.type.errors",
  pg_log_checkpoints: "events.type.checkpoints",
  pg_log_autovacuum: "events.type.autovacuum",
  pg_log_slow_queries: "events.type.slow_queries",
  pg_log_lock_waits: "events.type.lock_waits",
  pg_log_lifecycle: "events.type.lifecycle",
  pgbouncer_events: "events.type.pgbouncer",
}

export const SECTION_ICONS: Readonly<Record<string, LucideIcon>> = {
  pg_log_errors: CircleAlert,
  pg_log_checkpoints: HardDrive,
  pg_log_autovacuum: Recycle,
  pg_log_slow_queries: Timer,
  pg_log_lock_waits: Lock,
  pg_log_lifecycle: Power,
  pgbouncer_events: Network,
}

export interface EntryChip {
  readonly label: string
  readonly tone: "bad" | "warn" | "neutral"
}

const CHIP_TONES: Readonly<Record<EntryChip["tone"], string>> = {
  bad: "bg-bad/10 text-bad",
  warn: "bg-warn/10 text-warn",
  neutral: "bg-s3 text-fg3",
}

const GROUPED_NOTES: Readonly<Record<string, string>> = {
  "pg.errors": "events.grouped.errors",
  "pg.slow": "events.grouped.slow",
  "pg.autovacuum": "events.grouped.autovacuum",
  "pg.checkpoints": "events.grouped.checkpoints",
  "pg.checkpoint_warning": "events.grouped.checkpoints",
  "pg.locks": "events.grouped.locks",
  "pg.lifecycle": "events.grouped.lifecycle",
  "pgbouncer.events": "events.grouped.pgbouncer",
}

export function sectionLabel(section: string, t: Translate): string {
  const key = SECTION_LABELS[section]
  return key === undefined ? section : t(key)
}

export function entryTitle(entry: EventEntry, t: Translate, locale: Locale): string {
  const stat = entry.stat
  if (stat.kind === "pg.checkpoints") {
    return t("events.entry.checkpoints", { count: entry.count, timed: stat.timed, requested: stat.requested })
  }
  if (stat.kind === "pg.checkpoint_warning") {
    return t("events.entry.checkpoint_warning", { count: entry.count, seconds: stat.secondsApart ?? "?" })
  }
  if (stat.kind === "pg.locks") {
    const duration = stat.maxMs === null ? "?" : humanDuration(stat.maxMs, locale)
    if (stat.acquired) return t("events.entry.locks_acquired", { waiters: stat.waiters, duration })
    return stat.holders === null
      ? t("events.entry.locks_unknown", { waiters: stat.waiters, duration })
      : t("events.entry.locks", { holders: stat.holders, waiters: stat.waiters, duration })
  }
  if (stat.kind === "pg.lifecycle") {
    if (stat.lifecycle === 0) return t("events.entry.lifecycle.crash", { pid: stat.pid ?? "?", signal: stat.signal ?? "?" })
    if (stat.lifecycle === 1) return t("events.entry.lifecycle.shutdown", { mode: stat.mode ?? "?" })
    return t("events.entry.lifecycle.ready")
  }
  return entry.label ?? sectionLabel(entry.section, t)
}

export function entrySubtitle(entry: EventEntry, t: Translate, locale: Locale): string | null {
  const stat = entry.stat
  if (stat.kind === "pg.slow") {
    return t("events.entry.slow.sub", {
      count: entry.count,
      max: humanDuration(stat.maxMs, locale),
      total: humanDuration(stat.totalMs, locale),
    })
  }
  if (stat.kind === "pg.autovacuum") {
    return t("events.entry.autovacuum.sub", {
      runs: stat.runs,
      time: stat.totalMs === null ? "—" : humanDuration(stat.totalMs, locale),
    })
  }
  if (stat.kind === "pg.checkpoints") {
    return t("events.entry.checkpoints.sub", {
      buffers: stat.buffers === null ? "—" : compact(stat.buffers, locale),
      sync: stat.maxSyncMs === null ? "—" : humanDuration(stat.maxSyncMs, locale),
    })
  }
  if (stat.kind === "pg.locks" && stat.targets.length > 0) return stat.targets.join(" · ")
  if (stat.kind === "pgbouncer.events") {
    return [
      pgbouncerDatabase(stat.database, t),
      pgbouncerUsername(stat.username, t),
      stat.host === null ? null : t("events.pgbouncer.context.client", { value: stat.host }),
    ].filter((value): value is string => value !== null).join(" · ") || null
  }
  return null
}

function pgbouncerDatabase(value: string | null, t: Translate): string | null {
  if (value === null) return null
  return t(value === "(nodb)"
    ? "events.pgbouncer.context.database_unset"
    : "events.pgbouncer.context.database", { value })
}

function pgbouncerUsername(value: string | null, t: Translate): string | null {
  if (value === null) return null
  return t(value === "(nouser)"
    ? "events.pgbouncer.context.username_unset"
    : "events.pgbouncer.context.username", { value })
}

export function entryChips(entry: EventEntry, t: Translate, locale: Locale = "en"): readonly EntryChip[] {
  const stat = entry.stat
  const neutral = (label: string): EntryChip => ({ label, tone: "neutral" })
  if (stat.kind === "pg.errors") {
    const severity = ["error", "fatal", "panic", "warning", "log"][stat.severity] ?? "log"
    return [
      { label: t(`events.severity.${severity}`), tone: severity === "warning" ? "warn" : severity === "log" ? "neutral" : "bad" },
      ...(stat.sqlstate === null ? [] : [neutral(stat.sqlstate)]),
      ...(stat.category === null ? [] : [neutral(categoryLabel(stat.category, t))]),
      ...(stat.database === null ? [] : [neutral(stat.database)]),
    ]
  }
  if (stat.kind === "pg.slow") {
    // log_min_duration_statement of zero logs every statement, and "≥ 0 ms"
    // reads like a defect rather than that setting.
    if (stat.thresholdMs === null) return []
    return [neutral(stat.thresholdMs === 0
      ? t("events.slow.all")
      : t("events.slow.threshold", { threshold: humanDuration(stat.thresholdMs, locale) }))]
  }
  if (stat.kind === "pg.autovacuum") return [neutral(t(stat.analyze ? "events.autovacuum.analyze" : "events.autovacuum.vacuum"))]
  if (stat.kind === "pgbouncer.events") {
    const level = ["fatal", "error", "warning", "log", "debug", "noise"][stat.level] ?? "log"
    return [{ label: t(`events.pgbouncer.${level}`), tone: level === "fatal" || level === "error" ? "bad" : level === "warning" ? "warn" : "neutral" }]
  }
  return []
}

export function MinuteStrip({ fill, hour, minutes, onCursor, t, testId }: {
  readonly fill: string
  readonly hour: number
  readonly minutes: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
  readonly testId?: string
}) {
  const peak = Math.max(...minutes, 1)
  const columns = minutes.length
  return <button
    aria-label={t("events.strip")}
    className="block h-[24px] w-[120px] flex-none cursor-pointer rounded-[var(--radius-xs)] border-0 bg-transparent p-0 transition-colors hover:bg-s2"
    data-testid={testId}
    onClick={(event) => {
      event.stopPropagation()
      const bounds = event.currentTarget.getBoundingClientRect()
      const minute = Math.min(columns - 1, Math.max(0, Math.floor(((event.clientX - bounds.left) / bounds.width) * columns)))
      onCursor(hour + minute * MINUTE_US)
    }}
    tabIndex={-1}
    type="button"
  >
    <svg aria-hidden="true" className="block h-full w-full" preserveAspectRatio="none" viewBox={`0 0 ${columns * 2} 24`}>
      <line className="stroke-line2" strokeWidth="1" x1="0" x2={columns * 2} y1="23.5" y2="23.5" />
      {minutes.map((count, minute) => count === 0 ? null : <rect
        className={fill}
        height={Math.max(2, Math.round((count / peak) * 21))}
        key={minute}
        rx="0.5"
        width="1.6"
        x={minute * 2 + 0.2}
        y={23 - Math.max(2, Math.round((count / peak) * 21))}
      />)}
    </svg>
  </button>
}

export function EventEntryRow({ entry, expanded, hour, locale, onCursor, onToggle, t }: {
  readonly entry: EventEntry
  readonly expanded: boolean
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onToggle: () => void
  readonly t: Translate
}) {
  const request = useRef<AbortController | null>(null)
  const [detail, setDetail] = useState<EventRowDetail | null>(null)
  const [detailStatus, setDetailStatus] = useState<"idle" | "loading" | "failed">("idle")
  useEffect(() => {
    request.current?.abort()
    request.current = null
    setDetail(null)
    setDetailStatus("idle")
    return () => request.current?.abort()
  }, [entry.detailRef])
  const time = useDisplayTime()
  const title = entryTitle(entry, t, locale)
  const subtitle = entrySubtitle(entry, t, locale)
  const chips = entryChips(entry, t, locale)
  const critical = entry.tier === "critical"
  const Icon = SECTION_ICONS[entry.section] ?? CircleAlert
  const recorded = entry.label !== null && title === entry.label
  const moments = entry.firstTs === entry.lastTs
    ? time.timestamp(entry.firstTs)
    : `${time.timestamp(entry.firstTs)}–${time.timestamp(entry.lastTs)}`
  return <div
    className={`border-b border-line ${critical ? "border-l-2 border-l-bad bg-bad/[0.04]" : ""} ${expanded ? "border-l-2 border-l-accent3" : ""}`}
    data-entry-key={entry.key}
    data-testid="event-entry"
    data-tier={entry.tier}
  >
    <button
      aria-expanded={expanded}
      className="grid w-full cursor-pointer grid-cols-[26px_minmax(0,1fr)_auto_120px_auto] items-center gap-2.5 border-0 bg-transparent px-[9px] py-[7px] text-left text-fg2 transition-colors hover:bg-s3 aria-expanded:bg-s2 max-[760px]:grid-cols-[26px_minmax(0,1fr)_auto]"
      onClick={onToggle}
      type="button"
    >
      <span aria-hidden="true" className={`flex h-[24px] w-[24px] items-center justify-center rounded-[var(--radius-sm)] ${critical ? "bg-bad/15 text-bad" : "bg-s3 text-fg3"}`}>
        <Icon size={13} />
      </span>
      <span className="min-w-0">
        <strong className={`block truncate text-[12px] font-medium text-fg ${recorded ? "font-mono" : ""}`} data-testid="event-entry-title" title={title}>{title}</strong>
        <span className="mt-[3px] flex items-baseline gap-1.5">
          <small className="flex-none text-[12px] text-fg3">{sectionLabel(entry.section, t)}</small>
          {chips.map((chip) => <small className={`flex-none rounded-[var(--radius-xs)] px-1 text-[12px] ${CHIP_TONES[chip.tone]}`} key={chip.label}>{chip.label}</small>)}
          {subtitle !== null && <small className="truncate text-[12px] text-fg3" title={subtitle}>{subtitle}</small>}
        </span>
      </span>
      <span className="whitespace-nowrap text-right font-mono text-[13px] font-semibold tabular-nums text-fg">×{compact(entry.count, locale)}</span>
      <span className="max-[760px]:hidden"><MinuteStrip
        fill={entry.tier === "critical" ? "fill-bad" : "fill-accent3"}
        hour={hour}
        minutes={entry.minutes}
        onCursor={onCursor}
        t={t}
        testId="event-strip"
      /></span>
      <time className="whitespace-nowrap text-right font-mono text-[12px] tabular-nums text-fg3 max-[760px]:hidden">{moments}</time>
    </button>
    {expanded && <EventEntryDetail
      detail={detail}
      detailStatus={detailStatus}
      entry={entry}
      locale={locale}
      onCursor={onCursor}
      onDetail={() => {
        if (detailStatus !== "idle" || detail !== null || request.current !== null) return
        const controller = new AbortController()
        request.current = controller
        setDetailStatus("loading")
        void loadEventRowDetail(entry.detailRef, controller.signal).then((loaded) => {
          if (controller.signal.aborted) return
          request.current = null
          if (loaded.section !== entry.section || loaded.at !== entry.representativeTs) {
            setDetailStatus("failed")
            return
          }
          setDetail(loaded)
          setDetailStatus("idle")
        }).catch((error: unknown) => {
          if (controller.signal.aborted) return
          request.current = null
          console.error("event detail load failed", error)
          setDetailStatus("failed")
        })
      }}
      t={t}
    />}
  </div>
}

function EventEntryDetail({ detail, detailStatus, entry, locale, onCursor, onDetail, t }: {
  readonly detail: EventRowDetail | null
  readonly detailStatus: "idle" | "loading" | "failed"
  readonly entry: EventEntry
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onDetail: () => void
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const note = GROUPED_NOTES[entry.stat.kind]
  const facts = entry.stat.kind === "pgbouncer.events"
    ? [
        [t("events.field.occurrences"), compact(entry.count, locale)] as const,
        [t("events.field.first_occurrence"), time.timestamp(entry.firstTs)] as const,
        ...(entry.firstTs === entry.lastTs ? [] : [
          [t("events.field.last_occurrence"), time.timestamp(entry.lastTs)] as const,
          [t("events.field.activity_span"), humanDuration((entry.lastTs - entry.firstTs) / 1_000, locale)] as const,
        ]),
        ...entryFacts(entry, locale, t),
      ]
    : entryFacts(entry, locale, t)
  const actionLabel = detail !== null
    ? t("events.detail.recorded_at")
    : t(detailStatus === "loading" ? "events.detail.loading" : "events.detail.open")
  return <div className="grid gap-2 border-t border-line2 bg-s2 px-[9px] py-[9px]" data-testid="event-entry-detail">
    {facts.length > 0 && <div className="flex flex-wrap gap-1.5" data-testid="event-entry-facts">
      {facts.map(([label, shownValue]) => <span className="flex items-baseline gap-1.5 rounded-[var(--radius-sm)] border border-line2 bg-s1 px-2 py-1" key={label}>
        <span className="text-[12px] text-fg3">{label}</span>
        <strong className="font-mono text-[12px] font-medium tabular-nums text-fg">{shownValue}</strong>
      </span>)}
    </div>}
    <button
      aria-busy={detailStatus === "loading" || undefined}
      aria-disabled={detail === null && detailStatus !== "idle" || undefined}
      className="w-fit cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s1 px-2.5 py-1.5 text-[12px] font-medium text-accent3 transition-colors hover:bg-s3 aria-disabled:cursor-wait aria-disabled:opacity-70 coarse:min-h-11"
      data-detail-status={detail === null ? detailStatus : "loaded"}
      data-testid="event-representative-detail"
      onClick={detail === null ? onDetail : () => onCursor(detail.at)}
      type="button"
    ><span aria-live="polite" data-testid={detailStatus === "loading" ? "event-detail-loading" : undefined}>{actionLabel}</span> · <time className="font-mono tabular-nums">{time.timestamp(detail?.at ?? entry.representativeTs)}</time></button>
    {detailStatus === "failed" && <p className="m-0 text-[12px] text-bad" data-testid="event-detail-error" role="alert">{t("events.detail.error")}</p>}
    {detail !== null && <RepresentativeDetail detail={detail} t={t} />}
    {note !== undefined && <p className="text-right text-[12px] text-fg3">{t(note)}</p>}
  </div>
}

export interface EventDetailText {
  readonly field: string
  readonly fullLen: string
  readonly storedText: string
  readonly truncated: boolean
}

export function eventDetailTexts(fields: Readonly<Record<string, Cell>>): readonly EventDetailText[] {
  return Object.entries(fields).flatMap(([field, cell]) => {
    if (cell === null || typeof cell !== "object" || Array.isArray(cell)) return []
    const value = cell as Readonly<Record<string, unknown>>
    if (typeof value.stored_text !== "string" || typeof value.full_len !== "string" || typeof value.truncated !== "boolean") return []
    return [{ field, fullLen: value.full_len, storedText: value.stored_text, truncated: value.truncated }]
  })
}

function RepresentativeDetail({ detail, t }: {
  readonly detail: EventRowDetail
  readonly t: Translate
}) {
  const texts = eventDetailTexts(detail.fields)
  return <section className="grid gap-2 rounded-[var(--radius-sm)] border border-line2 bg-s1 p-2" data-testid="event-representative-occurrence">
    {texts.length === 0 && <p className="m-0 text-[12px] text-fg3">{t("events.detail.no_text")}</p>}
    {texts.map((text) => <div className="grid gap-1" key={text.field}>
      <small className="text-[12px] text-fg3">{t(`events.field.${text.field}`)}</small>
      <pre aria-label={t(`events.field.${text.field}`)} className="m-0 max-h-[180px] overflow-auto whitespace-pre-wrap rounded-[var(--radius-sm)] border border-line2 bg-s2 px-2 py-1.5 font-mono text-[12px] leading-[1.5] text-fg2 [overflow-wrap:anywhere]" tabIndex={0}>{text.storedText}</pre>
      {text.truncated && <small className="text-[12px] text-warn">{t("events.detail.text_truncated", { length: text.fullLen })}</small>}
    </div>)}
  </section>
}

function entryFacts(entry: EventEntry, locale: Locale, t: Translate): readonly (readonly [string, string])[] {
  const stat = entry.stat
  const field = (name: string) => t(`events.field.${name}`)
  if (stat.kind === "pg.errors") {
    return [
      ...(stat.database === null ? [] : [[field("database"), stat.database] as const]),
      ...(stat.username === null ? [] : [[field("username"), stat.username] as const]),
    ]
  }
  if (stat.kind === "pg.slow") {
    return [
      [field("count"), compact(entry.count, locale)],
      [field("max_duration_ms"), humanDuration(stat.maxMs, locale)],
      [field("total_duration_ms"), humanDuration(stat.totalMs, locale)],
    ]
  }
  if (stat.kind === "pg.autovacuum") {
    return [
      ...(stat.totalMs === null ? [] : [[field("elapsed_ms"), humanDuration(stat.totalMs, locale)] as const]),
      ...(stat.tuplesRemoved === null ? [] : [[field("tuples_removed"), compact(stat.tuplesRemoved, locale)] as const]),
      ...(stat.tuplesDead === null ? [] : [[field("tuples_dead_not_removable"), compact(stat.tuplesDead, locale)] as const]),
    ]
  }
  if (stat.kind === "pg.checkpoints") {
    return [
      ...(stat.buffers === null ? [] : [[field("buffers_written"), compact(stat.buffers, locale)] as const]),
      ...(stat.maxSyncMs === null ? [] : [[field("sync_ms"), humanDuration(stat.maxSyncMs, locale)] as const]),
    ]
  }
  if (stat.kind === "pg.checkpoint_warning") {
    return stat.secondsApart === null ? [] : [[field("seconds_apart"), `${compact(stat.secondsApart, locale)} s`]]
  }
  if (stat.kind === "pg.locks") {
    return [
      ...(stat.holders === null ? [] : [[field("holding_pids"), stat.holders] as const]),
      ...(stat.targets.length === 0 ? [] : [[field("lock_target"), stat.targets.join(" · ")] as const]),
    ]
  }
  if (stat.kind === "pg.lifecycle") {
    return [
      ...(stat.pid === null ? [] : [[field("pid"), String(stat.pid)] as const]),
      ...(stat.signal === null ? [] : [[field("signal"), String(stat.signal)] as const]),
      ...(stat.mode === null ? [] : [[field("shutdown_mode"), stat.mode] as const]),
    ]
  }
  return [
    ...(stat.database === null ? [] : [[field("database"), stat.database] as const]),
    ...(stat.username === null ? [] : [[field("username"), stat.username] as const]),
    ...(stat.host === null ? [] : [[field("client"), stat.host] as const]),
    ...(stat.sourceFile === null ? [] : [[field("source_file"), stat.sourceFile] as const]),
  ]
}

export function EventTierSection({ entries, expandedKey, filtered, hour, locale, onCursor, onToggle, t, tier }: {
  readonly entries: readonly EventEntry[]
  readonly expandedKey: string | null
  // A digest tile or a search narrows the console to what the reader asked
  // for; a tier that stays folded then answers with nothing.
  readonly filtered: boolean
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onToggle: (key: string) => void
  readonly t: Translate
  readonly tier: EventTier
}) {
  const [open, setOpen] = useState(tier !== "routine")
  const [shown, setShown] = useState(TIER_ROWS)
  const folded = useRef(false)
  useEffect(() => {
    if (!filtered || folded.current) return
    setOpen(true)
  }, [filtered])
  // Do not reopen a manually collapsed section after a refetch.
  const opened = useRef<string | null>(null)
  useEffect(() => setShown(TIER_ROWS), [hour])
  useEffect(() => {
    if (expandedKey === null || opened.current === expandedKey) return
    const index = entries.findIndex((entry) => entry.key === expandedKey)
    if (index < 0) return
    opened.current = expandedKey
    setOpen(true)
    setShown((current) => index < current ? current : index + 1)
  }, [entries, expandedKey])
  if (entries.length === 0) return null
  const total = entries.reduce((sum, entry) => sum + entry.count, 0)
  return <section data-testid={`event-tier-${tier}`}>
    <button
      aria-expanded={open}
      className="flex w-full cursor-pointer items-center gap-2 border-b border-line2 bg-s2 px-[9px] py-[6px] text-left transition-colors hover:bg-s3"
      onClick={() => setOpen((current) => { folded.current = current; return !current })}
      type="button"
    >
      <ChevronRight aria-hidden="true" className={`flex-none text-fg3 transition-transform duration-100 motion-reduce:transition-none ${open ? "rotate-90" : ""}`} size={13} />
      <span aria-hidden="true" className={`h-[8px] w-[8px] rounded-full ${TIER_DOT[tier]}`} />
      <span className="text-xs font-medium text-fg2">{t(`events.tier.${tier}`)}</span>
      <span className="text-xs tabular-nums text-fg3">{t("events.console.count", { groups: entries.length, count: total })}</span>
    </button>
    {open && entries.slice(0, shown).map((entry) => <EventEntryRow
      entry={entry}
      expanded={expandedKey === entry.key}
      hour={hour}
      key={entry.key}
      locale={locale}
      onCursor={onCursor}
      onToggle={() => onToggle(entry.key)}
      t={t}
    />)}
    {open && entries.length > shown && <button
      className="m-1.5 cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4"
      onClick={() => setShown((current) => current + TIER_ROWS)}
      type="button"
    >{t("events.more_groups", { count: entries.length - shown })}</button>}
  </section>
}

export function tiersOf(entries: readonly EventEntry[]): readonly (readonly [EventTier, readonly EventEntry[]])[] {
  return EVENT_TIERS.map((tier) => [tier, entries.filter((entry) => entry.tier === tier)] as const)
}
