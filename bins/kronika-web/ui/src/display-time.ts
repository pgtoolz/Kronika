import type { Locale } from "./model"

export type DisplayTimeZone = "browser" | "utc"

export const DISPLAY_TIME_ZONE_KEY = "kronika.timezone"
const HOUR_US = 3_600_000_000

export interface DisplayTimeFormatter {
  readonly mode: DisplayTimeZone
  axis(timestamp: number): string
  clock(timestamp: number): string
  date(timestamp: number): string
  dayKey(timestamp: number): string
  hourLabel(timestamp: number): string
  hourRange(timestamp: number): { readonly date: string; readonly primary: string }
  monthKey(timestamp: number): string
  range(from: number, to: number, context?: number): { readonly from: string; readonly to: string }
  startTime(timestamp: number | null, context?: number): string
  timestamp(timestamp: number | null, context?: number): string
}

interface PreferenceStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

export function displayTimeZone(value: unknown): DisplayTimeZone {
  return value === "utc" ? "utc" : "browser"
}

export function loadDisplayTimeZone(storage: Pick<PreferenceStorage, "getItem">): DisplayTimeZone {
  try { return displayTimeZone(storage.getItem(DISPLAY_TIME_ZONE_KEY)) } catch { return "browser" }
}

export function saveDisplayTimeZone(storage: Pick<PreferenceStorage, "setItem">, value: DisplayTimeZone): void {
  try { storage.setItem(DISPLAY_TIME_ZONE_KEY, value) } catch {}
}

export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC"
}

export function createDisplayTimeFormatter(locale: Locale, mode: DisplayTimeZone, localZone = browserTimeZone()): DisplayTimeFormatter {
  const timeZone = mode === "utc" ? "UTC" : localZone
  const clockFormat = Intl.DateTimeFormat("en-US-u-ca-gregory-nu-latn", { day: "2-digit", hour: "2-digit", hourCycle: "h23", minute: "2-digit", month: "2-digit", second: "2-digit", timeZone, year: "numeric" })
  const parts = (timestamp: number) => Object.fromEntries(clockFormat.formatToParts(moment(timestamp)).map(({ type, value }) => [type, value]))
  const date = (timestamp: number) => civilLabel(parts(timestamp), locale)
  const time = (timestamp: number, seconds = false) => {
    const value = parts(timestamp)
    return `${value.hour}:${value.minute}${seconds ? `:${value.second}` : ""}`
  }
  const dayKey = (timestamp: number) => civilLabel(parts(timestamp))
  const timestamp = (value: number | null, context?: number) => value === null || !Number.isFinite(value)
    ? "—"
    : context !== undefined && dayKey(value) === dayKey(context) ? time(value, true) : `${date(value)} · ${time(value, true)}`
  // ps prints the clock for a process started within the shown day and the
  // civil date for anything older.
  const startTime = (value: number | null, context?: number) => value === null || !Number.isFinite(value)
    ? "—"
    : context !== undefined && dayKey(value) === dayKey(context) ? time(value) : date(value)
  const range = (from: number, to: number, context?: number) => {
    const compact = context !== undefined && dayKey(from) === dayKey(context) && dayKey(to) === dayKey(context) && dayKey(from) === dayKey(to)
    return compact
      ? { from: time(from, true), to: time(to, true) }
      : { from: timestamp(from), to: timestamp(to) }
  }
  return {
    mode,
    axis: time,
    clock: (timestamp) => time(timestamp, true),
    date,
    dayKey,
    hourLabel: time,
    hourRange: (timestamp) => {
      const end = timestamp + HOUR_US
      const leftDate = date(timestamp), rightDate = date(end), left = time(timestamp), right = time(end)
      return {
        date: leftDate === rightDate ? leftDate : `${leftDate}–${rightDate}`,
        primary: `${left}–${right}`,
      }
    },
    monthKey: (timestamp) => dayKey(timestamp).slice(0, 7),
    range,
    startTime,
    timestamp,
  }
}

export function calendarDateLabel(key: string, locale: Locale): string {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(key)
  return match === null ? key : civilLabel({ day: match[3]!, month: match[2]!, year: match[1]! }, locale)
}

export function calendarMonthLabel(key: string, locale: Locale): string {
  const date = civilDate(`${key}-01`, 12)
  return date === null ? key : Intl.DateTimeFormat(locale, { month: "long", timeZone: "UTC", year: "numeric" }).format(date)
}

export function calendarMonthDays(key: string): readonly string[] {
  const match = /^(\d{4})-(\d{2})$/.exec(key)
  if (match === null) return []
  const first = civilDate(`${key}-01`, 12)
  if (first === null) return []
  const next = new Date(first)
  next.setUTCMonth(next.getUTCMonth() + 1)
  const count = Math.round((next.getTime() - first.getTime()) / 86_400_000)
  return Array.from({ length: count }, (_, index) => `${match[1]}-${match[2]}-${String(index + 1).padStart(2, "0")}`)
}

function moment(timestamp: number): Date {
  return new Date(Math.trunc(timestamp / 1_000))
}

function civilLabel(parts: Readonly<Record<string, string>>, locale?: Locale): string {
  return locale === undefined ? `${parts.year}-${parts.month}-${parts.day}` : locale === "ru" ? `${parts.day}.${parts.month}.${parts.year}` : `${parts.month}/${parts.day}/${parts.year}`
}

function civilDate(key: string, hour: number): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(key)
  if (match === null) return null
  const year = Number(match[1]), month = Number(match[2]), day = Number(match[3])
  const date = new Date(0)
  date.setUTCFullYear(year, month - 1, day)
  date.setUTCHours(hour, 0, 0, 0)
  return date.getUTCFullYear() === year && date.getUTCMonth() + 1 === month && date.getUTCDate() === day ? date : null
}
