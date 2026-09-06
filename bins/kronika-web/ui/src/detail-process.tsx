import { Copy } from "lucide-react"

import { copyText } from "./clipboard"
import type { DataRow } from "./api"
import { DetailList, DetailRow } from "./detail-list"
import { useDisplayTime } from "./display-time-context"
import { LabelHelp, type Translate } from "./help"
import { identifier, processCommand, rawText, value, type Locale } from "./model"
import { processDetailFields, processDetailValueRole } from "./detail"
import { CellValue } from "./process-table"

// The OS process recorded under the selected backend's PID. It is the same
// identity in another recorded section, so it reads as its own panel.

export function ProcessFacts({ locale, process, processTime, t }: {
  readonly locale: Locale
  readonly process: DataRow | null | undefined
  readonly processTime: number | null
  readonly t: Translate
}) {
  const time = useDisplayTime()
  if (process === undefined) {
    return <section className="p-3" data-testid="backend-process-panel"><p className="m-0 text-sm text-fg4">{t("history.loading")}</p></section>
  }
  if (process === null) {
    return <section className="p-3" data-testid="backend-process-panel"><p className="m-0 text-sm text-fg4">{t("pg.related.process_missing")}</p></section>
  }
  const command = processCommand(process)
  const pid = identifier(value(process, "pid"))
  const commandPath = rawText(value(process, "cmdline"))?.trim() ? `/proc/${pid}/cmdline` : `/proc/${pid}/comm`
  return <section className="p-3" data-testid="backend-process-panel">
    <section className="flex items-center gap-1.5 border-y border-line2 bg-s1 px-1.5 py-[5px]" title={commandPath}>
      <code className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-sm leading-[1.5] text-fg">{command}</code>
      <button aria-label={t("common.raw")} className="inline-flex flex-none cursor-pointer items-center justify-center rounded-[var(--radius-xs)] border-0 bg-transparent p-1 text-accent3 transition-colors hover:bg-s3" onClick={() => void copyText(command, t("clipboard.manual"))} type="button"><Copy aria-hidden="true" size={12} /></button>
    </section>
    <DetailList>
      <DetailRow term={<LabelHelp helpKey="detail.pg_snapshot.help" labelKey="detail.pg_snapshot.label" t={t} />} valueRole="machine">{processTime === null ? "—" : time.timestamp(processTime)}</DetailRow>
      <DetailRow term={<LabelHelp helpKey="col.pid.help" labelKey="col.pid.label" t={t} />} valueRole="machine">{pid}</DetailRow>
      {processDetailFields("generic", process).map((field) => <DetailRow key={field.id} term={<LabelHelp helpKey={field.help} labelKey={field.label} t={t} />} valueRole={processDetailValueRole(field)}>
        <CellValue field={field} linked={false} locale={locale} row={process} t={t} ticksPerSecond={null} />
      </DetailRow>)}
    </DetailList>
  </section>
}
