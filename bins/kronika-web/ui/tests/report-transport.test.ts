import assert from "node:assert/strict"
import test from "node:test"

import {
  reportFetch,
  reportLatestHour,
  reportVisibleAt,
  reportVisibleCursor,
  reportVisibleRange,
} from "../src/report-transport.ts"

type Result = {
  readonly status: number
  readonly code: string | undefined
  readonly parameter: string | undefined
  readonly message: string | undefined
  takeBody(): Uint8Array<ArrayBuffer>
  free(): void
}

type Session = {
  request(path: string, query: string): Result
}

function installRuntime(
  session: Session,
  visibleFrom?: string,
  visibleToExclusive?: string,
  locationHref = "file:///tmp/kronika-report.html",
): () => void {
  const location = Object.getOwnPropertyDescriptor(globalThis, "location")
  const runtime = Object.getOwnPropertyDescriptor(globalThis, "__KRONIKA_REPORT_RUNTIME__")
  Object.defineProperty(globalThis, "location", {
    configurable: true,
    value: { href: locationHref },
  })
  Object.defineProperty(globalThis, "__KRONIKA_REPORT_RUNTIME__", {
    configurable: true,
    value: { ready: Promise.resolve(session), visibleFrom, visibleToExclusive },
  })
  return () => {
    if (location === undefined) Reflect.deleteProperty(globalThis, "location")
    else Object.defineProperty(globalThis, "location", location)
    if (runtime === undefined) Reflect.deleteProperty(globalThis, "__KRONIKA_REPORT_RUNTIME__")
    else Object.defineProperty(globalThis, "__KRONIKA_REPORT_RUNTIME__", runtime)
  }
}

test("report navigation accepts only exact instants inside the embedded visible range", () => {
  const session = { request() { throw new Error("unused") } }
  const restore = installRuntime(session, "1788523200000000", "1788526800000000")
  try {
    const range = reportVisibleRange()
    assert.deepEqual(range, { from: 1788523200000000, toExclusive: 1788526800000000 })
    assert.equal(reportVisibleAt(1788523200000000, range), 1788523200000000)
    assert.equal(reportVisibleAt(1788526799999999, range), 1788526799999999)
    assert.equal(reportVisibleAt(1788526800000000, range), null)
    assert.equal(reportVisibleAt(1788530400000000, range), null)
    assert.equal(reportLatestHour(range), 1788523200000000)
    assert.equal(reportVisibleCursor(1788523199999999, range), 1788523200000000)
    assert.equal(reportVisibleCursor(1788526800000000, range), 1788526799999999)
  } finally {
    restore()
  }
})

test("report navigation rejects nonpositive and unsafe embedded bounds", () => {
  const session = { request() { throw new Error("unused") } }
  for (const [from, toExclusive] of [
    ["0", "1"],
    ["-1", "1"],
    ["9007199254740991", "9007199254740992"],
  ]) {
    const restore = installRuntime(session, from, toExclusive)
    try {
      assert.equal(reportVisibleRange(), null)
    } finally {
      restore()
    }
  }
})

test("report transport returns unchanged NDJSON and releases the response", async () => {
  const expected = new TextEncoder().encode('{"record":"hour"}\n')
  let freed = 0
  const restore = installRuntime({
    request(path, query) {
      assert.equal(path, "/api/hour")
      assert.equal(query, "part=base&segments=42")
      return {
        status: 200,
        code: undefined,
        parameter: undefined,
        message: undefined,
        takeBody: () => expected,
        free: () => { freed += 1 },
      }
    },
  })

  try {
    const response = await reportFetch("/api/hour?part=base&segments=42")
    assert.equal(response.status, 200)
    assert.equal(response.headers.get("Content-Type"), "application/x-ndjson")
    assert.deepEqual(new Uint8Array(await response.arrayBuffer()), expected)
    assert.equal(freed, 1)
  } finally {
    restore()
  }
})

test("report transport keeps the API root across file and hosted report URLs", async () => {
  for (const locationHref of [
    "file:///tmp/kronika-report.html",
    "file:///C:/Users/vadvm/Downloads/Telegram%20Desktop/kronika-report.html",
    "https://pgtoolz.github.io/Kronika/",
  ]) {
    const seen: string[] = []
    const restore = installRuntime({
      request(path, query) {
        seen.push(`${path}?${query}`)
        return {
          status: 200,
          code: undefined,
          parameter: undefined,
          message: undefined,
          takeBody: () => new Uint8Array(),
          free() {},
        }
      },
    }, undefined, undefined, locationHref)

    try {
      const response = await reportFetch("/api/hour?part=base&segments=42")
      assert.equal(response.status, 200)
      assert.deepEqual(seen, ["/api/hour?part=base&segments=42"])
    } finally {
      restore()
    }
  }
})

test("report transport keeps every product range inside the embedded visible range", async () => {
  const seen: string[] = []
  const restore = installRuntime({
    request(path, query) {
      seen.push(`${path}?${query}`)
      return {
        status: 200,
        code: undefined,
        parameter: undefined,
        message: undefined,
        takeBody: () => new Uint8Array(),
        free() {},
      }
    },
  }, "1788523200000000", "1788526800000000")

  try {
    await reportFetch("/api/events?from=1788523199999999&to=1788526800000001&representation=groups&limit=5000")
    await reportFetch("/api/hour?from=1788523199999999&to=1788526800000001&section=os_process&field=pid")
    await reportFetch("/api/heatmap?from=1788523199999999&to=1788526800000001&section=os_process&field=cpu_ticks&columns=60&top=25")
    assert.deepEqual(seen, [
      "/api/events?from=1788523200000000&to=1788526800000000&representation=groups&limit=5000",
      "/api/hour?from=1788523200000000&to=1788526799999999&section=os_process&field=pid",
      "/api/heatmap?from=1788523200000000&to=1788526799999999&section=os_process&field=cpu_ticks&columns=60&top=25",
    ])
  } finally {
    restore()
  }
})

test("report transport does not query retained context outside its visible range", async () => {
  let requests = 0
  const restore = installRuntime({
    request() {
      requests += 1
      throw new Error("unreachable")
    },
  }, "1788523200000000", "1788526800000000")

  try {
    await assert.rejects(
      reportFetch("/api/events?from=1788523190000000&to=1788523200000000&representation=groups&limit=5000"),
      /outside its visible range/,
    )
    await assert.rejects(
      reportFetch("/api/hour?from=1788526800000000&to=1788526809999999&section=os_process&field=pid"),
      /outside its visible range/,
    )
    await assert.rejects(
      reportFetch("/api/heatmap?from=1788526800000000&to=1788526809999999&section=os_process&field=cpu_ticks"),
      /outside its visible range/,
    )
    assert.equal(requests, 0)
  } finally {
    restore()
  }
})

test("report transport preserves stable refusal fields", async () => {
  let freed = false
  const restore = installRuntime({
    request() {
      return {
        status: 400,
        code: "bad_parameter",
        parameter: "from",
        message: "invalid parameter from",
        takeBody: () => new Uint8Array(),
        free: () => { freed = true },
      }
    },
  })

  try {
    const response = await reportFetch("/api/hour?from=bad")
    assert.equal(response.status, 400)
    assert.equal(response.headers.get("Content-Type"), "application/json")
    assert.deepEqual(await response.json(), {
      error: "bad_parameter",
      parameter: "from",
      message: "invalid parameter from",
    })
    assert.equal(freed, true)
  } finally {
    restore()
  }
})

test("report transport refuses methods without calling the engine", async () => {
  const response = await reportFetch("/api/hour", { method: "POST" })
  assert.equal(response.status, 405)
})

test("report transport observes an abort after the synchronous query returns", async () => {
  const abort = new AbortController()
  let freed = false
  const restore = installRuntime({
    request() {
      return {
        status: 200,
        code: undefined,
        parameter: undefined,
        message: undefined,
        takeBody() {
          abort.abort()
          return new Uint8Array([1])
        },
        free: () => { freed = true },
      }
    },
  })

  try {
    await assert.rejects(reportFetch("/api/catalog", { signal: abort.signal }), { name: "AbortError" })
    assert.equal(freed, true)
  } finally {
    restore()
  }
})
