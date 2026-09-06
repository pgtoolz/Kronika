import { useEffect, useState } from "react"

import type { Translate } from "./help"
import { humanAge, humanBytes, type Locale } from "./model"

export interface LoadProgress {
  readonly received: number
  readonly startedAt: number
  readonly lastSeconds: number | null
}

const RAIL_CHIPS = 7
const TABLE_COLUMNS = [176, 96, 128, 112, 96, 144, 112, 96] as const
const TABLE_ROWS = 14

// While the hour loads, the workspace paints its own shape — the timeline
// band, a status line with the honest transfer facts, and table rows — so
// arriving data replaces placeholders instead of a modal-looking card.
export function HourSkeleton({ locale, progress, t }: {
  readonly locale: Locale
  readonly progress?: LoadProgress | undefined
  readonly t: Translate
}) {
  const [now, setNow] = useState(() => Date.now() * 1_000)
  useEffect(() => {
    if (progress === undefined) return
    const timer = setInterval(() => setNow(Date.now() * 1_000), 500)
    return () => clearInterval(timer)
  }, [progress === undefined]) // eslint-disable-line react-hooks/exhaustive-deps
  return <div className="flex min-h-0 flex-1 flex-col" data-testid="hour-skeleton">
    <div aria-hidden="true" className="timeline-shell mt-2 flex h-[124px] flex-none flex-col overflow-hidden border-y border-line2 bg-s1">
      <div className="flex h-7 flex-none items-center gap-2 border-b border-line2 px-2">
        {Array.from({ length: RAIL_CHIPS }, (_, index) => <span className="h-3 min-w-0 flex-1 animate-skeleton rounded-[3px] bg-s3 motion-reduce:animate-none" key={index} style={{ animationDelay: `${index * -90}ms` }} />)}
      </div>
      <div className="flex min-h-0 flex-1 px-2 pb-3 pt-2">
        <span className="h-full w-full animate-skeleton rounded-[3px] bg-s2 motion-reduce:animate-none" />
      </div>
    </div>
    <p className="m-0 flex min-h-[28px] flex-none flex-wrap items-center gap-2 border-b border-line2 bg-s2 px-2 font-sans text-xs text-fg3" role="status">
      <span aria-hidden="true" className="loading-ring animate-loading-spin motion-reduce:animate-none h-3 w-3 !mr-0" />
      {t("status.loading")}
      {progress !== undefined && <span className="font-mono tabular-nums text-fg4">{progressDetail(progress, now, locale, t)}</span>}
    </p>
    <div aria-hidden="true" className="min-h-0 flex-1 overflow-hidden bg-s1">
      <div className="flex h-head flex-none items-center gap-4 border-b border-line2 px-2">
        {TABLE_COLUMNS.map((width, index) => <span className="h-2.5 flex-none animate-skeleton rounded-[3px] bg-s3 motion-reduce:animate-none" key={index} style={{ width: width * 0.6 }} />)}
      </div>
      {Array.from({ length: TABLE_ROWS }, (_, row) => <div className="flex h-row items-center gap-4 border-b border-line px-2" key={row}>
        {TABLE_COLUMNS.map((width, column) => <span className="h-2 flex-none animate-skeleton rounded-[3px] bg-s3 motion-reduce:animate-none" key={column} style={{ animationDelay: `${row * -90}ms`, width: width * (0.4 + (row * 7 + column * 13) % 40 / 100) }} />)}
      </div>)}
    </div>
  </div>
}

function progressDetail(progress: LoadProgress, now: number, locale: Locale, t: Translate): string {
  const elapsed = humanAge(Math.max(0, (now - progress.startedAt) / 1_000_000), locale)
  const parts = [t("status.loading_received", { bytes: humanBytes(progress.received, locale) }), elapsed]
  if (progress.lastSeconds !== null) parts.push(t("status.loading_last", { age: humanAge(progress.lastSeconds, locale) }))
  return parts.join(" · ")
}
