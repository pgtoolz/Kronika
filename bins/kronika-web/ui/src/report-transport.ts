interface ReportResult {
  readonly status: number
  readonly code: string | undefined
  readonly parameter: string | undefined
  readonly message: string | undefined
  takeBody(): Uint8Array<ArrayBuffer>
  free(): void
}

interface ReportSession {
  request(path: string, query: string): ReportResult
}

interface ReportRuntime {
  readonly visibleFrom?: string | undefined
  readonly visibleToExclusive?: string | undefined
  readonly ready: Promise<ReportSession>
}

export interface ReportVisibleRange {
  readonly from: number
  readonly toExclusive: number
}

const HOUR_MICROS = 3_600_000_000
const REPORT_REQUEST_BASE = "https://kronika-report.invalid/"

type ReportRangeConvention = "exclusive" | "inclusive"

const REPORT_RANGE_CONVENTIONS = new Map<string, ReportRangeConvention>([
  ["/api/events", "exclusive"],
  ["/api/heatmap", "inclusive"],
  ["/api/hour", "inclusive"],
])

function runtime(): ReportRuntime {
  const value = (globalThis as { __KRONIKA_REPORT_RUNTIME__?: ReportRuntime }).__KRONIKA_REPORT_RUNTIME__
  if (value === undefined) throw new Error("Kronika report runtime is missing")
  return value
}

export function reportVisibleRange(): ReportVisibleRange | null {
  const value = runtime()
  const from = Number(value.visibleFrom)
  const toExclusive = Number(value.visibleToExclusive)
  return Number.isSafeInteger(from) && Number.isSafeInteger(toExclusive) && from > 0 && from < toExclusive
    ? { from, toExclusive }
    : null
}

export function reportVisibleAt(at: number | null, range: ReportVisibleRange | null): number | null {
  if (at === null || range === null) return at
  return at >= range.from && at < range.toExclusive ? at : null
}

export function reportLatestHour(range: ReportVisibleRange | null): number | null {
  return range === null ? null : Math.floor((range.toExclusive - 1) / HOUR_MICROS) * HOUR_MICROS
}

export function reportVisibleCursor(cursor: number, range: ReportVisibleRange | null): number {
  if (range === null) return cursor
  return Math.min(Math.max(cursor, range.from), range.toExclusive - 1)
}

function clampReportRequestRange(url: URL): void {
  const convention = REPORT_RANGE_CONVENTIONS.get(url.pathname)
  if (convention === undefined || !url.searchParams.has("from") || !url.searchParams.has("to")) return
  const fromValues = url.searchParams.getAll("from")
  const toValues = url.searchParams.getAll("to")
  const fromText = fromValues[0] ?? ""
  const toText = toValues[0] ?? ""
  const from = Number(fromText)
  const to = Number(toText)
  if (fromValues.length !== 1 || toValues.length !== 1
    || !/^-?\d+$/.test(fromText) || !/^-?\d+$/.test(toText)
    || !Number.isSafeInteger(from) || !Number.isSafeInteger(to)) {
    throw new Error("Kronika report request range is invalid")
  }
  const visible = reportVisibleRange()
  if (visible === null) throw new Error("Kronika report visible range is invalid")
  const boundedFrom = Math.max(from, visible.from)
  const boundedTo = Math.min(to, convention === "exclusive" ? visible.toExclusive : visible.toExclusive - 1)
  if (convention === "exclusive" ? boundedFrom >= boundedTo : boundedFrom > boundedTo) {
    throw new Error("Kronika report request is outside its visible range")
  }
  url.searchParams.set("from", String(boundedFrom))
  url.searchParams.set("to", String(boundedTo))
}

export async function reportFetch(input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  const signal = init.signal
  signal?.throwIfAborted()
  const method = init.method ?? (input instanceof Request ? input.method : "GET")
  if (method !== "GET") return new Response(null, { status: 405 })
  const url = new URL(input instanceof Request ? input.url : input, REPORT_REQUEST_BASE)
  clampReportRequestRange(url)
  const session = await runtime().ready
  signal?.throwIfAborted()
  const result = session.request(url.pathname, url.search.startsWith("?") ? url.search.slice(1) : url.search)
  try {
    const status = result.status
    if (status === 200) {
      const body = result.takeBody()
      signal?.throwIfAborted()
      return new Response(body, {
        status,
        headers: { "Content-Type": "application/x-ndjson" },
      })
    }
    const body = JSON.stringify({
      error: result.code ?? "unreadable",
      ...(result.parameter === undefined ? {} : { parameter: result.parameter }),
      ...(result.message === undefined ? {} : { message: result.message }),
    })
    return new Response(body, {
      status,
      headers: { "Content-Type": "application/json" },
    })
  } finally {
    result.free()
  }
}
