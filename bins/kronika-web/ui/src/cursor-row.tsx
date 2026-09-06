import type { Translate } from "./help"
import { moveCursor } from "./keyboard"

// A phone has no arrow keys, and tapping the plot resolves to about nine
// seconds per pixel. These two buttons call the same step the keyboard calls,
// so the cursor lands on a recorded instant instead of near one.
//
// The reading gets its own full-width band because the saturated health split
// needs about 310 px: flanked by two 44 px steps it would ellipsise, and a
// clipped three-part split reads as one number.
export function CursorRow({
  cursor,
  cursorTimes,
  onCursor,
  reading,
  t,
}: {
  readonly cursor: number
  readonly cursorTimes: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly reading: string
  readonly t: Translate
}) {
  const previous = moveCursor(cursor, cursorTimes, "ArrowLeft")
  const next = moveCursor(cursor, cursorTimes, "ArrowRight")
  return <div className="cursor-row" data-testid="cursor-row">
    <span className="cursor-row-reading" data-testid="cursor-row-reading">{reading}</span>
    <button aria-label={t("hour.cursor_previous")} className="cursor-row-step" disabled={previous === cursor} onClick={() => onCursor(previous)} title={t("hour.cursor_previous")} type="button"><span aria-hidden="true">◀</span></button>
    <button aria-label={t("hour.cursor_next")} className="cursor-row-step" disabled={next === cursor} onClick={() => onCursor(next)} title={t("hour.cursor_next")} type="button"><span aria-hidden="true">▶</span></button>
  </div>
}
