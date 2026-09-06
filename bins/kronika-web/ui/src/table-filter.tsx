import { Check, CircleHelp, Search, X } from "lucide-react"
import { useEffect, useId, useRef, useState, type ReactNode } from "react"
import { createPortal } from "react-dom"

import { copyText } from "./clipboard"
import type { Translate } from "./help"
import { parseSearch, searchFields, type SearchClause, type SearchError, type SearchExpr, type SearchSurface, withoutSearchClause } from "./search"

export function TableFilter({
  accessory,
  context,
  kept,
  onContextClear,
  onPattern,
  pattern,
  grouped = false,
  surface,
  status,
  t,
  total,
}: {
  readonly accessory?: ReactNode | undefined
  readonly context?: string | undefined
  readonly kept: number
  readonly onContextClear?: (() => void) | undefined
  readonly onPattern?: ((pattern: string) => void) | undefined
  readonly pattern: string
  readonly grouped?: boolean | undefined
  readonly surface: SearchSurface
  readonly status?: ReactNode | undefined
  readonly t: Translate
  readonly total: number
}) {
  const [draft, setDraft] = useState(pattern)
  const [submitted, setSubmitted] = useState(false)
  const [help, setHelp] = useState(false)
  const errorId = useId()
  const helpButton = useRef<HTMLButtonElement>(null)
  const input = useRef<HTMLInputElement>(null)
  const draftResult = parseSearch(draft, surface, { grouped })
  const appliedResult = parseSearch(pattern, surface, { grouped })
  const invalidApplied = draft === pattern && pattern !== "" && !appliedResult.ok
  const error = (submitted || invalidApplied) && !draftResult.ok ? draftResult.error : null
  const applied = appliedResult.ok ? appliedResult.query : null
  useEffect(() => {
    setDraft(pattern)
    setSubmitted(false)
  }, [grouped, pattern, surface])
  const apply = () => {
    setSubmitted(true)
    if (!draftResult.ok) return
    setDraft(draftResult.query.canonical)
    setSubmitted(false)
    onPattern?.(draftResult.query.canonical)
  }
  const clear = () => {
    setDraft("")
    setSubmitted(false)
    onPattern?.("")
    input.current?.focus()
  }
  return <div className="border-b border-line2 bg-s2 px-[7px] py-1 text-fg3" data-search-surface={surface}>
    <div className="flex min-h-[28px] min-w-0 flex-wrap items-center gap-1.5">
      {context !== undefined && <span className="inline-flex max-w-[58%] items-center gap-1.5 overflow-hidden whitespace-nowrap rounded-[var(--radius-xs)] bg-accent-soft pl-1.5 text-xs text-fg coarse:min-h-11" data-testid="entity-context-filter">
        <strong className="overflow-hidden text-ellipsis font-semibold">{context}</strong>
        <button className="inline-flex min-w-0 cursor-pointer items-center gap-[3px] self-stretch border-0 border-l border-accent2 bg-transparent px-[5px] text-fg2 coarse:min-w-11" onClick={onContextClear} type="button"><X aria-hidden="true" size={11} />{t("filter.show_all")}</button>
      </span>}
      {context !== undefined && onPattern !== undefined && <span className="text-xs text-fg4">{t("filter.and")}</span>}
      {onPattern !== undefined && <form className="flex min-w-[210px] flex-1 items-center" onSubmit={(event) => { event.preventDefault(); apply() }}>
        <label className={`grid min-w-0 flex-1 grid-cols-[18px_minmax(0,1fr)] items-center rounded-[var(--radius-sm)] border coarse:min-h-11 ${error === null ? "border-line3" : "border-bad bg-[color-mix(in_srgb,var(--color-bad)_8%,transparent)]"}`}>
          <span aria-hidden="true" className="grid h-full w-[18px] place-items-center"><Search size={12} /></span>
          <input
            aria-describedby={error === null ? undefined : errorId}
            aria-errormessage={error === null ? undefined : errorId}
            aria-invalid={error === null ? undefined : true}
            aria-label={t("filter.label")}
            className="min-w-0 w-full border-0 bg-transparent px-1 py-1 font-mono text-sm text-fg outline-none placeholder:font-sans placeholder:text-fg4 [&::-webkit-search-cancel-button]:hidden [&::-webkit-search-cancel-button]:appearance-none"
            data-testid="table-filter"
            onChange={(event) => { setDraft(event.target.value); setSubmitted(false) }}
            onKeyDown={(event) => {
              if (event.key !== "Backspace" || draft !== "" || applied?.structured !== true || applied.clauses.length === 0) return
              event.preventDefault()
              const next = withoutSearchClause(applied, applied.clauses.length - 1)
              setDraft(next)
              onPattern(next)
            }}
            placeholder={t("filter.placeholder")}
            ref={input}
            spellCheck={false}
            type="search"
            value={draft}
          />
        </label>
        <button aria-label={t("filter.apply")} className="ml-1 inline-flex h-7 flex-none cursor-pointer items-center rounded-[var(--radius-sm)] border-0 bg-s3 px-2 text-accent3 transition-colors hover:bg-s4 coarse:h-11 coarse:min-w-11" type="submit"><Check aria-hidden="true" size={14} /></button>
      </form>}
      {onPattern !== undefined && <button aria-expanded={help} aria-label={t("filter.help.open")} className="inline-flex h-7 w-7 flex-none cursor-pointer items-center justify-center rounded-[var(--radius-sm)] border-0 bg-transparent text-accent3 transition-colors hover:bg-s3 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent coarse:h-11 coarse:w-11" onClick={() => setHelp((open) => !open)} ref={helpButton} type="button"><CircleHelp aria-hidden="true" size={14} /></button>}
      {pattern !== "" && <>
        {kept >= 0 && <span className="flex-none text-xs tabular-nums text-fg3">{t("filter.kept", { kept: String(kept), total: String(total) })}</span>}
        <button aria-label={t("filter.clear")} className="inline-flex flex-none cursor-pointer items-center justify-center border-0 bg-transparent p-0.5 text-accent3 coarse:h-11 coarse:w-11" onClick={clear} type="button"><X aria-hidden="true" size={12} /></button>
      </>}
      {accessory}
      {status !== undefined && <span className="ml-auto flex flex-none items-center gap-x-[14px] whitespace-nowrap text-xs text-fg3 [&_strong]:font-semibold [&_strong]:text-fg2" data-testid="table-status">{status}</span>}
    </div>
    {applied?.structured === true && applied.expr !== null && <div aria-label={`${t("filter.tokens")}: ${applied.canonical}`} className="mt-1 flex min-w-0 flex-wrap items-center gap-1" data-testid="search-chips">
      <SearchChips expr={applied.expr} onRemove={(index) => onPattern?.(withoutSearchClause(applied, index))} t={t} />
    </div>}
    {error !== null && <SearchErrorMessage draft={draft} error={error} id={errorId} t={t} />}
    {help && typeof document !== "undefined" && createPortal(<SearchHelp onClose={() => { setHelp(false); queueMicrotask(() => helpButton.current?.focus()) }} surface={surface} t={t} />, document.body)}
  </div>
}

function SearchChips({ expr, onRemove, t }: { readonly expr: SearchExpr; readonly onRemove: (index: number) => void; readonly t: Translate }) {
  let clauseIndex = 0
  const render = (current: SearchExpr, parentPrecedence: number, path: string): ReactNode[] => {
    if (current.kind === "predicate") {
      const index = clauseIndex
      clauseIndex += 1
      const clause = current.predicate
      return [<span className="inline-flex h-[19px] max-w-full items-center rounded-[var(--radius-xs)] bg-accent-soft text-xs text-fg" data-search-predicate="" key={`${path}:${clause.key}:${clause.value}`}>
        <SearchChip clause={clause} t={t} />
        <button aria-label={t("filter.token.remove", { field: clause.field.kind === "quantity" ? t(`filter.field.${clause.key}.label`) : clause.key, value: clause.field.kind === "quantity" ? `${clause.operator} ${clause.quantity?.number ?? clause.value} ${clause.quantity?.unit ?? ""}`.trim() : clause.value })} className="inline-flex self-stretch cursor-pointer items-center border-0 border-l border-accent2 bg-transparent px-1 text-fg2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent" onClick={() => onRemove(index)} type="button"><X aria-hidden="true" size={11} /></button>
      </span>]
    }
    const precedence = current.kind === "and" ? 2 : 1
    const grouped = precedence < parentPrecedence
    return [
      ...(grouped ? [<SearchSyntaxToken key={`${path}:open`} token="(" />] : []),
      ...render(current.left, precedence, `${path}:left`),
      <SearchSyntaxToken key={`${path}:operator`} token={current.kind === "and" ? "AND" : "OR"} />,
      ...render(current.right, precedence, `${path}:right`),
      ...(grouped ? [<SearchSyntaxToken key={`${path}:close`} token=")" />] : []),
    ]
  }
  return render(expr, 0, "root")
}

type SearchSyntaxTokenValue = "AND" | "OR" | "(" | ")"

function SearchSyntaxToken({ token }: { readonly token: SearchSyntaxTokenValue }) {
  return <span className="inline-flex h-[19px] items-center whitespace-nowrap px-0.5 text-[9px] font-medium leading-none text-fg4" data-search-syntax={token === "(" || token === ")" ? "parenthesis" : "connector"}>{token}</span>
}

function SearchChip({ clause, t }: { readonly clause: SearchClause; readonly t: Translate }) {
  if (clause.field.kind !== "quantity") return <span className="max-w-[240px] overflow-hidden text-ellipsis whitespace-nowrap px-1.5 py-0.5"><strong>{clause.key}</strong>: {clause.value}</span>
  return <span className="max-w-[280px] overflow-hidden text-ellipsis whitespace-nowrap px-1.5 py-0.5" title={clause.canonical}>
    <strong>{t(`filter.field.${clause.key}.label`)}</strong><span aria-hidden="true"> · </span>{clause.operator} {clause.quantity?.number}<span aria-hidden="true"> </span>{clause.quantity?.unit}
  </span>
}

function SearchErrorMessage({ draft, error, id, t }: { readonly draft: string; readonly error: SearchError; readonly id: string; readonly t: Translate }) {
  const marked = draft.slice(error.start, Math.max(error.start + 1, error.end)) || " "
  return <div className="mt-1 text-xs leading-normal text-bad" data-testid="search-error" id={id} role="alert">
    <span>{t(`filter.error.${error.code}`, { token: error.token ?? marked })}</span>
    <code aria-hidden="true" className="ml-2 whitespace-pre-wrap text-fg3"><span>{draft.slice(0, error.start)}</span><mark className="bg-[color-mix(in_srgb,var(--color-bad)_28%,transparent)] text-bad underline decoration-wavy">{marked}</mark><span>{draft.slice(error.end)}</span></code>
  </div>
}

function SearchHelp({ onClose, surface, t }: { readonly onClose: () => void; readonly surface: SearchSurface; readonly t: Translate }) {
  const fields = searchFields(surface)
  const examples = searchExamples(surface)
  const dialog = useRef<HTMLElement>(null)
  const close = useRef<HTMLButtonElement>(null)
  useEffect(() => { close.current?.focus() }, [])
  return <div className="fixed inset-0 z-[1100] flex items-start justify-end bg-[color-mix(in_srgb,var(--color-shadow)_34%,transparent)] p-2" data-testid="search-help" onKeyDown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); onClose(); return }
    if (event.key !== "Tab") return
    const focusable = [...(dialog.current?.querySelectorAll<HTMLElement>("button") ?? [])]
    if (focusable.length === 0) return
    const index = focusable.indexOf(document.activeElement as HTMLElement)
    const next = event.shiftKey ? (index <= 0 ? focusable.length - 1 : index - 1) : (index < 0 || index === focusable.length - 1 ? 0 : index + 1)
    event.preventDefault()
    focusable[next]?.focus()
  }} onPointerDown={(event) => { if (event.currentTarget === event.target) onClose() }}>
    <aside aria-label={t("filter.help.title")} aria-modal="true" className="max-h-[calc(100dvh_-_16px)] w-[min(430px,calc(100vw_-_16px))] overflow-auto border border-line4 bg-s1 p-3 text-sm text-fg shadow-[0_8px_24px_var(--color-shadow-a)]" ref={dialog} role="dialog">
      <header className="flex items-center justify-between gap-2"><h2 className="m-0 text-md">{t("filter.help.title")}</h2><button aria-label={t("help.close")} className="icon-button" onClick={onClose} ref={close} type="button">×</button></header>
      <p className="leading-relaxed text-fg2">{t("filter.help.grammar")}</p>
      <p className="leading-relaxed text-fg3">{t("filter.help.rules")}</p>
      <h3 className="mt-3 text-xs font-medium text-fg3">{t("filter.help.fields")}</h3>
      <dl className="m-0 grid gap-2">
        {fields.map((field) => <div className="border-t border-line2 pt-2" key={field.key}><dt><code className="text-accent3">{field.key}</code>{field.aliases.length === 0 ? null : <span className="ml-2 text-xs text-fg4">{t("filter.help.aliases", { aliases: field.aliases.join(", ") })}</span>}</dt><dd className="m-0 mt-1 text-fg3">{t(field.help)}</dd></div>)}
      </dl>
      <h3 className="mt-3 text-xs font-medium text-fg3">{t("filter.help.examples")}</h3>
      <div className="grid gap-1.5">{examples.map((example) => <button aria-label={`${t("filter.help.copy")}: ${example}`} className="min-w-0 cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap border border-line3 bg-s2 px-2 py-1.5 text-left text-fg2" key={example} onClick={() => void copyText(example, t("clipboard.manual"))} type="button"><code>{example}</code></button>)}</div>
    </aside>
  </div>
}

function searchExamples(surface: SearchSurface): readonly string[] {
  if (surface === "pg_store_plans" || surface === "pg_stat_statements") return ["exec_time_rate>500ms/s AND call_rate>1/s"]
  if (surface === "pg_stat_activity") return ["query_id:-912345", 'database:app AND (text:"select orders*" OR text:"update orders*")']
  if (surface === "pg_stat_user_tables") return ["size>100MB", "(schema:public OR schema:audit) AND size>100MiB", "schema:public AND (buffer_hit<90% OR seq_scan_rate>0.5/s)"]
  if (surface === "pg_stat_user_indexes") return ["size>100MB", "(schema:public OR schema:audit) AND buffer_hit<99.5%", "table_name:orders AND (scan_rate>10/s OR size>100MiB)"]
  if (surface === "os_process") return ["cpu_cores>0.1 AND rss>2MiB"]
  if (surface === "events") return ["kind:notable AND source:postgres*", 'text:"lock timeout"']
  return ["database:app", "state:active"]
}
