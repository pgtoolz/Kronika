/**
 * A visible support contract for generated offline reports.
 *
 * The report runtime is injected before the UI bundle inside the same inline
 * script. It uses gzip streams and WebAssembly as soon as that script runs, so
 * this module checks those capabilities, attaches a rejection handler to the
 * runtime promise and replaces a silent failure with a localized message.
 *
 * Feature detection lives only behind the report build gate. It deliberately
 * does not inspect the user agent, polyfill old WebKit versions, or claim that
 * a browser which cannot parse the ES2022 bundle can execute this diagnostic.
 */
export type SupportLocale = "en" | "ru"

export type SupportTranslate = (locale: SupportLocale, key: string) => string | undefined
export type SupportGetter = (key: string) => string | null

interface ReportFeature {
  readonly label: string
  readonly available: () => boolean
}

interface ReportRuntime {
  readonly ready?: Promise<unknown>
}

const REQUIRED_REPORT_FEATURES: readonly ReportFeature[] = [
  {
    label: 'DecompressionStream("gzip") + ReadableStream.pipeThrough()',
    available: () => typeof DecompressionStream === "function"
      && typeof ReadableStream === "function"
      && typeof ReadableStream.prototype?.pipeThrough === "function",
  },
  {
    label: "Blob.stream()",
    available: () => typeof Blob === "function" && typeof Blob.prototype.stream === "function",
  },
  {
    label: "Response.arrayBuffer()",
    available: () => typeof Response === "function" && typeof Response.prototype.arrayBuffer === "function",
  },
  {
    label: "WebAssembly.compile()",
    available: () => typeof WebAssembly === "object" && typeof WebAssembly.compile === "function",
  },
  {
    label: "BigInt()",
    available: () => typeof BigInt === "function",
  },
  {
    label: "atob()",
    available: () => typeof atob === "function",
  },
]

export function missingReportFeatures(): string[] {
  return REQUIRED_REPORT_FEATURES
    .filter((feature) => {
      try {
        return !feature.available()
      } catch {
        return true
      }
    })
    .map((feature) => feature.label)
}

export function reportSupportLocale(getItem: SupportGetter): SupportLocale {
  try {
    const saved = getItem("kronika.locale")
    if (saved === "ru" || saved === "en") return saved
  } catch {}
  if (typeof navigator === "undefined") return "en"
  const languages = navigator.languages !== undefined && navigator.languages.length > 0
    ? navigator.languages
    : typeof navigator.language === "string" ? [navigator.language] : []
  for (const language of languages) {
    const tag = language.toLowerCase()
    if (tag.startsWith("ru")) return "ru"
    if (tag.startsWith("en")) return "en"
  }
  return "en"
}

function showReportSupportError(
  translate: SupportTranslate,
  locale: SupportLocale,
  missing: readonly string[],
): void {
  if (typeof document === "undefined" || document.body === null || document.body === undefined) return
  if (document.querySelector('[data-testid="report-support-error"]') !== null) return

  const message = (key: string): string => {
    try {
      return translate(locale, key) ?? key
    } catch {
      return key
    }
  }
  const text = (tag: string, value: string, css: string): HTMLElement => {
    const node = document.createElement(tag)
    node.style.cssText = css
    node.textContent = value
    return node
  }

  document.documentElement.lang = locale
  const root = document.getElementById("root")
  if (root !== null) root.setAttribute("aria-hidden", "true")

  const overlay = document.createElement("div")
  overlay.setAttribute("data-testid", "report-support-error")
  overlay.setAttribute("role", "alert")
  overlay.setAttribute("aria-live", "assertive")
  overlay.lang = locale
  overlay.style.cssText = "position:fixed;top:0;right:0;bottom:0;left:0;z-index:2147483647;box-sizing:border-box;overflow:auto;display:flex;align-items:center;justify-content:center;padding:24px;background:#02060c;color:#f1f5f9;font:16px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif"

  const card = document.createElement("div")
  card.style.cssText = "box-sizing:border-box;width:100%;max-width:640px;padding:24px;border:1px solid #334155;border-radius:12px;background:#111820;box-shadow:0 16px 48px rgba(0,0,0,.45)"
  card.appendChild(text("h1", message("support.offline.title"), "margin:0 0 12px;font-size:22px;line-height:1.3;font-weight:600"))
  card.appendChild(text("p", message("support.offline.body"), "margin:0 0 12px"))
  card.appendChild(text("p", message("support.offline.update"), "margin:0 0 12px"))
  const detail = missing.length > 0
    ? message("support.offline.missing").replace("{features}", missing.join(", "))
    : message("support.offline.failed")
  card.appendChild(text("p", detail, "margin:0;color:#cbd5e1;font-size:14px;overflow-wrap:anywhere"))
  overlay.appendChild(card)
  document.body.appendChild(overlay)
}

/**
 * Returns true when React must not render because the report cannot start.
 * The runtime rejection handler stays attached on the supported path too, so
 * a later WASM/compile failure also becomes a visible error, not a blank root.
 */
export function installReportSupport(translate: SupportTranslate, getItem: SupportGetter): boolean {
  if (typeof document === "undefined") return false
  const locale = reportSupportLocale(getItem)
  const missing = missingReportFeatures()
  const runtime = (globalThis as { __KRONIKA_REPORT_RUNTIME__?: ReportRuntime }).__KRONIKA_REPORT_RUNTIME__
  const ready = runtime?.ready
  const fail = () => {
    try {
      showReportSupportError(translate, locale, missingReportFeatures())
    } catch {}
  }

  if (typeof ready?.catch !== "function") {
    fail()
    return true
  }
  const promise = ready as Promise<unknown>
  void promise.catch(() => fail())
  if (missing.length > 0) {
    fail()
    return true
  }
  return false
}
