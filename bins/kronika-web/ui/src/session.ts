export type SessionSnapshot = "pending" | "signed-out" | "signed-in" | "expired"

type SessionListener = () => void
type SignedOutSnapshot = "signed-out" | "expired"

const listeners = new Set<SessionListener>()

let currentSnapshot: SessionSnapshot = "pending"
let generation = 0
let cleanupPromise: Promise<void> | null = null

export function getSessionSnapshot(): SessionSnapshot {
  return currentSnapshot
}

export function subscribeSession(listener: SessionListener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export async function bootstrapSession(): Promise<void> {
  const captured = ++generation
  const response = await sessionFetch("GET").catch(() => null)
  if (generation !== captured) return
  if (response?.status === 401) return clearSession("signed-out")
  transition(response?.status === 204 ? "signed-in" : "signed-out")
}

export function signInBasic(
  user: string,
  password: string,
  signal: AbortSignal,
): Promise<"signed-in" | "invalid"> {
  const submit = async (): Promise<"signed-in" | "invalid"> => {
    ++generation
    const response = await sessionFetch("POST", basicAuthorization(user, password), signal)
    if (response.status === 401) return "invalid"
    if (response.status !== 204) throw new Error()
    transition("signed-in")
    return "signed-in"
  }
  return cleanupPromise === null ? submit() : cleanupPromise.then(submit)
}

export async function apiFetch(input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  if (currentSnapshot !== "signed-in") throw new Error()
  const captured = generation
  const headers = new Headers(input instanceof Request ? input.headers : undefined)
  new Headers(init.headers).forEach((value, name) => headers.set(name, value))
  headers.delete("Authorization")
  headers.set("X-Kronika-UI", "1")

  const response = await fetch(input, { ...init, credentials: "same-origin", headers })
  if (response.status === 401 && generation === captured) void clearSession("expired")
  return response
}

export function logout(): Promise<void> {
  return clearSession("signed-out")
}

function clearSession(destination: SignedOutSnapshot): Promise<void> {
  if (cleanupPromise !== null) return cleanupPromise

  const cleanupGeneration = ++generation
  transition("pending")
  cleanupPromise = sessionFetch("DELETE").catch(() => {}).then(() => {
    cleanupPromise = null
    if (generation === cleanupGeneration) transition(destination)
  })
  return cleanupPromise
}

function sessionFetch(method: string, authorization?: string, signal: AbortSignal | null = null): Promise<Response> {
  const headers: Record<string, string> = { "X-Kronika-UI": "1" }
  if (authorization !== undefined) headers.Authorization = authorization
  return fetch("/auth/session", { credentials: "same-origin", headers, method, signal })
}

function transition(snapshot: SessionSnapshot): void {
  currentSnapshot = snapshot
  for (const listener of listeners) listener()
}

function basicAuthorization(user: string, password: string): string {
  return `Basic ${btoa(String.fromCharCode(...new TextEncoder().encode(`${user}:${password}`)))}`
}
