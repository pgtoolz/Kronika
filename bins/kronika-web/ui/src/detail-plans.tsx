import { useEffect, useState } from "react"

import { acceptResponse, loadRelatedPlanRows, loadRelatedStatementRow, type DataRow, type SegmentBound } from "./api"
import { DetailList, DetailRow } from "./detail-list"
import { LabelHelp, type Translate } from "./help"
import { asNumber, humanDuration, identifier, measure, rawText, value, type Locale } from "./model"
import { decoratePostgresIntervalRow } from "./postgres-metrics"
import { plansForPlanId, type RelatedNavigation } from "./statement-navigation"

// The statements ↔ plans relation as peer panels: the rows of the other
// recorded section under the selection's identity, read from the newest
// snapshot at or before the cursor. Counters arrive as the stored interval's
// rates — the same readings the target table's load lens shows for the rows.

export function StatementPlansPanel({ cursor, expression, locale, onRelated, segments, t }: {
  readonly cursor: number
  readonly expression: string
  readonly locale: Locale
  readonly onRelated: (target: RelatedNavigation) => void
  readonly segments: readonly SegmentBound[]
  readonly t: Translate
}) {
  const [plans, setPlans] = useState<readonly DataRow[] | undefined>(undefined)
  useEffect(() => {
    setPlans(undefined)
    const controller = new AbortController()
    acceptResponse(
      loadRelatedPlanRows(segments, cursor, expression, controller.signal),
      controller.signal,
      (rows) => setPlans(rows),
      () => setPlans([]),
    )
    return () => controller.abort()
  }, [cursor, expression, segments])
  if (plans === undefined) {
    return <section className="p-3" data-testid="statement-plans-panel"><p className="m-0 text-sm text-fg4">{t("history.loading")}</p></section>
  }
  if (plans.length === 0) {
    return <section className="p-3" data-testid="statement-plans-panel"><p className="m-0 text-sm text-fg4">{t("pg.related.plans_missing")}</p></section>
  }
  return <section className="p-3" data-testid="statement-plans-panel">
    <p className="m-0 mb-2 text-xs text-fg3">{t("pg.related.plans_count", { count: plans.length })}</p>
    <ul className="m-0 grid list-none gap-1 p-0">
      {plans.map((plan) => {
        const target = plansForPlanId(plan)
        const decorated = decoratePostgresIntervalRow(plan)
        const planId = identifier(value(plan, "planid"))
        const mean = asNumber(value(decorated, "mean_exec_ms_per_call"))
        const calls = asNumber(value(decorated, "calls_per_second"))
        const facts = [
          ...(mean === null ? [] : [`${humanDuration(mean, locale)}${t("pg.related.per_call")}`]),
          ...(calls === null ? [] : [measure(calls, locale, t("unit.per_second"))]),
        ].join(" · ")
        return <li key={`${plan.typeId}:${plan.ordinal}`}>
          <button
            className="grid w-full cursor-pointer grid-cols-[minmax(0,1fr)_auto] items-baseline gap-2 rounded-[var(--radius-xs)] border border-line2 bg-s1 px-2 py-1.5 text-left transition-colors hover:bg-s3 disabled:cursor-default disabled:hover:bg-s1"
            disabled={target === null}
            onClick={() => { if (target !== null) onRelated(target) }}
            type="button"
          >
            <span className="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-xs text-accent3">{planId}</span>
            <span className="whitespace-nowrap font-mono text-xs tabular-nums text-fg3">{facts}</span>
          </button>
        </li>
      })}
    </ul>
  </section>
}

export function PlanStatementPanel({ cursor, locale, onRelated, segments, t, target }: {
  readonly cursor: number
  readonly locale: Locale
  readonly onRelated: (target: RelatedNavigation) => void
  readonly segments: readonly SegmentBound[]
  readonly t: Translate
  readonly target: RelatedNavigation
}) {
  const [statement, setStatement] = useState<DataRow | null | undefined>(undefined)
  useEffect(() => {
    setStatement(undefined)
    const controller = new AbortController()
    acceptResponse(
      loadRelatedStatementRow(segments, cursor, target.expression, controller.signal),
      controller.signal,
      (row) => setStatement(row),
      () => setStatement(null),
    )
    return () => controller.abort()
  }, [cursor, segments, target.expression])
  if (statement === undefined) {
    return <section className="p-3" data-testid="plan-statement-panel"><p className="m-0 text-sm text-fg4">{t("history.loading")}</p></section>
  }
  if (statement === null) {
    return <section className="p-3" data-testid="plan-statement-panel"><p className="m-0 text-sm text-fg4">{t("pg.related.statement_missing")}</p></section>
  }
  const decorated = decoratePostgresIntervalRow(statement)
  const query = rawText(value(statement, "query"))
  const readings = [
    ["calls_per_second", (cell: number) => measure(cell, locale, t("unit.per_second"))],
    ["rows_per_second", (cell: number) => measure(cell, locale, t("unit.per_second"))],
    ["execution_ms_per_second", (cell: number) => `${humanDuration(cell, locale)}${t("unit.per_second")}`],
    ["mean_exec_ms_per_call", (cell: number) => humanDuration(cell, locale)],
  ] as const
  return <section className="p-3" data-testid="plan-statement-panel">
    <button className="mb-2 min-h-7 cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s2 px-2 text-xs font-medium text-accent3 transition-colors hover:bg-s3" onClick={() => onRelated(target)} type="button">{t("pg.plan.open_query")}</button>
    {query !== null && <section className="query-block"><span>{t("pg.query.label")}</span><pre data-testid="plan-statement-query">{query}</pre></section>}
    <DetailList>
      {readings.map(([field, format]) => {
        const cell = asNumber(value(decorated, field))
        return cell === null ? null : <DetailRow key={field} term={<LabelHelp helpKey={`pg.field.${field}.help`} labelKey={`pg.field.${field}.label`} t={t} />}>{format(cell)}</DetailRow>
      })}
    </DetailList>
  </section>
}
