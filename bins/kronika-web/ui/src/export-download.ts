export const FALLBACK_EXPORT_FILENAME = "kronika-export.html"

export class ExportResponseError extends Error {
  readonly code: string | null

  constructor(message: string, code: string | null) {
    super(message)
    this.name = "ExportResponseError"
    this.code = code
  }
}

export class ExportServerStateUnknownError extends Error {
  constructor() {
    super("the export request ended before the server returned a status")
    this.name = "ExportServerStateUnknownError"
  }
}

export interface ExportArtifact {
  readonly blob: Blob
  readonly filename: string
}

export interface ExportTransferProgress {
  readonly received: number
  readonly total: number | null
}

type AuthenticatedFetch = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>

export function exportPath(from: number, to: number): string {
  if (!Number.isSafeInteger(from) || !Number.isSafeInteger(to)) throw new RangeError("export bounds must be whole seconds")
  const query = new URLSearchParams({ from: String(from), to: String(to) })
  return `/api/export?${query}`
}

export async function fetchExportArtifact(
  fetcher: AuthenticatedFetch,
  from: number,
  to: number,
  onProgress: (progress: ExportTransferProgress) => void,
): Promise<ExportArtifact> {
  let response: Response
  try {
    response = await fetcher(exportPath(from, to), {
      headers: { Accept: "text/html" },
    })
  } catch {
    throw new ExportServerStateUnknownError()
  }
  if (!response.ok) {
    const detail = (await response.text()).trim()
    throw new ExportResponseError(detail, responseErrorCode(detail))
  }
  const total = responseContentLength(response.headers.get("Content-Length"))
  onProgress({ received: 0, total })
  const chunks: ArrayBuffer[] = []
  let received = 0
  if (response.body === null) {
    const chunk = await response.arrayBuffer()
    chunks.push(chunk)
    received = chunk.byteLength
    onProgress({ received, total })
  } else {
    const reader = response.body.getReader()
    try {
      while (true) {
        const chunk = await reader.read()
        if (chunk.done) break
        chunks.push(copyBuffer(chunk.value))
        received += chunk.value.byteLength
        onProgress({ received, total })
      }
    } finally {
      reader.releaseLock()
    }
  }
  return {
    blob: new Blob(chunks, { type: response.headers.get("Content-Type") ?? "text/html" }),
    filename: exportFilename(response.headers.get("Content-Disposition")),
  }
}

function copyBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new ArrayBuffer(bytes.byteLength)
  new Uint8Array(copy).set(bytes)
  return copy
}

export function responseContentLength(value: string | null): number | null {
  if (value === null || !/^\d+$/.test(value)) return null
  const length = Number(value)
  return Number.isSafeInteger(length) ? length : null
}

export function exportFilename(disposition: string | null): string {
  if (disposition === null) return FALLBACK_EXPORT_FILENAME
  const candidates: string[] = []
  const encoded = /(?:^|;)\s*filename\*\s*=\s*UTF-8''([^;]*)/i.exec(disposition)?.[1]
  if (encoded !== undefined) {
    try { candidates.push(decodeURIComponent(encoded.trim())) } catch {}
  }
  const quoted = /(?:^|;)\s*filename\s*=\s*"((?:\\.|[^"])*)"/i.exec(disposition)?.[1]
  if (quoted !== undefined) candidates.push(quoted.replace(/\\(["\\])/g, "$1"))
  const plain = /(?:^|;)\s*filename\s*=\s*([^;\s]+)/i.exec(disposition)?.[1]
  if (plain !== undefined) candidates.push(plain)
  return candidates.find(safeHtmlFilename) ?? FALLBACK_EXPORT_FILENAME
}

export interface HtmlDownload {
  readonly blob: Blob
  readonly filename: string
  url: string | null
}

type ObjectUrlCreator = Pick<typeof URL, "createObjectURL">
type ObjectUrlReleaser = Pick<typeof URL, "revokeObjectURL">

// Ownership of the Blob URL stays with this handle until the caller explicitly
// leaves the "report is ready" state. WebKit resolves an a[download] navigation
// asynchronously; revoking in the same tick as click() can cancel the save on
// iOS Safari, and a long fetch already consumed the transient user activation.
// Therefore the URL is created lazily on the first real gesture, reused for a
// retry, and released on a verified lifecycle event (new export, dismissal,
// close, unmount, pagehide) — never on a guessed timeout.
export function htmlDownload(blob: Blob, filename: string): HtmlDownload {
  return { blob, filename: safeHtmlFilename(filename) ? filename : FALLBACK_EXPORT_FILENAME, url: null }
}

export function handoffHtmlDownload(
  download: HtmlDownload,
  documentRef: Pick<Document, "body" | "createElement"> = document,
  urlRef: ObjectUrlCreator = URL,
): void {
  const objectUrl = download.url ?? urlRef.createObjectURL(download.blob)
  download.url = objectUrl
  let link: HTMLAnchorElement | null = null
  try {
    link = documentRef.createElement("a")
    link.download = download.filename
    link.href = objectUrl
    link.hidden = true
    documentRef.body.append(link)
    link.click()
  } finally {
    // Only the temporary anchor is removed here. The Blob URL must outlive this
    // call; releaseHtmlDownload() owns it.
    link?.remove()
  }
}

export function releaseHtmlDownload(
  download: HtmlDownload | null,
  urlRef: ObjectUrlReleaser = URL,
): void {
  if (download === null || download.url === null) return
  const objectUrl = download.url
  download.url = null
  urlRef.revokeObjectURL(objectUrl)
}

function responseErrorCode(detail: string): string | null {
  try {
    const body: unknown = JSON.parse(detail)
    if (typeof body === "object" && body !== null && "error" in body) {
      const code = (body as { readonly error?: unknown }).error
      return typeof code === "string" ? code : null
    }
  } catch {}
  return null
}

function safeHtmlFilename(value: string): boolean {
  return value.length > 0 && value.length <= 200
    && value !== "." && value !== ".."
    && !/[\u0000-\u001f\u007f-\u009f/\\]/u.test(value)
    && value.toLowerCase().endsWith(".html")
}
