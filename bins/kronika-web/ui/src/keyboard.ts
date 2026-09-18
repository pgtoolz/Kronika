export function orderedRecordedTimes(timestamps: readonly number[]): readonly number[] {
  return [...new Set(timestamps.filter(Number.isSafeInteger))].sort((left, right) => left - right)
}

export function moveCursor(cursor: number, timestamps: readonly number[], key: string): number {
  if (timestamps.length === 0 || (key !== "ArrowLeft" && key !== "ArrowRight")) return cursor
  let chosen: number | null = null
  if (key === "ArrowLeft") {
    for (const timestamp of timestamps) {
      if (Number.isSafeInteger(timestamp) && timestamp < cursor && (chosen === null || timestamp > chosen)) chosen = timestamp
    }
  } else {
    for (const timestamp of timestamps) {
      if (Number.isSafeInteger(timestamp) && timestamp > cursor && (chosen === null || timestamp < chosen)) chosen = timestamp
    }
  }
  return chosen ?? cursor
}

export function nearestRecordedTime(timestamps: readonly number[], target: number): number | null {
  let closest: number | null = null
  for (const timestamp of timestamps) {
    if (!Number.isSafeInteger(timestamp)) continue
    const distance = Math.abs(timestamp - target)
    if (closest === null || distance < Math.abs(closest - target) || distance === Math.abs(closest - target) && timestamp < closest) closest = timestamp
  }
  return closest
}

export function ownsArrowKeys(tagName: string, contentEditable: boolean): boolean {
  return contentEditable || ["BUTTON", "INPUT", "SELECT", "TEXTAREA"].includes(tagName.toUpperCase())
}

export function keyboardTargetOwnsArrows(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && ownsArrowKeys(target.tagName, target.isContentEditable)
}

// Escape closes an overlay wherever focus is, including the control that
// opened it; "?" toggles help only outside controls, where it would be typed.
export function overlayShortcut(key: string, targetOwnsKeys: boolean, defaultPrevented: boolean): "close" | "help" | null {
  if (defaultPrevented) return null
  if (key === "Escape") return "close"
  if (key === "?" && !targetOwnsKeys) return "help"
  return null
}
