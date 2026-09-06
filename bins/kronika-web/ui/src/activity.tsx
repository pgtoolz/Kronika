import { ChevronDown, ChevronRight, Maximize2, Minimize2 } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { createPortal } from "react-dom"

import { CGROUP_CPU_CUTS, CGROUP_IO_CUTS, DATABASE_CUTS, INDEX_CUTS, PLAN_CUTS, PROCESS_CUTS, STATEMENT_CUTS, TABLE_CUTS, activityPreview, cutScale, type ActivityCut, type ActivityScales } from "./activity-cuts"
import { type StatementScope, loadHeatmap, type DataRow } from "./api"
import { cgroupDeviceKey, cgroupDevicePrimary, cgroupDeviceSecondary, type CgroupDevicePresentation } from "./cgroup-device"
import { HOUR_MICROS, collapseHeatmapView, heatmapIntensity, heatmapViewMax, type HeatmapView, type HeatmapViewRow } from "./heatmap"
import { LabelHelp, type Translate } from "./help"
import { humanBytes, humanDuration, measure, rawText, value, type Locale } from "./model"
import { canonicalSearch } from "./search"
import type { RelatedNavigation } from "./statement-navigation"

const TOP_BLOCK = 8
const TOP_CHOICES = [10, 25, 50, 100] as const
const DEFAULT_TOP = 25

type ScaleMode = "global" | "row"

interface LedgerKeys {
  readonly title: string
  readonly bands: string
}

interface HeatmapState {
  readonly loading: boolean
  readonly error: boolean
  readonly view: HeatmapView | null
  // Keep the displayed units aligned with the loaded data during a reload.
  readonly viewCut: ActivityCut | null
}

function useHeatmapView(
  section: string,
  columns: number,
  group: readonly string[] | undefined,
  hour: number,
  cut: ActivityCut,
  top: number,
  revision: number,
  enabled: boolean,
  scope: StatementScope,
): HeatmapState {
  const [state, setState] = useState<HeatmapState>({ loading: true, error: false, view: null, viewCut: null })
  useEffect(() => {
    if (!enabled) return
    const controller = new AbortController()
    setState((current) => ({ loading: true, error: false, view: current.view, viewCut: current.viewCut }))
    loadHeatmap(hour, section, cut.fields, columns, top, controller.signal, group, scope)
      .then((view) => { if (!controller.signal.aborted) setState({ loading: false, error: false, view, viewCut: cut }) })
      .catch(() => {
        if (!controller.signal.aborted) {
          setState((current) => ({ loading: false, error: true, view: current.view, viewCut: current.viewCut }))
        }
      })
    return () => controller.abort()
  }, [columns, cut, enabled, group, hour, revision, scope, section, top])
  return state
}

function loadActivityOpen(storageKey: string): boolean {
  try {
    return localStorage.getItem(storageKey) === "1"
  } catch {
    return false
  }
}

function storeActivityOpen(storageKey: string, open: boolean): void {
  try {
    localStorage.setItem(storageKey, open ? "1" : "0")
  } catch {}
}

interface RowLabel {
  readonly detail?: string | null
  readonly semantic?: boolean
  readonly text: string
  readonly title?: string
  readonly prefix: string | null
}

function ActivityLedger({ columns, cursor, cuts, defaultCut, drill, group, headingContext, hour, keys, label, locale, onCursor, scales, scope = "all", section, storageKey, t }: {
  readonly columns: number
  readonly cursor: number
  readonly cuts: readonly ActivityCut[]
  readonly defaultCut: string
  readonly drill?: ((row: HeatmapViewRow) => void) | undefined
  readonly group?: readonly string[] | undefined
  readonly headingContext?: ((view: HeatmapView) => string | null) | undefined
  readonly hour: number
  readonly keys: LedgerKeys
  readonly label: (row: HeatmapViewRow) => RowLabel
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly scales: ActivityScales
  readonly scope?: StatementScope | undefined
  readonly section: string
  readonly storageKey: string
  readonly t: Translate
}) {
  const [open, setOpen] = useState(() => loadActivityOpen(storageKey))
  const [cutId, setCutId] = useState(defaultCut)
  const [scale, setScale] = useState<ScaleMode>("global")
  const [top, setTop] = useState<number>(DEFAULT_TOP)
  const [maximized, setMaximized] = useState(false)
  const [chosen, setChosen] = useState<string | null>(null)
  const [revision, setRevision] = useState(0)
  const cut = cuts.find((candidate) => candidate.id === cutId) ?? cuts[0] as ActivityCut
  const state = useHeatmapView(section, columns, group, hour, cut, top, revision, open, scope)
  const view = useMemo(() => {
    if (state.view === null) return null
    return maximized ? state.view : collapseHeatmapView(state.view, TOP_BLOCK)
  }, [maximized, state.view])
  useEffect(() => setChosen(null), [hour, section])

  // A drill filters the table below the ledger, which a full-screen ledger
  // covers; it steps back so the filtered rows are the next thing seen.
  // A row that recorded nothing at the cursor would filter to an empty
  // table, which reads as a wrong filter rather than a wrong moment; the
  // cursor then moves to the row's own peak — the same instant clicking
  // that cell sets. A row alive at the cursor leaves the cursor alone.
  const choose = drill === undefined ? undefined : (row: HeatmapViewRow) => {
    setChosen(rowKey(row))
    setMaximized(false)
    const cursorColumn = cursorColumnOf(cursor, hour, columns)
    if (cursorColumn === null || (row.cells[cursorColumn] ?? null) === null) {
      const peak = rowPeakColumn(row.cells)
      if (peak !== null) onCursor(intervalInstant(hour, peak, columns))
    }
    drill(row)
  }

  useEffect(() => {
    if (!maximized) return
    const keydown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      setMaximized(false)
    }
    window.addEventListener("keydown", keydown, true)
    return () => window.removeEventListener("keydown", keydown, true)
  }, [maximized])

  const toggle = (next: boolean) => {
    setOpen(next)
    storeActivityOpen(storageKey, next)
    if (!next) setMaximized(false)
  }

  if (!open) {
    return <section aria-label={t(`${keys.title}.title`)} className="activity-block" data-testid={`activity-${section}`}>
      <header className="flex items-center gap-1 px-2 py-[6px]">
        <button aria-expanded={false} className="flex cursor-pointer items-center gap-1 border-0 bg-transparent p-0 font-sans text-sm font-medium text-fg2" data-testid="activity-toggle" onClick={() => toggle(true)} type="button">
          <ChevronRight aria-hidden="true" className="flex-none text-fg4" size={13} />
          {t(`${keys.title}.title`)}
        </button>
        <LabelHelp helpKey={`${keys.title}.title.help`} iconOnly labelKey={`${keys.title}.title`} t={t} />
      </header>
    </section>
  }
  if (state.error && state.view === null) {
    return <section aria-label={t(`${keys.title}.title`)} className="activity-block" data-testid={`activity-${section}`}>
      <div className="flex items-center gap-3 px-3 py-2 font-sans text-sm text-fg3">
        <span>{t("activity.error")}</span>
        <button className="cursor-pointer border-0 bg-transparent p-0 font-sans text-sm text-accent3 underline decoration-dotted underline-offset-2" onClick={() => setRevision((current) => current + 1)} type="button">{t("activity.retry")}</button>
      </div>
    </section>
  }
  if (view === null) {
    return <section aria-label={t(`${keys.title}.title`)} className="activity-block" data-testid={`activity-${section}`}>
      <header className="flex items-center gap-2 border-b border-line px-3 py-[6px]">
        <h2 className="m-0 font-sans text-sm font-medium text-fg2">{t(`${keys.title}.title`)}</h2>
        <span className="font-sans text-xs text-fg4">{t("activity.loading")}</span>
      </header>
      <div aria-hidden="true" className="px-3 py-2">
        {Array.from({ length: 4 }, (_, index) => <div className="mb-[6px] h-[18px] animate-pulse rounded-[3px] bg-s3 last:mb-0" key={index} />)}
      </div>
    </section>
  }

  const panel = <ActivityPanel
    columns={columns}
    cursor={cursor}
    cut={cut}
    cuts={cuts}
    loadedCut={state.viewCut ?? cut}
    loading={state.loading}
    chosen={chosen}
    drill={choose}
    headingContext={headingContext?.(view) ?? null}
    hour={hour}
    keys={keys}
    label={label}
    locale={locale}
    maximized={maximized}
    onCollapse={() => toggle(false)}
    onCursor={onCursor}
    onCut={setCutId}
    onMaximized={setMaximized}
    onScale={setScale}
    onTop={setTop}
    scale={scale}
    scales={scales}
    section={section}
    t={t}
    top={top}
    view={view}
  />
  if (!maximized) return panel
  return <>
    <section aria-label={t(`${keys.title}.title`)} className="activity-block" data-testid={`activity-${section}-placeholder`} />
    {createPortal(<div aria-label={t(`${keys.title}.title`)} className="activity-overlay" data-testid="activity-overlay" role="dialog">{panel}</div>, document.body)}
  </>
}

// One ledger row across renders: its layout and recorded identity.
function rowKey(row: HeatmapViewRow): string {
  return `${row.typeId}:${row.identity.join(":")}`
}

// The last moment of a column: what clicking that cell on the strip sets.
export function intervalInstant(hour: number, column: number, columns: number): number {
  return hour + Math.floor(((column + 1) * HOUR_MICROS) / columns) - 1
}

// The column the cursor stands in, or null when it is outside the hour.
export function cursorColumnOf(cursor: number, hour: number, columns: number): number | null {
  return cursor >= hour && cursor < hour + HOUR_MICROS
    ? Math.min(columns - 1, Math.floor(((cursor - hour) * columns) / HOUR_MICROS))
    : null
}

// The row's busiest recorded interval: the first strictly positive maximum.
// An all-null or all-zero row has no peak and offers nowhere to go.
export function rowPeakColumn(cells: readonly (number | null)[]): number | null {
  let peak: number | null = null
  for (let index = 0; index < cells.length; index += 1) {
    const cell = cells[index]
    if (cell === null || cell === undefined || cell <= 0) continue
    if (peak === null || cell > (cells[peak] ?? 0)) peak = index
  }
  return peak
}

const STATEMENT_KEYS: LedgerKeys = { title: "activity", bands: "activity" }
const PLAN_KEYS: LedgerKeys = { title: "activity.plans", bands: "activity.plans" }
const TABLE_KEYS: LedgerKeys = { title: "activity.tables", bands: "activity.tables" }
const INDEX_KEYS: LedgerKeys = { title: "activity.indexes", bands: "activity.indexes" }

export function StatementsActivity({ blockSize, cursor, hour, locale, onCursor, onRelated, rows, scope, t }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onRelated: (target: RelatedNavigation) => void
  readonly rows: readonly DataRow[]
  readonly scope: StatementScope
  readonly t: Translate
}) {
  const texts = useMemo(() => statementTextsByQueryId(rows), [rows])
  const drill = (row: HeatmapViewRow) => {
    const queryId = rowQueryId(row)
    if (queryId === null) return
    const database = labelText(row, "datname")
    const role = labelText(row, "usename")
    const clauses = [
      ...(database == null ? [] : [{ key: "database", value: database }]),
      ...(role == null ? [] : [{ key: "role", value: role }]),
      { key: "query_id", value: queryId },
    ]
    const expression = canonicalSearch(clauses, "pg_stat_statements")
    if (expression !== null) onRelated({ expression, queryId, section: "statements" })
  }

  const label = (row: HeatmapViewRow): RowLabel => {
    const queryId = rowQueryId(row)
    const text = queryId === null ? undefined : texts.get(queryId)
    return {
      text: text === undefined ? t("pg.detail.query", { id: queryId ?? "—" }) : activityPreview(text),
      // Query identity includes role, database, and top-level status.
      prefix: identityPrefix(row, row.identity[3] === "false" ? t("activity.nested") : null),
    }
  }

  return <ActivityLedger columns={60} cursor={cursor} cuts={STATEMENT_CUTS} defaultCut="exec_time" drill={drill} hour={hour} keys={STATEMENT_KEYS} label={label} locale={locale} onCursor={onCursor} scales={{ blockSize, clockTicks: null }} scope={scope} section="pg_stat_statements" storageKey="kronika.activity-open" t={t} />
}

export function PlansActivity({ blockSize, cursor, hour, locale, onCursor, onRelated, rows, t }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onRelated: (target: RelatedNavigation) => void
  readonly rows: readonly DataRow[]
  readonly t: Translate
}) {
  const texts = useMemo(() => planTextsByPlanId(rows), [rows])
  const drill = (row: HeatmapViewRow) => {
    const planId = row.identity[3]
    if (planId == null || planId === "0") return
    const expression = canonicalSearch([{ key: "plan_id", value: planId }], "pg_store_plans")
    if (expression !== null) onRelated({ expression, planId, section: "plans" })
  }
  const label = (row: HeatmapViewRow): RowLabel => {
    const planId = row.identity[3]
    const text = planId == null ? undefined : texts.get(planId)
    return {
      text: text === undefined ? t("pg.detail.plan", { id: planId ?? "—" }) : activityPreview(text),
      prefix: identityPrefix(row, null),
    }
  }
  return <ActivityLedger columns={60} cursor={cursor} cuts={PLAN_CUTS} defaultCut="exec_time" drill={drill} hour={hour} keys={PLAN_KEYS} label={label} locale={locale} onCursor={onCursor} scales={{ blockSize, clockTicks: null }} section="pg_store_plans" storageKey="kronika.activity-open.plans" t={t} />
}

export type RelationActivityLevel = "object" | "schema" | "database" | "tablespace"

const RELATION_GROUPS: Readonly<Record<Exclude<RelationActivityLevel, "object">, readonly string[]>> = {
  schema: ["datname", "schemaname"],
  database: ["datname"],
  tablespace: ["tablespace"],
}

export function RelationsActivity({ blockSize, cursor, hour, level, locale, onCursor, onPattern, section, t }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly hour: number
  readonly level: RelationActivityLevel
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onPattern: (pattern: string) => void
  readonly section: "pg_stat_user_tables" | "pg_stat_user_indexes"
  readonly t: Translate
}) {
  const indexes = section === "pg_stat_user_indexes"
  // Match the grouping selected for the relation table.
  const group = level === "object" ? undefined : RELATION_GROUPS[level]
  const drill = (row: HeatmapViewRow) => {
    const name = level === "object" ? labelText(row, indexes ? "indexrelname" : "relname") : row.identity[row.identity.length - 1]
    if (name != null && name !== "") onPattern(name)
  }
  const label = (row: HeatmapViewRow): RowLabel => {
    if (level !== "object") {
      const parts = row.identity.filter((part): part is string => part != null)
      return {
        text: parts.length === 0 ? "—" : parts.join("."),
        prefix: row.members === null || row.members < 2
          ? null
          : t(indexes ? "activity.indexes.members" : "activity.tables.members", { count: row.members }),
      }
    }
    const datname = labelText(row, "datname")
    const schemaname = labelText(row, "schemaname")
    const relname = labelText(row, "relname")
    const indexrelname = labelText(row, "indexrelname")
    const object = [schemaname, indexes ? indexrelname ?? relname : relname]
      .filter((part): part is string => part != null)
      .join(".")
    return {
      text: object === "" ? "—" : object,
      prefix: datname ?? null,
    }
  }
  return <ActivityLedger columns={12} cursor={cursor} cuts={indexes ? INDEX_CUTS : TABLE_CUTS} defaultCut={indexes ? "idx_scan" : "writes"} drill={drill} group={group} hour={hour} keys={indexes ? INDEX_KEYS : TABLE_KEYS} label={label} locale={locale} onCursor={onCursor} scales={{ blockSize, clockTicks: null }} section={section} storageKey={`kronika.activity-open.${indexes ? "indexes" : "tables"}`} t={t} />
}

export function ProcessesActivity({ cursor, hour, locale, onCursor, onPattern, t, ticksPerSecond }: {
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onPattern: (pattern: string) => void
  readonly t: Translate
  readonly ticksPerSecond: number | null
}) {
  const drill = (row: HeatmapViewRow) => {
    const comm = row.identity[0]
    if (comm != null && comm !== "") onPattern(comm)
  }
  const label = (row: HeatmapViewRow): RowLabel => ({
    text: row.identity[0] ?? "—",
    prefix: row.members === null || row.members < 2 ? null : t("activity.members", { count: row.members }),
  })
  return <ActivityLedger columns={60} cursor={cursor} cuts={PROCESS_CUTS} defaultCut="cpu" drill={drill} group={PROCESS_GROUP} hour={hour} keys={PROCESS_KEYS} label={label} locale={locale} onCursor={onCursor} scales={{ blockSize: null, clockTicks: ticksPerSecond }} section="os_process" storageKey="kronika.activity-open.processes" t={t} />
}

export function DatabasesActivity({ blockSize, cursor, hour, locale, onCursor, onPattern, t }: {
  readonly blockSize: number | null
  readonly cursor: number
  readonly hour: number
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly onPattern: (pattern: string) => void
  readonly t: Translate
}) {
  const drill = (row: HeatmapViewRow) => {
    const datname = labelText(row, "datname")
    if (datname != null && datname !== "") onPattern(datname)
  }
  const label = (row: HeatmapViewRow): RowLabel => ({
    text: labelText(row, "datname") ?? row.identity[0] ?? "—",
    prefix: null,
  })
  return <ActivityLedger columns={60} cursor={cursor} cuts={DATABASE_CUTS} defaultCut="commits" drill={drill} hour={hour} keys={DATABASE_KEYS} label={label} locale={locale} onCursor={onCursor} scales={{ blockSize, clockTicks: null }} section="pg_stat_database" storageKey="kronika.activity-open.databases" t={t} />
}

export function CgroupActivity({ cursor, devices = EMPTY_CGROUP_DEVICES, hour, io, locale, onCursor, t }: {
  readonly cursor: number
  readonly devices?: ReadonlyMap<string, CgroupDevicePresentation>
  readonly hour: number
  readonly io: boolean
  readonly locale: Locale
  readonly onCursor: (timestamp: number) => void
  readonly t: Translate
}) {
  const label = (row: HeatmapViewRow): RowLabel => cgroupActivityIdentity(row, io, devices)
  return <ActivityLedger columns={60} cursor={cursor} cuts={io ? CGROUP_IO_CUTS : CGROUP_CPU_CUTS} defaultCut={io ? "cg_read" : "cg_cpu"} headingContext={io ? cgroupIoSharedPath : undefined} hour={hour} keys={io ? CGROUP_IO_KEYS : CGROUP_CPU_KEYS} label={label} locale={locale} onCursor={onCursor} scales={{ blockSize: null, clockTicks: null }} section={io ? "os_cgroup_io" : "os_cgroup_cpu"} storageKey={`kronika.activity-open.${io ? "cgroup-io" : "cgroup-cpu"}`} t={t} />
}

const EMPTY_CGROUP_DEVICES: ReadonlyMap<string, CgroupDevicePresentation> = new Map()

// A device row is labelled by where its I/O lands: the mount point, else the
// exact device name, else the bare major:minor. The chain down to the disk
// stays in the title.
export function cgroupActivityIdentity(row: HeatmapViewRow, io: boolean, devices: ReadonlyMap<string, CgroupDevicePresentation> = EMPTY_CGROUP_DEVICES): RowLabel {
  if (!io) return { text: row.identity[0] ?? "—", prefix: null }
  const id = row.identity[1] == null ? null : `${row.identity[1]}:${row.identity[2] ?? ""}`
  const presentation = id === null ? undefined : devices.get(cgroupDeviceKey(row.identity[0] ?? null, id))
  if (presentation === undefined) return { text: id ?? "—", prefix: row.identity[0] ?? "—" }
  const primary = cgroupDevicePrimary(presentation)
  return {
    detail: id,
    text: `${primary}${presentation.preferredMounts.length > 1 ? ` +${presentation.preferredMounts.length - 1}` : ""}`,
    title: [primary, cgroupDeviceSecondary(presentation), presentation.id].filter((part, index, all): part is string => part !== null && all.indexOf(part) === index).join(" · "),
    prefix: row.identity[0] ?? "—",
  }
}

export function cgroupIoSharedPath(view: HeatmapView): string | null {
  if (view.rows.length === 0 || view.othersCount !== 0 || view.rows.length !== view.entityCount) return null
  const path = view.rows[0]?.identity[0] ?? null
  return path !== null && view.rows.every((row) => row.identity[0] === path) ? path : null
}

const DATABASE_KEYS: LedgerKeys = { title: "activity.databases", bands: "activity.databases" }
const CGROUP_CPU_KEYS: LedgerKeys = { title: "activity.cgroup_cpu", bands: "activity.cgroups" }
const CGROUP_IO_KEYS: LedgerKeys = { title: "activity.cgroup_io", bands: "activity.cgroups" }

const PROCESS_KEYS: LedgerKeys = { title: "activity.processes", bands: "activity.processes" }

const PROCESS_GROUP = ["comm"] as const

function labelText(row: HeatmapViewRow, name: string): string | null {
  return rawText(row.labels[name] ?? null)
}

function identityPrefix(row: HeatmapViewRow, marker: string | null): string | null {
  const database = labelText(row, "datname")
  const role = labelText(row, "usename")
  const parts = [
    ...(database === null ? [] : [database]),
    ...(role === null || role === database ? [] : [role]),
    ...(marker === null ? [] : [marker]),
  ]
  return parts.length === 0 ? null : parts.join(" · ")
}

function rowQueryId(row: HeatmapViewRow): string | null {
  const stored = row.identity[0]
  return stored == null || stored === "0" ? null : stored
}

function textsById(rows: readonly DataRow[], idField: string, textField: string): ReadonlyMap<string, string> {
  const texts = new Map<string, string>()
  for (const row of rows) {
    const id = rawText(value(row, idField))
    const text = rawText(value(row, textField))
    if (id === null || text === null || text.trim() === "" || texts.has(id)) continue
    texts.set(id, text)
  }
  return texts
}

export function statementTextsByQueryId(rows: readonly DataRow[]): ReadonlyMap<string, string> {
  return textsById(rows, "queryid", "query")
}

export function planTextsByPlanId(rows: readonly DataRow[]): ReadonlyMap<string, string> {
  return textsById(rows, "planid", "plan")
}

function ActivityPanel({ chosen, columns, cursor, cut, cuts, drill, headingContext, hour, keys, label, loadedCut, loading, locale, maximized, onCollapse, onCursor, onCut, onMaximized, onScale, onTop, scale, scales, section, t, top, view }: {
  readonly columns: number
  readonly cursor: number
  readonly cut: ActivityCut
  readonly cuts: readonly ActivityCut[]
  readonly loadedCut: ActivityCut
  readonly loading: boolean
  readonly chosen: string | null
  readonly drill?: ((row: HeatmapViewRow) => void) | undefined
  readonly headingContext: string | null
  readonly hour: number
  readonly keys: LedgerKeys
  readonly label: (row: HeatmapViewRow) => RowLabel
  readonly locale: Locale
  readonly maximized: boolean
  readonly onCollapse: () => void
  readonly onCursor: (timestamp: number) => void
  readonly onCut: (id: string) => void
  readonly onMaximized: (maximized: boolean) => void
  readonly onScale: (scale: ScaleMode) => void
  readonly onTop: (top: number) => void
  readonly scale: ScaleMode
  readonly scales: ActivityScales
  readonly section: string
  readonly t: Translate
  readonly top: number
  readonly view: HeatmapView
}) {
  const { kind, scale: valueScale } = cutScale(loadedCut, scales)
  const suffix = view.cumulative ? t("unit.per_second") : ""
  const cursorColumn = cursorColumnOf(cursor, hour, columns)
  const globalMax = heatmapViewMax(view)
  const totalsMax = view.totals.cells.reduce<number>((current, cell) => cell !== null && cell > current ? cell : current, 0)
  const rowMax = (cells: readonly (number | null)[]) => scale === "row"
    ? cells.reduce<number>((current, cell) => cell !== null && cell > current ? cell : current, 0)
    : globalMax
  const atCursor = (cells: readonly (number | null)[]) => {
    const cell = cursorColumn === null ? null : cells[cursorColumn] ?? null
    return cell === null ? "—" : formatValue(cell * valueScale, kind, locale, suffix)
  }
  const total = (stored: number | null) => stored === null ? "—" : formatValue(stored * valueScale, kind, locale)

  return <section aria-label={t(`${keys.title}.title`)} className={`activity-block${maximized ? " activity-max" : ""}`} data-testid={`activity-${section}`}>
    <header className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-3 py-[6px]">
      <button aria-expanded className="flex cursor-pointer items-center gap-1 border-0 bg-transparent p-0 font-sans text-sm font-medium text-fg2" data-testid="activity-toggle" onClick={onCollapse} type="button">
        <ChevronDown aria-hidden="true" className="flex-none text-fg4" size={13} />
        {t(`${keys.title}.title`)}
      </button>
      {headingContext !== null && <span aria-label={`${t("system.field.cgroup_path.label")}: ${headingContext}`} className="font-sans text-xs text-fg4" data-testid="activity-heading-context">· <span className="font-mono text-fg2">{headingContext}</span></span>}
      <LabelHelp helpKey={`${keys.title}.title.help`} iconOnly labelKey={`${keys.title}.title`} t={t} />
      <div aria-label={t("activity.cut_label")} className="lens-tabs" role="group">
        {cuts.map((candidate) => <button aria-pressed={cut.id === candidate.id} data-testid={`activity-cut-${candidate.id}`} key={candidate.id} onClick={() => onCut(candidate.id)} type="button">{t(`activity.cut.${candidate.id}`)}</button>)}
      </div>
      <LabelHelp helpKey={`activity.cut.${cut.id}.help`} iconOnly labelKey={`activity.cut.${cut.id}`} t={t} />
      <span className="font-sans text-xs text-fg4" data-testid="activity-top-count">{loading ? t("activity.loading") : t("activity.top", { top: String(view.rows.length), total: String(view.entityCount) })}</span>
      <div className="ml-auto flex items-center gap-2">
        {maximized && <div aria-label={t("activity.top_label")} className="lens-tabs" role="group">
          {TOP_CHOICES.map((choice) => <button aria-pressed={top === choice} data-testid={`activity-top-${choice}`} key={choice} onClick={() => onTop(choice)} type="button">{choice}</button>)}
        </div>}
        <div aria-label={t("activity.scale_label")} className="lens-tabs" role="group">
          <button aria-pressed={scale === "global"} data-testid="activity-scale-global" onClick={() => onScale("global")} type="button">{t("activity.scale.global")}</button>
          <button aria-pressed={scale === "row"} data-testid="activity-scale-row" onClick={() => onScale("row")} type="button">{t("activity.scale.row")}</button>
        </div>
        <LabelHelp helpKey="activity.scale.help" iconOnly labelKey="activity.scale_label" t={t} />
        <button aria-label={t(maximized ? "activity.restore" : "activity.maximize")} aria-pressed={maximized} className="inspector-maximize" data-testid="activity-maximize" onClick={() => onMaximized(!maximized)} type="button">{maximized ? <Minimize2 aria-hidden="true" size={13} /> : <Maximize2 aria-hidden="true" size={13} />}</button>
      </div>
    </header>
    {view.entityCount === 0 && <div className="px-3 py-2 font-sans text-sm text-fg4">{t("activity.empty")}</div>}
    {view.entityCount > 0 && <div className={`${maximized ? "min-h-0 flex-1 overflow-y-auto" : ""}${loading ? " animate-pulse opacity-55 transition-opacity" : ""}`} data-loading={loading || undefined}>
      {view.summary === "mean" && <div className="activity-row font-sans text-xs text-fg4" data-testid="activity-column-headings">
        <span aria-hidden="true" />
        <span aria-hidden="true" />
        <span className="px-2 text-right" data-testid="activity-summary-label">{t("activity.average")}</span>
        <strong className="px-2 text-right font-normal">{t("activity.at_cursor")}</strong>
      </div>}
      <ActivityRow cells={view.totals.cells} cursor={cursor} help={<LabelHelp helpKey={`${keys.bands}.totals.help`} iconOnly labelKey="activity.totals" t={t} />} hour={hour} max={totalsMax} muted onCursor={onCursor} reading={atCursor(view.totals.cells)} testId="activity-row-totals" text={t("activity.totals")} total={total(view.totals.total)} />
      {view.rows.map((row) => {
        const { detail = null, prefix, semantic = false, text, title } = label(row)
        return <ActivityRow active={chosen === rowKey(row)} cells={row.cells} cursor={cursor} detail={detail} hour={hour} key={rowKey(row)} labelTitle={title} max={rowMax(row.cells)} onClick={drill === undefined ? undefined : () => drill(row)} onCursor={onCursor} prefix={headingContext !== null && prefix === headingContext ? null : prefix} reading={atCursor(row.cells)} semantic={semantic} testId="activity-row" text={text} total={total(row.total)} />
      })}
      {view.othersCount > 0 && <ActivityRow cells={view.others.cells} cursor={cursor} help={<LabelHelp helpKey={`${keys.bands}.others.help`} iconOnly labelKey={`${keys.bands}.others_label`} t={t} />} hour={hour} max={rowMax(view.others.cells)} muted onCursor={onCursor} reading={atCursor(view.others.cells)} testId="activity-row-others" text={t(`${keys.bands}.others`, { count: String(view.othersCount) })} total={total(view.others.total)} />}
    </div>}
  </section>
}

function ActivityRow({ active = false, cells, cursor, detail = null, help, hour, labelTitle, max, muted = false, onClick, onCursor, prefix = null, reading, semantic = false, testId, text, total }: {
  readonly active?: boolean | undefined
  readonly cells: readonly (number | null)[]
  readonly cursor: number
  readonly detail?: string | null
  readonly help?: React.ReactNode
  readonly hour: number
  readonly labelTitle?: string | undefined
  readonly max: number
  readonly muted?: boolean
  readonly onClick?: (() => void) | undefined
  readonly onCursor: (timestamp: number) => void
  readonly prefix?: string | null
  readonly reading: string
  readonly semantic?: boolean
  readonly testId: string
  readonly text: string
  readonly total: string
}) {
  return <div aria-pressed={onClick === undefined ? undefined : active} className={`activity-row${onClick === undefined ? "" : " activity-row-link"}`} data-active={active || undefined} data-testid={testId} onClick={onClick} role={onClick === undefined ? undefined : "button"}>
    <span className={`flex min-w-0 items-baseline gap-[6px] overflow-hidden px-2 ${muted || semantic ? "font-sans" : "font-mono"} ${detail === null ? "text-xs" : "text-[12px] font-normal"} ${muted ? "text-fg4" : "text-fg2"}`} title={labelTitle ?? [text, detail].filter((part) => part !== null).join(" · ")}>
      {prefix !== null && <span className="flex-none font-sans text-fg4">{prefix}</span>}
      <span className="overflow-hidden text-ellipsis whitespace-nowrap">{text}</span>
      {detail !== null && <span className="flex-none font-mono text-[12px] font-normal text-fg4">{detail}</span>}
      {help}
    </span>
    <ActivityStrip cells={cells} cursor={cursor} hour={hour} max={max} onCursor={onCursor} />
    <strong className="px-2 text-right font-mono text-xs font-normal tabular-nums text-fg2">{total}</strong>
    <strong className="px-2 text-right font-mono text-xs font-normal tabular-nums text-fg3">{reading}</strong>
  </div>
}

function ActivityStrip({ cells, cursor, hour, max, onCursor }: {
  readonly cells: readonly (number | null)[]
  readonly cursor: number
  readonly hour: number
  readonly max: number
  readonly onCursor: (timestamp: number) => void
}) {
  const columns = Math.max(cells.length, 1)
  const cursorX = cursor >= hour && cursor < hour + HOUR_MICROS ? ((cursor - hour) / HOUR_MICROS) * columns : null
  const pick = (event: React.MouseEvent<SVGSVGElement>) => {
    event.stopPropagation()
    const bounds = event.currentTarget.getBoundingClientRect()
    if (bounds.width <= 0) return
    const column = Math.max(0, Math.min(columns - 1, Math.floor(((event.clientX - bounds.left) / bounds.width) * columns)))
    onCursor(intervalInstant(hour, column, columns))
  }
  return <svg className="activity-strip" onClick={pick} preserveAspectRatio="none" viewBox={`0 0 ${columns} 8`}>
    {cells.map((cell, index) => cell === null
      ? null
      : <rect className={`heat-${heatmapIntensity(cell, max)}`} height={8} key={index} width={0.9} x={index + 0.05} y={0} />)}
    {cursorX !== null && <>
      <path className="activity-cursor-halo" d={`M${cursorX.toFixed(3)} 0 V8`} vectorEffect="non-scaling-stroke" />
      <path className="activity-cursor" d={`M${cursorX.toFixed(3)} 0 V8`} vectorEffect="non-scaling-stroke" />
    </>}
  </svg>
}

function formatValue(stored: number, kind: ActivityCut["kind"], locale: Locale, suffix = ""): string {
  if (kind === "bytes") return humanBytes(stored, locale, suffix)
  if (kind === "milliseconds") return humanDuration(stored, locale, "milliseconds", suffix)
  if (kind === "seconds") return humanDuration(stored, locale, "seconds", suffix)
  if (kind === "microseconds") return humanDuration(stored, locale, "microseconds", suffix)
  if (kind === "nanoseconds") return humanDuration(stored, locale, "nanoseconds", suffix)
  return measure(stored, locale, suffix)
}
