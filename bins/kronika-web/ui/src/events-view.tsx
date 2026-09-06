import { useEffect, useMemo, useRef, useState } from "react"

import { Diamond, TriangleAlert } from "lucide-react"

import { acceptResponse, loadEventGroups, type Finding, type HourData } from "./api"
import { useDisplayTime } from "./display-time-context"
import { EventTierSection, MinuteStrip, SECTION_ICONS, entryChips, entryTitle, sectionLabel, tiersOf } from "./events-console"
import { categoryLabel } from "./events-format"
import { MINUTE_COLUMNS, type EventEntry } from "./events-groups"
import { findingKey, findingMetric, findingOrder, findingSource, isEventFindingSource, summarizeFindings, type FindingMetric } from "./finding-presentation"
import type { Translate } from "./help"
import { globMatcher } from "./glob"
import { compact, type Locale } from "./model"
import { evaluateExpr, parseSearch } from "./search"
import { TableFilter } from "./table-filter"
import { Timeline } from "./timeline"

const MARK_ROWS = 120
// The digest tile that selects the threshold band instead of a log stream.
const MARKS_TILE = "marks"

const EVENT_SOURCES = [
  "pg_log_errors", "pg_log_checkpoints", "pg_log_autovacuum", "pg_log_slow_queries",
  "pg_log_lock_waits", "pg_log_lifecycle", "pgbouncer_events",
] as const

export function EventsView({
  cursor,
  data,
  environment,
  hour,
  loading = false,
  locale,
  navigationTimestamps,
  onCursor,
  onFinding,
  onOpenChart,
  onPreview,
  onPattern,
  onReady,
  onShowAll,
  onSelectedLane,
  revision,
  scope,
  pattern,
  selected,
  selectedLane,
  t,
}: {
  readonly cursor: number
  readonly data: HourData
  readonly environment: "machine" | "container" | null
  readonly hour: number
  readonly loading?: boolean | undefined
  readonly locale: Locale
  readonly navigationTimestamps: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly onOpenChart: () => void
  readonly onPreview?: ((timestamp: number | null) => void) | undefined
  readonly onPattern: (pattern: string) => void
  readonly onReady: () => void
  readonly onShowAll: () => void
  readonly onSelectedLane: (lane: string) => void
  readonly revision: number
  readonly scope: readonly Finding[] | null
  readonly pattern: string
  readonly selected: Finding | null
  readonly selectedLane: string
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const eventSelection = useMemo(() => eventGroupSelection(data.availableSections, hour, scope), [data.availableSections, hour, scope])
  const events = useEventGroups(eventSelection, revision, onReady)
  const entries = events.rows
  const [expandedKey, setExpandedKey] = useState<string | null>(null)
  useEffect(() => setExpandedKey(null), [hour])
  const selectedEntry = useMemo(() => selected === null || entries === null ? null : entryOf(entries, selected), [entries, selected])
  // A refetch must not reopen an entry that the user collapsed.
  const expandedFor = useRef<string | null>(null)
  useEffect(() => {
    if (selected === null || selectedEntry === null) return
    const key = findingKey(selected)
    if (expandedFor.current === key) return
    expandedFor.current = key
    setExpandedKey(selectedEntry.key)
  }, [selected, selectedEntry])
  const list = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (expandedKey === null || selectedEntry?.key !== expandedKey) return
    for (const node of list.current?.querySelectorAll("[data-entry-key]") ?? []) {
      if (node.getAttribute("data-entry-key") === expandedKey) {
        node.scrollIntoView({ block: "nearest" })
        return
      }
    }
  }, [expandedKey, selectedEntry])
  const parsedSearch = useMemo(() => parseSearch(pattern, "events"), [pattern])
  const [digest, setDigest] = useState<string | null>(null)
  useEffect(() => setDigest(null), [hour])
  const scoped = entries
  const chosen = useMemo(() => {
    if (scoped === null || digest === null) return scoped
    return scoped.filter((entry) => digest === "critical" ? entry.tier === "critical" : entry.section === digest)
  }, [digest, scoped])
  const visible = useMemo(() => chosen === null ? null : chosen.filter((entry) => {
    if (!parsedSearch.ok || parsedSearch.query.canonical === "") return true
    const title = entryTitle(entry, t, locale)
    const category = "category" in entry.stat ? entry.stat.category : null
    const fields: Readonly<Record<string, readonly string[]>> = {
      text: [title, sectionLabel(entry.section, t), ...entryChips(entry, t).map((chip) => chip.label)],
      kind: [entry.tier],
      source: [sectionLabel(entry.section, t), entry.section],
      category: category === null ? [] : [categoryLabel(category, t)],
    }
    const matches = (clause: { readonly key: string; readonly value: string }) =>
      fields[clause.key]?.some((candidate) => globMatcher(clause.value)?.(candidate) ?? true) === true
    if (!parsedSearch.query.structured || parsedSearch.query.expr === null) {
      return matches({ key: "text", value: parsedSearch.query.freeText ?? "" })
    }
    return evaluateExpr(parsedSearch.query.expr, (clause) => matches({ key: clause.key, value: clause.value }))
  }), [chosen, locale, parsedSearch, t])
  const marks = useMemo(() => (scope ?? data.findings)
    .filter((finding) => finding.kind !== "event" && !isEventFindingSource(finding.logicalName))
    .slice()
    .sort((left, right) => findingOrder(right, left)), [data.findings, scope])
  const markGroups = useMemo(() => groupMarks(marks, hour, t), [hour, marks, t])
  const scopeSummary = useMemo(() => scope === null ? null : summarizeFindings(scope), [scope])
  const busy = loading || events.loading
  const consoleStatus = eventConsoleStatus(visible, busy, events.failed)
  const consoleCount = t(consoleStatus.key, consoleStatus.slots)
  return <>
    <Timeline cursor={cursor} environment={environment} findings={data.findings} health={data.health} hour={hour} lanePoints={data.lanePoints} locale={locale} navigationTimestamps={navigationTimestamps} onCursor={onCursor} onFinding={onFinding} onOpenChart={onOpenChart} onPreview={onPreview} onSelectedLane={onSelectedLane} primaryLane="health" selectedLane={selectedLane} t={t} />
    {scopeSummary !== null && <section
      aria-describedby="events-scope-help"
      aria-label={t("events.scope")}
      className="flex min-h-[38px] items-center gap-3 border-b border-line2 bg-s2 px-1.5 py-1 max-[620px]:flex-wrap max-[620px]:gap-x-2 max-[620px]:gap-y-1"
      data-event-marks={scopeSummary.event}
      data-sharp-rise-marks={scopeSummary.spike}
      data-testid="events-scope"
      data-threshold-marks={scopeSummary.knownBad}
    >
      <span className="text-xs font-medium text-fg2">{t("events.scope")}</span>
      <time className="font-mono text-xs tabular-nums text-fg3">{scopeSummary.from === scopeSummary.to ? time.timestamp(scopeSummary.from) : `${time.timestamp(scopeSummary.from)}–${time.timestamp(scopeSummary.to)}`}</time>
      <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1 text-xs tabular-nums text-fg3">
        {scopeSummary.event > 0 && <span>{t("events.scope.event", { count: scopeSummary.event })}</span>}
        {scopeSummary.knownBad > 0 && <span>{t("events.scope.known_bad", { count: scopeSummary.knownBad })}</span>}
        {scopeSummary.spike > 0 && <span>{t("events.scope.spike", { count: scopeSummary.spike })}</span>}
      </span>
      <span className="sr-only" id="events-scope-help">{t("events.scope.help")}</span>
      <button className="min-h-[28px] cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s1 px-2.5 text-xs font-medium text-accent3 transition-colors hover:bg-s3 coarse:min-h-11" onClick={onShowAll} type="button">{t("events.show_all")}</button>
    </section>}
    {markGroups.length > 0 && (digest === null || digest === MARKS_TILE)
      ? <section className="mt-2" data-testid="event-marks">
        <header className="flex min-h-[38px] items-center justify-between border-b border-line2 px-1.5 py-1">
          <span className="flex items-center gap-2">
            <Diamond aria-hidden="true" className="text-bad" size={13} />
            <span className="text-xs font-medium text-fg2">{t("events.marks")}</span>
          </span>
          <span className="text-xs tabular-nums text-fg3">{t("events.marks.count", { groups: markGroups.length, count: marks.length })}</span>
        </header>
        {markGroups.map((group) => <MarkGroupRow
          expanded={expandedKey === group.key}
          group={group}
          hour={hour}
          key={group.key}
          locale={locale}
          onCursor={onCursor}
          onFinding={onFinding}
          onToggle={() => setExpandedKey((current) => current === group.key ? null : group.key)}
          t={t}
        />)}
      </section>
      : null}
    <section className="mt-2" data-testid="events-console">
      <header className="flex min-h-[38px] items-center justify-between border-b border-line2 px-1.5 py-1 max-[760px]:flex-col max-[760px]:items-stretch max-[760px]:gap-[5px]">
        <span className="text-xs font-medium text-fg2">{t("events.console")}</span>
        <span aria-live="polite" className="text-xs tabular-nums text-fg3" role={events.failed ? "alert" : "status"}>{consoleCount}</span>
      </header>
      {scoped !== null && scoped.length > 0 && <EventsDigest active={digest} entries={scoped} locale={locale} marks={markGroups} onChoose={(key) => setDigest((current) => current === key ? null : key)} t={t} />}
      <TableFilter kept={visible?.length ?? 0} onPattern={onPattern} pattern={pattern} surface="events" t={t} total={chosen?.length ?? 0} />
      {events.truncated && <EventsIncompleteNotice locale={locale} t={t} />}
      <div className={`${digest === MARKS_TILE ? "" : "max-[520px]:min-h-0 min-h-[390px] "}${busy && visible !== null ? "animate-pulse opacity-55" : ""}`} data-loading={busy || undefined} ref={list}>
        {digest === MARKS_TILE && <p className="table-empty" role="status">{t("events.marks.only")}</p>}
        {digest !== MARKS_TILE && <>
        {visible === null && events.failed && <p className="table-empty" role="status">{t("events.console.error")}</p>}
        {visible === null && !events.failed && <p className="table-empty flex items-baseline" role="status"><span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none mr-[7px] h-[11px] w-[11px] align-[-1px]" />{t("table.loading")}</p>}
        {visible !== null && visible.length === 0 && <EventsEmptyState
          filtered={pattern !== "" || digest !== null}
          onClear={() => { onPattern(""); onShowAll() }}
          selected={scope !== null}
          t={t}
          truncated={events.truncated}
        />}
        {visible !== null && tiersOf(visible).map(([tier, tierEntries]) => <EventTierSection
          entries={tierEntries}
          expandedKey={expandedKey}
          filtered={digest !== null || pattern !== "" || scope !== null}
          hour={eventSelection.from}
          key={tier}
          locale={locale}
          onCursor={onCursor}
          onToggle={(key) => setExpandedKey((current) => current === key ? null : key)}
          t={t}
          tier={tier}
        />)}
        </>}
      </div>
    </section>
  </>
}

export function EventsIncompleteNotice({ locale, t }: {
  readonly locale: Locale
  readonly t: Translate
}) {
  return <p className="m-0 border-b border-line2 bg-warn/10 px-[9px] py-2 text-xs leading-[1.45] text-warn" data-testid="events-truncated" role="status">
    {t("events.console.truncated", { limit: new Intl.NumberFormat(locale).format(5000) })}
  </p>
}

export function EventsEmptyState({ filtered, onClear, selected = false, t, truncated }: {
  readonly filtered: boolean
  readonly onClear: () => void
  readonly selected?: boolean | undefined
  readonly t: Translate
  readonly truncated: boolean
}) {
  return <div className="table-empty flex items-center gap-2.5">{filtered
    ? <>{t("filter.none")}<button className="cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4" data-testid="events-clear-filter" onClick={onClear} type="button">{t("filter.clear")}</button></>
    : selected
      ? t("events.console.empty_selection")
      : t(truncated ? "events.console.truncated_none" : "events.console.empty")}</div>
}

interface DigestTile {
  readonly key: string
  readonly label: string
  readonly count: number
  readonly minutes: readonly number[]
  readonly bad: boolean
  readonly section: string | null
}

function EventsDigest({ active, entries, locale, marks, onChoose, t }: {
  readonly active: string | null
  readonly entries: readonly EventEntry[]
  readonly locale: Locale
  readonly marks: readonly MarkGroup[]
  readonly onChoose: (key: string) => void
  readonly t: Translate
}) {
  const tiles = useMemo(() => {
    const bySection = new Map<string, { count: number; minutes: number[] }>()
    let criticalCount = 0
    const criticalMinutes = Array.from({ length: MINUTE_COLUMNS }, () => 0)
    for (const entry of entries) {
      const tile = bySection.get(entry.section) ?? { count: 0, minutes: Array.from({ length: MINUTE_COLUMNS }, () => 0) }
      tile.count += entry.count
      entry.minutes.forEach((count, minute) => { tile.minutes[minute] = (tile.minutes[minute] ?? 0) + count })
      bySection.set(entry.section, tile)
      if (entry.tier === "critical") {
        criticalCount += entry.count
        entry.minutes.forEach((count, minute) => { criticalMinutes[minute] = (criticalMinutes[minute] ?? 0) + count })
      }
    }
    const ordered = ["pg_log_errors", "pg_log_slow_queries", "pg_log_lock_waits", "pg_log_checkpoints", "pg_log_autovacuum", "pg_log_lifecycle", "pgbouncer_events"]
    return [
      ...(criticalCount === 0 ? [] : [{ key: "critical", label: t("events.tier.critical"), count: criticalCount, minutes: criticalMinutes, bad: true, section: null } satisfies DigestTile]),
      ...ordered.flatMap((section) => {
        const tile = bySection.get(section)
        return tile === undefined ? [] : [{ key: section, label: sectionLabel(section, t), count: tile.count, minutes: tile.minutes, bad: false, section } satisfies DigestTile]
      }),
    ]
  }, [entries, t])
  const markTile = useMemo(() => {
    if (marks.length === 0) return null
    const minutes = Array.from({ length: MINUTE_COLUMNS }, () => 0)
    let count = 0
    for (const group of marks) {
      count += group.findings.length
      group.minutes.forEach((crossings, minute) => { minutes[minute] = (minutes[minute] ?? 0) + crossings })
    }
    return { key: MARKS_TILE, label: t("events.marks"), count, minutes, bad: true, section: null } satisfies DigestTile
  }, [marks, t])
  const shown = markTile === null ? tiles : [markTile, ...tiles]
  if (shown.length < 2) return null
  return <div aria-label={t("events.digest")} className="flex flex-wrap gap-1.5 border-b border-line2 px-1.5 py-2" data-testid="events-digest" role="group">
    {shown.map((tile) => {
      const Icon = tile.key === MARKS_TILE ? Diamond : tile.section === null ? TriangleAlert : SECTION_ICONS[tile.section] ?? TriangleAlert
      const pressed = active === tile.key
      return <button
        aria-pressed={pressed}
        className={`flex cursor-pointer items-center gap-2 rounded-[var(--radius-sm)] border px-2 py-1.5 text-left transition-colors ${pressed ? "border-accent3 bg-s3" : "border-line2 bg-s1 hover:bg-s2"}`}
        key={tile.key}
        onClick={() => onChoose(tile.key)}
        type="button"
      >
        <Icon aria-hidden="true" className={tile.bad ? "text-bad" : "text-fg3"} size={13} />
        <span className="text-xs text-fg3">{tile.label}</span>
        <strong className="font-mono text-[13px] font-semibold tabular-nums text-fg">{compact(tile.count, locale)}</strong>
        <DigestStrip bad={tile.bad} minutes={tile.minutes} />
      </button>
    })}
  </div>
}

function DigestStrip({ bad, minutes }: { readonly bad: boolean; readonly minutes: readonly number[] }) {
  const peak = Math.max(...minutes, 1)
  return <svg aria-hidden="true" className="block h-[14px] w-[44px] flex-none" preserveAspectRatio="none" viewBox={`0 0 ${minutes.length} 14`}>
    {minutes.map((count, minute) => count === 0 ? null : <rect
      className={bad ? "fill-bad" : "fill-accent3"}
      height={Math.max(1.5, (count / peak) * 13)}
      key={minute}
      width="0.8"
      x={minute + 0.1}
      y={14 - Math.max(1.5, (count / peak) * 13)}
    />)}
  </svg>
}

// A threshold crossing repeats for as long as the value stays across, so one
// hour of a busy host is dozens of identical rows. They group the way log
// events do: one entry per crossed metric, with the hour's shape under it.
interface MarkGroup {
  readonly key: string
  readonly kind: Finding["kind"]
  readonly metric: FindingMetric
  readonly source: string
  readonly findings: readonly Finding[]
  readonly minutes: readonly number[]
}

export function groupMarks(marks: readonly Finding[], hour: number, t: Translate): readonly MarkGroup[] {
  const groups = new Map<string, Finding[]>()
  for (const finding of marks) {
    const key = `${finding.kind}\u{1f}${finding.logicalName}\u{1f}${finding.typeId}\u{1f}${finding.fieldOrdinal}`
    const members = groups.get(key)
    if (members === undefined) groups.set(key, [finding])
    else members.push(finding)
  }
  return [...groups.entries()].flatMap(([key, findings]) => {
    const [first] = findings
    if (first === undefined) return []
    const minutes = Array.from({ length: MINUTE_COLUMNS }, () => 0)
    for (const finding of findings) {
      const minute = Math.floor((finding.timestamp - hour) / 60_000_000)
      if (minute >= 0 && minute < MINUTE_COLUMNS) minutes[minute] = (minutes[minute] ?? 0) + 1
    }
    return [{
      key: `mark:${key}`,
      kind: first.kind,
      metric: findingMetric(first, t),
      source: findingSource(first, t),
      findings: findings.slice().sort((left, right) => findingOrder(right, left)),
      minutes,
    }]
  }).sort((left, right) => right.findings.length - left.findings.length || left.key.localeCompare(right.key))
}

export function MarkGroupRow({ expanded, group, hour, locale, onCursor, onFinding, onToggle, t }: {
  readonly expanded: boolean
  readonly group: MarkGroup
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onFinding: (finding: Finding) => void
  readonly onToggle: () => void
  readonly t: Translate
}) {
  const time = useDisplayTime()
  const [shown, setShown] = useState(MARK_ROWS)
  const first = group.findings[group.findings.length - 1]
  const last = group.findings[0]
  const moments = first === undefined || last === undefined
    ? ""
    : first.timestamp === last.timestamp
      ? time.timestamp(first.timestamp)
      : `${time.timestamp(first.timestamp)}–${time.timestamp(last.timestamp)}`
  const Icon = group.kind === "spike" ? TriangleAlert : Diamond
  return <div className="border-b border-line" data-testid="event-mark">
    <button
      aria-expanded={expanded}
      className="grid w-full cursor-pointer grid-cols-[26px_minmax(0,1fr)_auto_120px_auto] items-center gap-2.5 border-0 bg-transparent px-[9px] py-[7px] text-left text-fg2 transition-colors hover:bg-s3 aria-expanded:bg-s2 max-[760px]:grid-cols-[26px_minmax(0,1fr)_auto]"
      onClick={onToggle}
      type="button"
    >
      <span aria-hidden="true" className={`flex h-[24px] w-[24px] items-center justify-center rounded-[var(--radius-sm)] ${group.kind === "spike" ? "bg-warn/15 text-warn" : "bg-bad/15 text-bad"}`}>
        <Icon size={13} />
      </span>
      <span className="min-w-0">
        <strong className="block truncate text-xs font-medium text-fg" data-testid="event-mark-label">{group.metric.label}</strong>
        <small className="mt-[3px] block truncate text-xs text-fg3">
          {group.source}
          {group.metric.boundary !== null && <><span aria-hidden="true"> · </span><span data-testid="event-mark-boundary">{group.metric.boundary}</span></>}
        </small>
      </span>
      <span className="whitespace-nowrap text-right font-mono text-[13px] font-semibold tabular-nums text-fg">×{compact(group.findings.length, locale)}</span>
      <span className="max-[760px]:hidden"><MinuteStrip
        fill={group.kind === "spike" ? "fill-warn" : "fill-bad"}
        hour={hour}
        minutes={group.minutes}
        onCursor={onCursor}
        t={t}
      /></span>
      <time className="whitespace-nowrap text-right font-mono text-xs tabular-nums text-fg3 max-[760px]:hidden">{moments}</time>
    </button>
    {expanded && <div className="border-t border-line2 bg-s2 px-[9px] py-[7px]">
      <p className="mb-[7px] mt-0 text-xs leading-[1.45] text-fg3" data-testid="event-mark-help">{t(group.metric.helpKey)}</p>
      <div className="grid gap-px overflow-hidden rounded-[var(--radius-sm)] border border-line2 bg-s1">
        {group.findings.slice(0, shown).map((finding) => <button
          className="grid w-full cursor-pointer grid-cols-[auto_minmax(0,1fr)] items-baseline gap-2.5 border-0 bg-transparent px-2 py-[3px] text-left transition-colors hover:bg-s3"
          data-testid="event-mark-crossing"
          key={findingKey(finding)}
          onClick={() => onFinding(finding)}
          type="button"
        >
          <time className="whitespace-nowrap font-mono text-xs tabular-nums text-fg3">{time.timestamp(finding.timestamp)}</time>
          <span className="truncate text-xs text-fg2">{findingSource(finding, t)}</span>
        </button>)}
      </div>
      {group.findings.length > shown && <button className="mt-1.5 cursor-pointer rounded-[var(--radius-xs)] border-0 bg-s3 px-2 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s4" onClick={() => setShown((current) => current + MARK_ROWS)} type="button">{t("events.marks.more", { count: group.findings.length - shown })}</button>}
    </div>}
  </div>
}

export function entryOf(entries: readonly EventEntry[], finding: Finding): EventEntry | null {
  const matches = entries.filter((entry) => entry.section === finding.logicalName
    && entry.representativeTs === finding.timestamp)
  return matches.length === 1 ? matches[0] ?? null : null
}

interface EventGroupSelection {
  readonly from: number
  readonly sources: readonly string[]
  readonly to: number
}

interface EventConsoleStatus {
  readonly key: string
  readonly slots?: Readonly<Record<string, string | number>> | undefined
}

export function eventConsoleStatus(rows: readonly EventEntry[] | null, loading: boolean, failed: boolean): EventConsoleStatus {
  if (rows === null) return { key: failed ? "events.console.unavailable" : "events.console.loading" }
  if (loading) return { key: "events.console.loading" }
  const slots = { groups: rows.length, count: rows.reduce((sum, entry) => sum + entry.count, 0) }
  return { key: failed ? "events.console.update_failed" : "events.console.count", slots }
}

export function eventGroupSelection(availableSections: readonly string[], hour: number, scope: readonly Finding[] | null): EventGroupSelection {
  const available = new Set(availableSections)
  if (scope === null) {
    return {
      from: hour,
      sources: EVENT_SOURCES.filter((source) => available.has(source)),
      to: hour + 3_600_000_000,
    }
  }
  const selected = scope.filter((finding) => finding.kind === "event" && available.has(finding.logicalName))
  if (selected.length === 0) return { from: hour, sources: [], to: hour + 1 }
  const timestamps = selected.map((finding) => finding.timestamp)
  return {
    from: Math.min(...timestamps),
    sources: EVENT_SOURCES.filter((source) => selected.some((finding) => finding.logicalName === source)),
    to: Math.max(...timestamps) + 1,
  }
}

interface StreamState {
  readonly key: string
  readonly rows: readonly EventEntry[] | null
  readonly truncated: boolean
  readonly loading: boolean
  readonly failed: boolean
}

// Live-hour revisions are frequent; full log sections refresh at most once per minute.
const STREAM_REFRESH_MIN_MS = 60_000

function useEventGroups(selection: EventGroupSelection, revision: number, onReady: () => void): StreamState {
  const wanted = selection.sources.join(",")
  const key = `${selection.from}:${selection.to}:${wanted}`
  const [state, setState] = useState<StreamState>({ key: "", rows: null, truncated: false, loading: false, failed: false })
  const lastRead = useRef({ key: "", at: 0 })
  useEffect(() => {
    if (wanted === "") {
      setState({ key, rows: [], truncated: false, loading: false, failed: false })
      onReady()
      return
    }
    const now = Date.now()
    if (lastRead.current.key === key && now - lastRead.current.at < STREAM_REFRESH_MIN_MS) return
    lastRead.current = { key, at: now }
    setState((current) => current.key === key
      ? { ...current, loading: true, failed: false }
      : { key, rows: null, truncated: false, loading: true, failed: false })
    const controller = new AbortController()
    const load = loadEventGroups(selection.from, selection.to, wanted.split(","), controller.signal)
    acceptResponse(
      load,
      controller.signal,
      (result) => {
        setState({
          key,
          rows: result.rows,
          truncated: result.truncated,
          loading: false,
          failed: false,
        })
        onReady()
      },
      () => {
        lastRead.current = { key: "", at: 0 }
        setState((current) => ({ ...current, key, loading: false, failed: true }))
      },
    )
    load.catch((error: unknown) => {
      if (!controller.signal.aborted) console.error("events load failed", error)
    })
    return () => controller.abort()
  }, [key, onReady, revision, selection.from, selection.to, wanted])
  return state
}
