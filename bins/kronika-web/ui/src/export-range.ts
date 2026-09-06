const MICROS = 1_000_000
const HOUR_SECONDS = 3_600

// Inclusive whole Unix seconds, matching the export endpoint.
export interface ExportRange {
  readonly from: number
  readonly to: number
}

export type ExportPreset = "hour" | "around5" | "around15" | "around30"

export const EXPORT_PRESETS: readonly ExportPreset[] = ["hour", "around5", "around15", "around30"]

const AROUND_MINUTES: Readonly<Record<Exclude<ExportPreset, "hour">, number>> = { around5: 5, around15: 15, around30: 30 }

export function hourRange(hourMicros: number): ExportRange {
  const from = Math.floor(hourMicros / MICROS)
  return { from, to: from + HOUR_SECONDS - 1 }
}

// A window around the cursor: N minutes before it and N minutes after it,
// whole seconds, exactly 2N minutes long.
export function aroundCursor(cursorMicros: number, minutes: number): ExportRange {
  const centre = Math.floor(cursorMicros / MICROS)
  return { from: centre - minutes * 60, to: centre + minutes * 60 - 1 }
}

export function presetRange(preset: ExportPreset, hourMicros: number, cursorMicros: number): ExportRange {
  return preset === "hour" ? hourRange(hourMicros) : aroundCursor(cursorMicros, AROUND_MINUTES[preset])
}

export function activePreset(range: ExportRange, hourMicros: number, cursorMicros: number): ExportPreset | null {
  return EXPORT_PRESETS.find((preset) => sameRange(presetRange(preset, hourMicros, cursorMicros), range)) ?? null
}

export function shiftRange(range: ExportRange, seconds: number): ExportRange {
  return { from: range.from + seconds, to: range.to + seconds }
}

export function sameRange(left: ExportRange, right: ExportRange): boolean {
  return left.from === right.from && left.to === right.to
}

export function rangeSeconds(range: ExportRange): number {
  return range.to - range.from + 1
}

export function validRange(range: ExportRange): boolean {
  return Number.isSafeInteger(range.from) && Number.isSafeInteger(range.to) && range.from > 0 && range.from <= range.to
}

// Match the server filename before requesting the export.
export function exportFilename(range: ExportRange): string {
  return `kronika-${utcStamp(range.from)}-${utcStamp(range.to)}-utc.html`
}

function utcStamp(second: number): string {
  const date = new Date(second * 1_000)
  const two = (value: number) => String(value).padStart(2, "0")
  return `${date.getUTCFullYear()}-${two(date.getUTCMonth() + 1)}-${two(date.getUTCDate())}-${two(date.getUTCHours())}${two(date.getUTCMinutes())}${two(date.getUTCSeconds())}`
}

// The part of the range the current hour's timeline can show, in microseconds.
export function rangeOnHour(range: ExportRange, hourMicros: number): { readonly from: number; readonly to: number } | null {
  const from = Math.max(range.from * MICROS, hourMicros)
  const to = Math.min((range.to + 1) * MICROS, hourMicros + HOUR_SECONDS * MICROS)
  return from < to ? { from, to } : null
}

export const EXPORT_SECONDS_KEY = "kronika.export-seconds"

export function readLastExportSeconds(storage: Pick<Storage, "getItem">): number | null {
  try {
    const stored = storage.getItem(EXPORT_SECONDS_KEY)
    if (stored === null) return null
    const seconds = Number(stored)
    return Number.isFinite(seconds) && seconds > 0 ? seconds : null
  } catch {
    return null
  }
}

export function writeLastExportSeconds(storage: Pick<Storage, "setItem">, seconds: number): void {
  try {
    storage.setItem(EXPORT_SECONDS_KEY, String(Math.round(seconds * 10) / 10))
  } catch {}
}
