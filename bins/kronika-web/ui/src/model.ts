import type { Cell, DataRow } from "./api"
import type { Translate } from "./help"

export type Locale = "en" | "ru"
export type Lens = "generic" | "cpu" | "memory" | "disk" | "tree"

export function processLens(field: string | null): Lens {
  if (["rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt"].includes(field ?? "")) return "memory"
  if (["read_bytes", "write_bytes", "cancelled_write_bytes", "syscr", "syscw", "rchar", "wchar"].includes(field ?? "")) return "disk"
  if (["utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw", "nice", "prio", "rtprio", "policy", "curcpu"].includes(field ?? "")) return "cpu"
  return "generic"
}

export function processDefaultSort(lens: Lens, rows: readonly DataRow[]): string {
  if (lens === "cpu") return "utime"
  if (lens === "memory") return "rmem_kb"
  if (lens !== "disk") return "pid"
  const fields = ["read_bytes", "write_bytes", "syscr", "syscw"] as const
  return fields.find((field) => rows.some((row) => (asNumber(value(row, field)) ?? 0) > 0))
    ?? fields.find((field) => rows.some((row) => asNumber(value(row, field)) !== null))
    ?? fields[0]
}

export function floorHour(timestamp: number): number {
  return Math.floor(timestamp / 3_600_000_000) * 3_600_000_000
}

export interface TimePair {
  readonly primary: string
  readonly secondary: string | null
}

export function nearestTime(rows: readonly DataRow[], target: number): number | null {
  let nearest: number | null = null
  let distance = Number.POSITIVE_INFINITY
  for (const row of rows) {
    const candidate = Math.abs(row.timestamp - target)
    if (candidate < distance || (candidate === distance && (nearest === null || row.timestamp < nearest))) {
      nearest = row.timestamp
      distance = candidate
    }
  }
  return nearest
}

export function snapshot(rows: readonly DataRow[], target: number): readonly DataRow[] {
  const timestamp = nearestTime(rows, target)
  return timestamp === null ? [] : rows.filter((row) => row.timestamp === timestamp)
}

export function value(row: DataRow | null, field: string): Cell {
  return row?.values[field] ?? null
}

export function asNumber(cell: Cell): number | null {
  if (typeof cell === "number") return Number.isFinite(cell) ? cell : null
  if (typeof cell === "string" && cell.trim() !== "") {
    const number = Number(cell)
    return Number.isFinite(number) ? number : null
  }
  return null
}

export function rawText(cell: Cell): string | null {
  if (typeof cell === "string") return cell
  if (typeof cell === "number" || typeof cell === "boolean") return String(cell)
  if (cell === null) return null
  if (Array.isArray(cell)) return JSON.stringify(cell)
  const payload = cell as Readonly<Record<string, unknown>>
  if (payload.representation === "text" && typeof payload.stored_text === "string") return payload.stored_text
  if (payload.representation === "bytes") {
    if (typeof payload.stored_base64 === "string") return payload.stored_base64
    if (typeof payload.base64 === "string") return payload.base64
  }
  return JSON.stringify(payload)
}

export function processCommand(row: DataRow): string {
  const command = rawText(value(row, "cmdline"))
  if (command !== null && command.trim() !== "") return command
  return rawText(value(row, "comm")) ?? "—"
}

export function processKey(row: DataRow): string {
  return identifier(value(row, "pid"))
}

export function identifier(cell: Cell): string {
  return rawText(cell) ?? "—"
}

export function stateText(cell: Cell): string {
  const number = asNumber(cell)
  return number === null ? identifier(cell) : String.fromCharCode(number)
}

export function measure(cell: Cell, locale: Locale, suffix = ""): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  return `${compact(number, locale)}${suffix}`
}

export function humanPercent(cell: Cell, locale: Locale, suffix = ""): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  const output = number > 0 && number < 0.1
    ? `<${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(0.1)}`
    : new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(number === 0 ? 0 : number)
  return `${output}${locale === "ru" ? "\u00a0" : ""}%${suffix}`
}

export function humanCores(cell: Cell, locale: Locale, suffix = ""): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 3 }).format(number === 0 ? 0 : number)}${suffix}`
}

function durationUnits(locale: Locale) {
  return locale === "ru"
    ? { "hour": "ч", microsecond: "мкс", millisecond: "мс", minute: "мин", nanosecond: "нс", "second": "с" }
    : { "hour": "h", microsecond: "µs", millisecond: "ms", minute: "min", nanosecond: "ns", "second": "s" }
}

// Elapsed wall time rounds to seconds; measured durations retain subsecond precision.
export function humanAge(seconds: number, locale: Locale): string {
  const whole = Math.max(0, Math.floor(seconds))
  if (whole < 60) return `${whole} ${durationUnits(locale).second}`
  return humanDuration(whole * 1_000, locale)
}

// The TTY column of ps: the device name behind the recorded tty_nr, `?` when
// the process has no controlling terminal.
export function processTty(cell: Cell): string {
  const encoded = asNumber(cell)
  if (encoded === null || encoded === 0) return "?"
  const major = (encoded >> 8) & 0xfff
  const minor = (encoded & 0xff) | ((encoded >> 12) & 0xfff00)
  if (major === 136) return `pts/${minor}`
  if (major === 4) return minor < 64 ? `tty${minor}` : `ttyS${minor - 64}`
  return `${major}:${minor}`
}

// The TIME column of ps: MM:SS below an hour, HH:MM:SS below a day, then DD-HH:MM:SS.
export function processCpuTime(cell: Cell): string {
  const seconds = asNumber(cell)
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return "—"
  const whole = Math.floor(seconds)
  const pad = (part: number) => String(part).padStart(2, "0")
  const days = Math.floor(whole / 86_400)
  const hours = Math.floor(whole / 3_600) % 24
  const rest = `${pad(Math.floor(whole / 60) % 60)}:${pad(whole % 60)}`
  if (days > 0) return `${days}-${pad(hours)}:${rest}`
  return hours > 0 ? `${hours}:${rest}` : `${Math.floor(whole / 60)}:${pad(whole % 60)}`
}

export type DurationInputUnit = "nanoseconds" | "microseconds" | "milliseconds" | "seconds"

export function humanDuration(cell: Cell, locale: Locale, input: DurationInputUnit = "milliseconds", suffix = ""): string {
  const stored = asNumber(cell)
  if (stored === null) return "—"
  const milliseconds = stored * (input === "nanoseconds" ? 0.000_001 : input === "microseconds" ? 0.001 : input === "seconds" ? 1_000 : 1)
  const units = durationUnits(locale)
  const magnitude = Math.abs(milliseconds)
  const [scaled, unit] = magnitude > 0 && magnitude < 0.001
    ? [milliseconds * 1_000_000, units.nanosecond]
    : magnitude > 0 && magnitude < 1
      ? [milliseconds * 1_000, units.microsecond]
      : magnitude < 1_000
        ? [milliseconds, units.millisecond]
        : magnitude < 60_000
          ? [milliseconds / 1_000, units.second]
          : magnitude < 3_600_000
            ? [milliseconds / 60_000, units.minute]
            : [milliseconds / 3_600_000, units.hour]
  return `${compact(scaled, locale)} ${unit}${suffix}`
}

// Use one unit for all axis ticks, chosen from the range maximum.
export function humanDurationAxis(milliseconds: number, rangeMax: number, locale: Locale): string {
  const magnitude = Math.abs(rangeMax)
  const input = magnitude > 0 && magnitude < 0.001
    ? "nanoseconds"
    : magnitude > 0 && magnitude < 1
      ? "microseconds"
      : magnitude < 1_000
        ? "milliseconds"
        : magnitude < 60_000
          ? "seconds"
          : magnitude < 3_600_000 ? "minutes" : "hours"
  const units = durationUnits(locale)
  const divisor = input === "nanoseconds" ? 0.000_001
    : input === "microseconds" ? 0.001
      : input === "milliseconds" ? 1
        : input === "seconds" ? 1_000
          : input === "minutes" ? 60_000 : 3_600_000
  const unit = input === "nanoseconds" ? units.nanosecond
    : input === "microseconds" ? units.microsecond
      : input === "milliseconds" ? units.millisecond
        : input === "seconds" ? units.second
          : input === "minutes" ? units.minute : units.hour
  return `${compact(milliseconds / divisor, locale)} ${unit}`
}

export function compact(value: number, locale: Locale): string {
  const abs = Math.abs(value), notation = abs >= 1e15 || abs > 0 && abs < 1e-6 ? "scientific" : abs >= 1e3 ? "compact" : undefined
  return isFinite(value) ? new Intl.NumberFormat(locale, { maximumSignificantDigits: 3, notation }).format(value) : "—"
}

export function estimatedRows(cell: Cell, locale: Locale, t: Translate): TimePair | null {
  if (!(typeof cell === "number" && Number.isSafeInteger(cell) || typeof cell === "string" && /^\d+$/.test(cell))) return null
  const value = BigInt(cell)
  if (value < 0n) return null
  const compactKind = value >= 1_000n ? "many" : rowPlural(value, locale)
  const exactKind = rowPlural(value, locale)
  const notation = value >= 1_000_000_000_000_000n ? "scientific" : value >= 1_000n ? "compact" : undefined
  const compactValue = new Intl.NumberFormat(locale, { maximumSignificantDigits: 3, notation }).format(value)
  const exactValue = new Intl.NumberFormat(locale).format(value)
  return {
    primary: t(`unit.estimated_rows.${compactKind}`, { value: compactValue }),
    secondary: t(`unit.estimated_rows.${exactKind}`, { value: exactValue }),
  }
}

function rowPlural(value: bigint, locale: Locale): "one" | "few" | "many" {
  if (locale === "en") return value === 1n ? "one" : "many"
  const last = value % 10n, lastTwo = value % 100n
  if (last === 1n && lastTwo !== 11n) return "one"
  return last >= 2n && last <= 4n && (lastTwo < 12n || lastTwo > 14n) ? "few" : "many"
}

const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"] as const

export function humanBytes(cell: Cell, locale: Locale, suffix = ""): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  const sign = number < 0 ? "-" : ""
  let scaled = Math.abs(number)
  let step = 0
  while (scaled >= 1024 && step < BYTE_UNITS.length - 1) {
    scaled /= 1024
    step += 1
  }
  const output = step === 0 && !Number.isInteger(scaled)
    ? compact(scaled, locale)
    : new Intl.NumberFormat(locale, { maximumFractionDigits: scaled >= 100 || step === 0 ? 0 : 1 }).format(scaled)
  return `${sign}${output} ${BYTE_UNITS[step]}${suffix}`
}

export function humanHertz(cell: Cell, locale: Locale): string {
  const hertz = asNumber(cell)
  if (hertz === null) return "—"
  if (Math.abs(hertz) >= 1_000_000_000) return `${compact(hertz / 1_000_000_000, locale)} GHz`
  if (Math.abs(hertz) >= 1_000_000) return `${compact(hertz / 1_000_000, locale)} MHz`
  if (Math.abs(hertz) >= 1_000) return `${compact(hertz / 1_000, locale)} kHz`
  return `${compact(hertz, locale)} Hz`
}

export function cores(cell: Cell, locale: Locale, ticksPerSecond: number | null, suffix = ""): string {
  const number = asNumber(cell)
  if (number === null || ticksPerSecond === null || ticksPerSecond <= 0) return "—"
  return humanCores(number / ticksPerSecond, locale, suffix)
}

export function millisecondsPerSecond(cell: Cell, locale: Locale): string {
  const number = asNumber(cell)
  if (number === null) return "—"
  return compact(number / 1_000_000, locale)
}

export function activityFor(
  process: DataRow | null,
  activities: readonly DataRow[],
  cursor: number,
): { readonly row: DataRow | null; readonly snapshotTime: number | null } {
  if (process === null) return { row: null, snapshotTime: null }
  const pid = asNumber(value(process, "pid"))
  if (pid === null) return { row: null, snapshotTime: null }
  // Filter by PID before choosing the nearest per-database Activity timestamp.
  const matching = activities.filter((activity) => asNumber(value(activity, "pid")) === pid)
  const activitySnapshot = snapshot(matching, cursor)
  const snapshotTime = activitySnapshot[0]?.timestamp ?? null
  const row = activitySnapshot[0] ?? null
  return { row, snapshotTime }
}

export function interpolate(template: string, slots: Readonly<Record<string, string | number>>): string {
  return template.replace(/\{([a-z][a-z0-9_]*)\}/gi, (_whole, name: string) => String(slots[name] ?? `{${name}}`))
}
