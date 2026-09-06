import { reportFetch } from "./report-transport"

export type SessionSnapshot = "signed-in"

export function getSessionSnapshot(): SessionSnapshot {
  return "signed-in"
}

export function subscribeSession(): () => void {
  return () => {}
}

export function bootstrapSession(): Promise<void> {
  return Promise.resolve()
}

export function signInBasic(): Promise<"invalid"> {
  return Promise.resolve("invalid")
}

export function apiFetch(input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  return reportFetch(input, init)
}

export function logout(): Promise<void> {
  return Promise.resolve()
}
