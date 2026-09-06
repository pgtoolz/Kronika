import { loadSeries, type Cell, type DataRow, type StatementScope } from "./api"
import { LabelHelp, type Translate } from "./help"
import { useHistoryRequest, type HistoryState } from "./history-request"
import { asNumber, humanBytes, humanDuration, humanPercent, measure, value, type Locale } from "./model"

export type PostgresSummarySection = "databases" | "indexes" | "plans" | "statements" | "tables"
export type PostgresSummaryState = HistoryState<readonly DataRow[]>

interface Fact {
  readonly key: string
  readonly output: string
}

const SURFACES: Readonly<Record<PostgresSummarySection, number>> = {
  statements: 1,
  plans: 2,
  databases: 3,
  tables: 4,
  indexes: 5,
}

export function usePostgresSummary(hour: number, historyRevision: number, scope: StatementScope = "all"): PostgresSummaryState {
  return useHistoryRequest(`${hour}:${scope}`, historyRevision,
    (signal) => loadSeries(hour, "postgresql_summary", {}, [], signal, undefined, undefined, scope))
}

export function postgresSummaryRow(rows: readonly DataRow[], section: PostgresSummarySection, cursor: number): DataRow | null {
  const surface = SURFACES[section]
  let selected: DataRow | null = null
  for (const row of rows) {
    if (row.logicalName !== "postgresql_summary" || asNumber(value(row, "surface")) !== surface || row.timestamp > cursor) continue
    if (selected === null || row.timestamp > selected.timestamp) selected = row
  }
  return selected
}

export function PostgresSummary({ cursor, lens, locale, section, state, t }: {
  readonly cursor: number
  readonly lens: string
  readonly locale: Locale
  readonly section: PostgresSummarySection
  readonly state: PostgresSummaryState
  readonly t: Translate
}) {
  const row = postgresSummaryRow(state.value ?? [], section, cursor)
  const facts = row === null ? [] : summaryFacts(row, section, lens, locale, t)
  const statusKey = row !== null ? null : state.status === "loading" ? "pg.summary.loading" : state.status === "error" ? "pg.summary.error" : "pg.summary.empty"
  return <section aria-label={t("pg.summary.title")} className="process-summary-inline flex min-w-0 flex-1 items-center gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden min-[521px]:max-[760px]:flex-wrap min-[521px]:max-[760px]:overflow-visible" data-status={state.status}>
    {facts.map((fact) => <div className="flex h-[25px] flex-none items-center gap-1.5 px-2" data-summary-fact={fact.key} key={fact.key}>
      <span className="whitespace-nowrap text-xs font-medium text-fg3">{t(`pg.summary.${fact.key}`)}</span>
      <strong className="flex-none whitespace-nowrap font-mono text-xs tabular-nums text-fg">{fact.output}</strong>
      <LabelHelp helpKey={`pg.summary.${fact.key}.help`} iconOnly labelKey={`pg.summary.${fact.key}`} t={t} />
    </div>)}
    {statusKey !== null && <p aria-live="polite" className="m-0 flex h-[25px] flex-none items-center px-2 text-xs font-medium text-fg4" data-testid="postgres-summary-status">{t(statusKey)}</p>}
  </section>
}

function summaryFacts(row: DataRow, section: PostgresSummarySection, lens: string, locale: Locale, t: Translate): readonly Fact[] {
  const number = (field: string) => value(row, field)
  const percent = (field: string) => humanPercent(number(field), locale)
  const fact = (key: string, output: string): Fact => ({ key, output })
  const reads = () => fact("reads_outside_buffers", percent("buffer_read_pct"))
  if (section === "statements") {
    if (lens === "load") return [fact("active_statements", `${measure(number("active_count"), locale)} · ${percent("active_pct")}`)]
    if (lens === "per_call") return [fact("execution_per_call", humanDuration(number("mean_exec_ms"), locale))]
    if (lens === "io") return [reads()]
    if (lens === "resources") return [fact("wal_per_call", humanBytes(number("wal_bytes_per_call"), locale, t("unit.per_call")))]
    return []
  }
  if (section === "plans") {
    if (lens === "load") return [fact("used_plans", `${measure(number("active_count"), locale)} · ${percent("active_pct")}`)]
    if (lens === "timing") return [fact("execution_per_call", humanDuration(number("mean_exec_ms"), locale))]
    if (lens === "io") return [reads()]
    return lens === "identity" ? [fact("executions_per_used_plan", measure(number("calls_per_active"), locale))] : []
  }
  if (section === "databases") return [
    fact("rollbacks", percent("rollback_pct")),
    fact("temp_per_transaction", humanBytes(number("temp_bytes_per_transaction"), locale)),
    reads(),
  ]
  if (section === "tables") {
    if (lens === "access") return [fact("scan_methods", complementary(percent("seq_scan_pct"), number("seq_scan_pct"), locale, t("pg.summary.part.seq"), t("pg.summary.part.index")))]
    if (lens === "changes") return [fact("hot_updates", percent("hot_update_pct")), fact("dead_rows", percent("dead_tuple_pct"))]
    if (lens === "maintenance") return [fact("vacuumed_tables", percent("vacuumed_pct"))]
    if (lens === "size_buffers") return [fact("storage", complementary(percent("toast_pct"), number("toast_pct"), locale, t("pg.summary.part.toast"), t("pg.summary.part.main"), true)), reads()]
    return lens === "freeze" ? [fact("xid_boundary", percent("xid_boundary_pct"))] : []
  }
  if (lens === "usage") return [fact("scanned_indexes", percent("scanned_pct"))]
  if (lens === "low_activity") return [fact("without_scans", percent("no_scan_pct"))]
  if (lens === "size_buffers") return [reads()]
  return lens === "state" ? [fact("usable_indexes", percent("usable_pct"))] : []
}

function complementary(storedOutput: string, stored: Cell, locale: Locale, storedLabel: string, complementLabel: string, complementFirst = false): string {
  const number = asNumber(stored)
  if (number === null) return "—"
  const complement = humanPercent(100 - number, locale)
  return complementFirst
    ? `${complementLabel} ${complement} · ${storedLabel} ${storedOutput}`
    : `${storedLabel} ${storedOutput} · ${complementLabel} ${complement}`
}
