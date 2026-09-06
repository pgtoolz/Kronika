import assert from "node:assert/strict"
import test from "node:test"

type SessionModule = typeof import("../src/session.ts")
type FetchCall = { readonly input: RequestInfo | URL, readonly init?: RequestInit }

test("import is inert and bootstrap checks the cookie before signing in", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    return new Response(null, { status: 204 })
  }

  try {
    const session = await isolatedSession("bootstrap-valid")
    assert.equal(session.getSessionSnapshot(), "pending")
    assert.equal(calls.length, 0)

    await session.bootstrapSession()
    assert.equal(session.getSessionSnapshot(), "signed-in")
    assert.equal(calls.length, 1)
    assert.equal(calls[0]?.input, "/auth/session")
    assert.equal(calls[0]?.init?.credentials, "same-origin")
    assert.equal(calls[0]?.init?.method, "GET")
    const headers = new Headers(calls[0]?.init?.headers)
    assert.equal(headers.get("X-Kronika-UI"), "1")
    assert.equal(headers.get("Authorization"), null)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("missing bootstrap and rejected Basic leave authentication absent", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    return new Response(null, { status: 401 })
  }

  try {
    const session = await isolatedSession("invalid")
    await session.bootstrapSession()
    assert.equal(session.getSessionSnapshot(), "signed-out")
    assert.equal(await session.signInBasic("user", "wrong", new AbortController().signal), "invalid")
    assert.equal(session.getSessionSnapshot(), "signed-out")
    await assert.rejects(session.apiFetch("/api/hour"))
    assert.deepEqual(calls.map(({ input }) => input), ["/auth/session", "/auth/session", "/auth/session"])
    assert.deepEqual(calls.map(({ init }) => init?.method), ["GET", "DELETE", "POST"])
    assert.equal(new Headers(calls[2]?.init?.headers).get("Authorization"), utf8Basic("user", "wrong"))
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a missing bootstrap waits for one cleanup before exposing login", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  let finishDelete: ((value: Response) => void) | undefined
  const deleted = new Promise<Response>((resolve) => { finishDelete = resolve })
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    if (init?.method === "DELETE") return deleted
    return new Response(null, { status: 401 })
  }

  try {
    const session = await isolatedSession("bootstrap-missing")
    const bootstrap = session.bootstrapSession()
    await waitUntil(() => calls.some(({ init }) => init?.method === "DELETE"))
    assert.equal(session.getSessionSnapshot(), "pending")
    assert.deepEqual(calls.map(({ init }) => init?.method), ["GET", "DELETE"])
    finishDelete?.(new Response(null, { status: 204 }))
    await bootstrap
    assert.equal(session.getSessionSnapshot(), "signed-out")
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a delayed bootstrap response cannot overwrite a newer login", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  let finishBootstrap: ((value: Response) => void) | undefined
  const bootstrapResponse = new Promise<Response>((resolve) => { finishBootstrap = resolve })
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    if (init?.method === "GET") return bootstrapResponse
    return new Response(null, { status: 204 })
  }

  try {
    const session = await isolatedSession("stale-bootstrap")
    const bootstrap = session.bootstrapSession()
    await waitUntil(() => calls.length === 1)
    assert.equal(await session.signInBasic("new", "credential", new AbortController().signal), "signed-in")
    finishBootstrap?.(new Response(null, { status: 401 }))
    await bootstrap
    assert.equal(session.getSessionSnapshot(), "signed-in")
    assert.deepEqual(calls.map(({ init }) => init?.method), ["GET", "POST"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("Basic is sent once on login and API requests use only the cookie boundary", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    return new Response(null, { status: 204 })
  }

  try {
    const session = await isolatedSession("boundary")
    await session.bootstrapSession()
    const signal = new AbortController().signal
    assert.equal(await session.signInBasic("Jörg", "пароль", signal), "signed-in")

    const request = new Request("http://localhost/api/hour", {
      headers: { Authorization: "Basic request-secret", "X-Request": "kept" },
    })
    await session.apiFetch(request, {
      credentials: "include",
      headers: { Authorization: "Basic init-secret", "X-Init": "kept" },
    })

    assert.equal(calls.length, 3)
    const login = calls[1]
    assert.equal(login?.input, "/auth/session")
    assert.equal(login?.init?.method, "POST")
    assert.equal(login?.init?.credentials, "same-origin")
    assert.equal(login?.init?.signal, signal)
    assert.equal(new Headers(login?.init?.headers).get("Authorization"), utf8Basic("Jörg", "пароль"))
    assert.equal(new Headers(login?.init?.headers).get("X-Kronika-UI"), "1")

    const api = calls[2]
    assert.equal(api?.input, request)
    assert.equal(api?.init?.credentials, "same-origin")
    const headers = new Headers(api?.init?.headers)
    assert.equal(headers.get("Authorization"), null)
    assert.equal(headers.get("X-Kronika-UI"), "1")
    assert.equal(headers.get("X-Request"), "kept")
    assert.equal(headers.get("X-Init"), "kept")
    assert.equal(calls.filter(({ init }) => new Headers(init?.headers).has("Authorization")).length, 1)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("logout clears once before exposing the signed-out state", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  let finishDelete: ((value: Response) => void) | undefined
  const deleted = new Promise<Response>((resolve) => { finishDelete = resolve })
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    if (init?.method === "DELETE") return deleted
    return new Response(null, { status: 204 })
  }

  try {
    const session = await isolatedSession("logout")
    await session.bootstrapSession()
    const snapshots: string[] = []
    const unsubscribe = session.subscribeSession(() => snapshots.push(session.getSessionSnapshot()))
    const logout = session.logout()
    assert.equal(session.getSessionSnapshot(), "pending")
    assert.equal(calls.filter(({ init }) => init?.method === "DELETE").length, 1)
    const deleteCall = calls.find(({ init }) => init?.method === "DELETE")
    assert.equal(deleteCall?.input, "/auth/session")
    assert.equal(deleteCall?.init?.credentials, "same-origin")
    assert.equal(new Headers(deleteCall?.init?.headers).get("Authorization"), null)
    assert.equal(new Headers(deleteCall?.init?.headers).get("X-Kronika-UI"), "1")
    finishDelete?.(new Response(null, { status: 204 }))
    await logout
    assert.equal(session.getSessionSnapshot(), "signed-out")
    assert.deepEqual(snapshots, ["pending", "signed-out"])
    unsubscribe()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("concurrent current 401 responses share one cleanup and one expiry transition", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  let finishDelete: ((value: Response) => void) | undefined
  const deleted = new Promise<Response>((resolve) => { finishDelete = resolve })
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    if (init?.method === "DELETE") return deleted
    if (input === "/auth/session") return new Response(null, { status: 204 })
    return new Response(null, { status: 401 })
  }

  try {
    const session = await isolatedSession("concurrent-expiry")
    await session.bootstrapSession()
    const snapshots: string[] = []
    const unsubscribe = session.subscribeSession(() => snapshots.push(session.getSessionSnapshot()))
    const first = session.apiFetch("/api/first")
    const second = session.apiFetch("/api/second")
    await waitUntil(() => session.getSessionSnapshot() === "pending")
    assert.equal(calls.filter(({ init }) => init?.method === "DELETE").length, 1)
    finishDelete?.(new Response(null, { status: 204 }))
    assert.deepEqual((await Promise.all([first, second])).map(({ status }) => status), [401, 401])
    await waitUntil(() => session.getSessionSnapshot() === "expired")
    assert.deepEqual(snapshots, ["pending", "expired"])
    assert.equal(calls.filter(({ init }) => init?.method === "DELETE").length, 1)
    unsubscribe()
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a delayed old 401 after cleanup and relogin cannot clear the replacement", async () => {
  const originalFetch = globalThis.fetch
  const calls: FetchCall[] = []
  let resolveOld: ((response: Response) => void) | undefined
  const oldResponse = new Promise<Response>((resolve) => { resolveOld = resolve })
  globalThis.fetch = async (input, init) => {
    calls.push({ input, init })
    if (input === "/api/old") return oldResponse
    if (input === "/api/expire") return new Response(null, { status: 401 })
    return new Response(null, { status: 204 })
  }

  try {
    const session = await isolatedSession("stale")
    await session.bootstrapSession()
    const delayed = session.apiFetch("/api/old")
    assert.equal((await session.apiFetch("/api/expire")).status, 401)
    await waitUntil(() => session.getSessionSnapshot() === "expired")
    assert.equal(await session.signInBasic("replacement", "credential", new AbortController().signal), "signed-in")
    resolveOld?.(new Response(null, { status: 401 }))
    assert.equal((await delayed).status, 401)
    assert.equal(session.getSessionSnapshot(), "signed-in")
    assert.equal(calls.filter(({ init }) => init?.method === "DELETE").length, 1)
    await session.apiFetch("/api/current")
    assert.equal(session.getSessionSnapshot(), "signed-in")
  } finally {
    globalThis.fetch = originalFetch
  }
})

async function isolatedSession(name: string): Promise<SessionModule> {
  return import(`../src/session.ts?test=${name}`)
}

function utf8Basic(user: string, password: string): string {
  return `Basic ${Buffer.from(`${user}:${password}`, "utf8").toString("base64")}`
}

async function waitUntil(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
  throw new Error("condition was not reached")
}
