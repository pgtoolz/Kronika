import { Copy } from "lucide-react"

import { copyText } from "./clipboard"
import type { Translate } from "./help"
import { planTextSummary } from "./plan-text"
import type { PlanQueryTextState } from "./plan-query"

export function PlanSummary({ raw }: { readonly raw: string | null }) {
  const summary = planTextSummary(raw) ?? "—"
  return <span className="block overflow-hidden text-ellipsis whitespace-nowrap font-medium text-fg2" title={summary}>{summary}</span>
}

export function PlanView({ raw, t }: { readonly raw: string | null; readonly t: Translate }) {
  const available = planTextSummary(raw) !== null
  return <section className="query-block" data-testid="pg-plan-view">
    <header className="flex min-h-7 items-center justify-between gap-2 border-b border-line3 px-2 py-1">
      <strong className="text-xs font-medium text-fg3">{t("pg.plan.text")}</strong>
      {available && raw !== null && <button aria-label={t("pg.plan.copy")} className="inline-flex cursor-pointer items-center gap-1 rounded-[var(--radius-sm)] border border-line3 bg-s2 px-1.5 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s3" onClick={() => void copyText(raw, t("clipboard.manual"))} type="button"><Copy aria-hidden="true" size={12} />{t("pg.plan.copy")}</button>}
    </header>
    {available && raw !== null
      ? <pre className="m-0 max-h-[min(460px,48vh)] overflow-auto whitespace-pre-wrap px-2 py-1.5 text-xs leading-[1.5] text-fg2" data-testid="pg-text-plan">{raw}</pre>
      : <p className="m-0 px-2 py-2 text-sm text-fg3" data-testid="pg-plan-unavailable">{t("pg.plan.unavailable")}</p>}
  </section>
}

export function QueryView({ retry, status, text, t }: PlanQueryTextState & { readonly retry: () => void; readonly t: Translate }) {
  return <section className="query-block p-0" data-query-status={status} data-testid="pg-plan-query-view">
    <header className="flex min-h-7 items-center border-b border-line3 px-2 py-1">
      <strong className="text-xs font-medium text-fg3">{t("pg.query.label")}</strong>
    </header>
    {status === "ready"
      ? <div className="max-h-[min(320px,35vh)] overflow-auto [scrollbar-width:thin]" data-testid="pg-plan-query-list">
        <article data-testid="pg-plan-query-text">
          <div className="flex justify-end px-2 py-1.5">
            <button aria-label={t("pg.query.plan.copy_aria")} className="inline-flex min-h-7 flex-none cursor-pointer items-center gap-1 rounded-[var(--radius-sm)] border border-line3 bg-s2 px-1.5 py-1 text-xs font-medium text-accent3 transition-colors hover:bg-s3" onClick={() => void copyText(text, t("clipboard.manual"))} type="button"><Copy aria-hidden="true" size={12} />{t("pg.plan.copy")}</button>
          </div>
          <pre className="m-0 whitespace-pre-wrap break-words px-2 pb-2 text-sm leading-[1.55] text-event-edge">{text}</pre>
        </article>
      </div>
      : <div aria-live="polite" className="flex min-h-10 items-center justify-between gap-2 px-2 py-2 text-sm text-fg3" data-testid={`pg-plan-query-${status}`}>
        <p className="m-0">{t(`pg.query.plan.${status}`)}</p>
        {status === "error" && <button className="min-h-7 flex-none cursor-pointer border border-line4 bg-s2 px-2 text-xs text-accent3" onClick={retry} type="button">{t("pg.query.plan.retry")}</button>}
      </div>}
  </section>
}
