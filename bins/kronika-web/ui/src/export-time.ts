import type { DisplayTimeZone } from "./display-time"
import type { Locale } from "./model"

const MICROSECONDS_PER_SECOND = 1_000_000
const SECONDS_PER_HOUR = 3_600
const SECONDS_PER_DAY = 86_400
const DATE = /^(\d{4})-(\d{2})-(\d{2})$/
const TIME = /^(\d{2}):(\d{2}):(\d{2})$/

type CivilSecond = readonly [number, number, number, number, number, number]

export interface ExportEndpointValue {
  readonly date: string
  readonly second: number
  readonly time: string
}

export interface ExportRangeDefaults {
  readonly from: ExportEndpointValue
  readonly to: ExportEndpointValue
}

export type ExportEndpointError = "date_required" | "date_invalid" | "time_required" | "time_invalid" | "nonexistent" | "occurrence_required"

export interface ExportEndpointResolution {
  readonly candidates: readonly number[]
  readonly error: ExportEndpointError | null
  readonly occurrence: number | null
  readonly second: number | null
}

export interface ExportEndpointPreference {
  readonly occurrence: number | null
  readonly preferred: number | null
}

export function exportRangeDefaults(hourMicroseconds: number, mode: DisplayTimeZone): ExportRangeDefaults {
  if (!Number.isSafeInteger(hourMicroseconds) || hourMicroseconds % MICROSECONDS_PER_SECOND !== 0) {
    throw new RangeError("the selected hour must have whole-second precision")
  }
  const fromSecond = hourMicroseconds / MICROSECONDS_PER_SECOND
  return {
    from: formatExportEndpoint(fromSecond, mode),
    to: formatExportEndpoint(fromSecond + SECONDS_PER_HOUR - 1, mode),
  }
}

export function formatExportEndpoint(second: number, mode: DisplayTimeZone): ExportEndpointValue {
  if (!Number.isSafeInteger(second)) throw new RangeError("export time must be a safe whole second")
  const date = new Date(second * 1_000)
  if (!Number.isFinite(date.getTime())) throw new RangeError("export time is outside the browser date range")
  const read = mode === "utc"
    ? [date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), date.getUTCHours(), date.getUTCMinutes(), date.getUTCSeconds()]
    : [date.getFullYear(), date.getMonth() + 1, date.getDate(), date.getHours(), date.getMinutes(), date.getSeconds()]
  const year = read[0]
  if (year === undefined || year < 0 || year > 9_999) throw new RangeError("export time cannot be represented by the editor")
  return {
    date: `${String(year).padStart(4, "0")}-${two(read[1])}-${two(read[2])}`,
    second,
    time: `${two(read[3])}:${two(read[4])}:${two(read[5])}`,
  }
}

export function resolveExportEndpoint(
  date: string,
  time: string,
  mode: DisplayTimeZone,
  preference: ExportEndpointPreference = { occurrence: null, preferred: null },
): ExportEndpointResolution {
  if (date.trim() === "") return unresolved("date_required")
  const dateMatch = DATE.exec(date.trim())
  if (dateMatch === null) return unresolved("date_invalid")
  if (time.trim() === "") return unresolved("time_required")
  const timeMatch = TIME.exec(time.trim())
  if (timeMatch === null) return unresolved("time_invalid")

  const values: CivilSecond = [
    Number(dateMatch[1]), Number(dateMatch[2]), Number(dateMatch[3]),
    Number(timeMatch[1]), Number(timeMatch[2]), Number(timeMatch[3]),
  ]
  const [, month, day, hour, minute, second] = values
  if (month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 59) {
    return unresolved(month < 1 || month > 12 || day < 1 || day > 31 ? "date_invalid" : "time_invalid")
  }

  const wallSecond = utcSecond(values)
  if (wallSecond === null || !matchesCivilSecond(new Date(wallSecond * 1_000), values, true)) return unresolved("date_invalid")
  if (mode === "utc") return { candidates: [wallSecond], error: null, occurrence: 0, second: wallSecond }

  const candidates = localCivilCandidates(values, wallSecond)
  if (candidates.length === 0) return unresolved("nonexistent")
  if (candidates.length === 1) return { candidates, error: null, occurrence: 0, second: candidates[0] ?? null }
  const explicit = preference.occurrence !== null && candidates[preference.occurrence] !== undefined
    ? preference.occurrence : null
  const occurrence = explicit ?? preferredOccurrence(candidates, values, wallSecond, preference.preferred)
  if (occurrence === null) return { candidates, error: "occurrence_required", occurrence: null, second: null }
  return { candidates, error: null, occurrence, second: candidates[occurrence] ?? null }
}

export function exportDurationSeconds(from: number | null, to: number | null): number | null {
  if (from === null || to === null || !Number.isSafeInteger(from) || !Number.isSafeInteger(to) || from > to) return null
  const seconds = to - from + 1
  return Number.isSafeInteger(seconds) ? seconds : null
}

export function formatExportDuration(seconds: number, locale: Locale): string {
  if (!Number.isSafeInteger(seconds) || seconds < 1) throw new RangeError("export duration must be a positive whole second")
  const units = locale === "ru"
    ? ["д", "ч", "мин", "с"] as const
    : ["d", "h", "min", "s"] as const
  const values = [
    Math.floor(seconds / SECONDS_PER_DAY),
    Math.floor(seconds % SECONDS_PER_DAY / SECONDS_PER_HOUR),
    Math.floor(seconds % SECONDS_PER_HOUR / 60),
    seconds % 60,
  ]
  const number = new Intl.NumberFormat(locale)
  const parts = values.flatMap((value, index) => value === 0 ? [] : [`${number.format(value)}\u00a0${units[index]}`])
  return parts.length === 0 ? `0\u00a0${units[3]}` : parts.join(" ")
}

function unresolved(error: ExportEndpointError): ExportEndpointResolution {
  return { candidates: [], error, occurrence: null, second: null }
}

function localCivilCandidates(values: CivilSecond, wallSecond: number): readonly number[] {
  // Browser civil time has no offset in the editor. Sampling the surrounding
  // IANA rules yields every offset that can map this wall second, including
  // the thirty-minute transition used by Australia/Lord_Howe.
  const offsets = new Set<number>()
  const probe = new Date(0)
  for (let delta = -2 * SECONDS_PER_DAY; delta <= 2 * SECONDS_PER_DAY; delta += 15 * 60) {
    probe.setTime((wallSecond + delta) * 1_000)
    if (Number.isFinite(probe.getTime())) offsets.add(probe.getTimezoneOffset() * 60)
  }
  const candidates = [...offsets].flatMap((offset) => {
    const candidate = wallSecond + offset
    if (!Number.isSafeInteger(candidate)) return []
    return matchesCivilSecond(new Date(candidate * 1_000), values, false) ? [candidate] : []
  })
  return [...new Set(candidates)].sort((left, right) => left - right)
}

function preferredOccurrence(
  candidates: readonly number[],
  values: CivilSecond,
  wallSecond: number,
  preferred: number | null,
): number | null {
  if (preferred === null || !Number.isSafeInteger(preferred)) return null
  const preferredDate = new Date(preferred * 1_000)
  if (!Number.isFinite(preferredDate.getTime())) return null
  const preferredWall = utcSecond([
    preferredDate.getFullYear(), preferredDate.getMonth() + 1, preferredDate.getDate(),
    preferredDate.getHours(), preferredDate.getMinutes(), preferredDate.getSeconds(),
  ])
  if (preferredWall === null) return null
  const target = preferred + wallSecond - preferredWall
  let selected = 0
  for (let index = 1; index < candidates.length; index += 1) {
    if (Math.abs((candidates[index] ?? target) - target) < Math.abs((candidates[selected] ?? target) - target)) selected = index
  }
  return matchesCivilSecond(new Date((candidates[selected] ?? 0) * 1_000), values, false) ? selected : null
}

function utcSecond(values: CivilSecond): number | null {
  const [year, month, day, hour, minute, second] = values
  const date = new Date(0)
  date.setUTCFullYear(year, month - 1, day)
  date.setUTCHours(hour, minute, second, 0)
  const parsed = date.getTime() / 1_000
  return Number.isSafeInteger(parsed) ? parsed : null
}

function matchesCivilSecond(date: Date, values: CivilSecond, utc: boolean): boolean {
  if (!Number.isFinite(date.getTime())) return false
  return utc
    ? date.getUTCFullYear() === values[0] && date.getUTCMonth() + 1 === values[1] && date.getUTCDate() === values[2]
      && date.getUTCHours() === values[3] && date.getUTCMinutes() === values[4] && date.getUTCSeconds() === values[5]
    : date.getFullYear() === values[0] && date.getMonth() + 1 === values[1] && date.getDate() === values[2]
      && date.getHours() === values[3] && date.getMinutes() === values[4] && date.getSeconds() === values[5]
}

function two(value: number | undefined): string {
  return String(value ?? 0).padStart(2, "0")
}
