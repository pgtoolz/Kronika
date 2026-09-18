import type { Translate } from "./help"
import { moveCursor } from "./keyboard"
import type { CursorNavigation } from "./cursor-navigation"

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
  navigation = null,
  reading,
  t,
}: {
  readonly cursor: number
  readonly cursorTimes: readonly number[]
  readonly onCursor: (timestamp: number) => void
  readonly navigation?: CursorNavigation | null
  readonly reading: string
  readonly t: Translate
}) {
  const previous = moveCursor(cursor, cursorTimes, "ArrowLeft")
  const next = moveCursor(cursor, cursorTimes, "ArrowRight")
  return <div className="cursor-row" data-testid="cursor-row">
    <span className="cursor-row-reading" data-testid="cursor-row-reading">{reading}</span>
    <button aria-label={t("hour.cursor_previous")} className="cursor-row-step" disabled={navigation?.pending ?? previous === cursor} onClick={() => navigation === null ? onCursor(previous) : navigation.step("previous")} title={t("hour.cursor_previous")} type="button"><span aria-hidden="true">◀</span></button>
    <button aria-label={t("hour.cursor_next")} className="cursor-row-step" disabled={navigation?.pending ?? next === cursor} onClick={() => navigation === null ? onCursor(next) : navigation.step("next")} title={t("hour.cursor_next")} type="button"><span aria-hidden="true">▶</span></button>
  </div>
}
