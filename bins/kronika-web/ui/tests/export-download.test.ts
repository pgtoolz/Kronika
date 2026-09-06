import assert from "node:assert/strict"
import test from "node:test"

import {
  ExportResponseError,
  ExportServerStateUnknownError,
  FALLBACK_EXPORT_FILENAME,
  exportFilename,
  exportPath,
  fetchExportArtifact,
  responseContentLength,
  triggerHtmlDownload,
} from "../src/export-download.ts"

test("the export request streams exact bytes without a client cancellation signal", async () => {
  const calls: { input?: RequestInfo | URL, init?: RequestInit }[] = []
  const progress: { received: number; total: number | null }[] = []
  const expected = "kronika-1969-12-31-235959-1970-01-01-000000-utc.html"
  const chunks = ["<!doctype ", "html><title>Kronika</title>"]
  const body = chunks.join("")
  const fetcher = async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ input, init })
    return streamedResponse(chunks, {
      "Content-Disposition": `attachment; filename="${expected}"`,
      "Content-Length": String(Buffer.byteLength(body)),
      "Content-Type": "text/html",
    })
  }
  const artifact = await fetchExportArtifact(fetcher, -1, 0, (value) => progress.push(value))
  assert.equal(exportPath(-1, 0), "/api/export?from=-1&to=0")
  assert.equal(calls[0]?.input, "/api/export?from=-1&to=0")
  assert.equal(new Headers(calls[0]?.init?.headers).get("Accept"), "text/html")
  assert.equal(calls[0]?.init?.signal, undefined)
  assert.equal(artifact.filename, expected)
  assert.equal(artifact.blob.type, "text/html")
  assert.equal(await artifact.blob.text(), body)
  assert.deepEqual(progress, [
    { received: 0, total: Buffer.byteLength(body) },
    { received: Buffer.byteLength(chunks[0]!), total: Buffer.byteLength(body) },
    { received: Buffer.byteLength(body), total: Buffer.byteLength(body) },
  ])
  assert.throws(() => exportPath(0.5, 1), /whole seconds/)
})

test("unknown Content-Length reports only received bytes and bodyless responses still assemble", async () => {
  for (const header of [null, "", "-1", "1.5", "9007199254740992"]) assert.equal(responseContentLength(header), null)
  assert.equal(responseContentLength("0"), 0)
  assert.equal(responseContentLength("123"), 123)

  const response = new Response("report", { headers: { "Content-Length": "not-a-number", "Content-Type": "text/html" } })
  Object.defineProperty(response, "body", { value: null })
  const progress: { received: number; total: number | null }[] = []
  const artifact = await fetchExportArtifact(async () => response, 1, 2, (value) => progress.push(value))
  assert.equal(await artifact.blob.text(), "report")
  assert.deepEqual(progress, [{ received: 0, total: null }, { received: 6, total: null }])
})

test("a failed body stream never produces an artifact", async () => {
  const encoder = new TextEncoder()
  const response = new Response(new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(encoder.encode("partial"))
      controller.error(new Error("connection lost"))
    },
  }), { headers: { "Content-Length": "100", "Content-Type": "text/html" } })
  const progress: { received: number; total: number | null }[] = []
  await assert.rejects(fetchExportArtifact(async () => response, 1, 2, (value) => progress.push(value)), /connection lost/)
  assert.equal(progress[0]?.received, 0)
  assert.ok(progress.every(({ received }, index) => received >= (progress[index - 1]?.received ?? 0)))
})

test("non-OK response text is read and its typed code is retained", async () => {
  const response = new Response('{"error":"export_empty"}', { status: 404 })
  await assert.rejects(
    fetchExportArtifact(async () => response, 1, 2, () => {}),
    (reason: unknown) => reason instanceof ExportResponseError
      && reason.message === '{"error":"export_empty"}'
      && reason.code === "export_empty",
  )
})

test("a failure before response status leaves server admission unknown", async () => {
  await assert.rejects(
    fetchExportArtifact(async () => { throw new TypeError("connection reset") }, 1, 2, () => {}),
    (reason: unknown) => reason instanceof ExportServerStateUnknownError,
  )
})

test("Content-Disposition chooses a safe HTML filename", () => {
  assert.equal(exportFilename('attachment; filename="kronika-range-utc.html"'), "kronika-range-utc.html")
  assert.equal(exportFilename("attachment; filename*=UTF-8''kronika-%D1%82%D0%B5%D1%81%D1%82.html"), "kronika-тест.html")
  for (const disposition of [
    null,
    'attachment; filename="../report.html"',
    'attachment; filename="report.txt"',
    "attachment; filename*=UTF-8''..%2Freport.html",
  ]) assert.equal(exportFilename(disposition), FALLBACK_EXPORT_FILENAME)
})

test("one download preserves the filename and always revokes its object URL", () => {
  const actions: string[] = []
  const link = {
    download: "",
    hidden: false,
    href: "",
    click: () => actions.push("click"),
    remove: () => actions.push("remove"),
  }
  const documentRef = {
    body: { append: (node: unknown) => { assert.equal(node, link); actions.push("append") } },
    createElement: (name: string) => { assert.equal(name, "a"); return link },
  }
  const urlRef = {
    createObjectURL: (blob: Blob) => { assert.equal(blob.type, "text/html"); actions.push("create"); return "blob:kronika" },
    revokeObjectURL: (url: string) => { assert.equal(url, "blob:kronika"); actions.push("revoke") },
  }
  triggerHtmlDownload(new Blob(["<!doctype html>"], { type: "text/html" }), "kronika-range-utc.html", documentRef as never, urlRef)
  assert.equal(link.download, "kronika-range-utc.html")
  assert.equal(link.href, "blob:kronika")
  assert.equal(link.hidden, true)
  assert.deepEqual(actions, ["create", "append", "click", "remove", "revoke"])
})

test("object URLs are revoked when the synthetic click fails", () => {
  const revoked: string[] = []
  const link = { download: "", hidden: false, href: "", click: () => { throw new Error("blocked") }, remove: () => {} }
  assert.throws(() => triggerHtmlDownload(
    new Blob(),
    "kronika.html",
    { body: { append: () => {} }, createElement: () => link } as never,
    { createObjectURL: () => "blob:kronika", revokeObjectURL: (url) => revoked.push(url) },
  ), /blocked/)
  assert.deepEqual(revoked, ["blob:kronika"])
})

function streamedResponse(chunks: readonly string[], headers: HeadersInit): Response {
  const encoder = new TextEncoder()
  return new Response(new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk))
      controller.close()
    },
  }), { headers })
}
