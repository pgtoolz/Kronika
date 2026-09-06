import { useEffect, useRef, useState } from "react"

export type HistoryStatus = "error" | "loading" | "ready"

interface StoredHistory<T> {
  readonly key: string | null
  readonly status: HistoryStatus
  readonly value: T | null
}

export interface HistoryState<T> {
  readonly status: HistoryStatus
  readonly value: T | null
}

export function useHistoryRequest<T>(
  key: string | null,
  revision: string | number | null,
  request: ((signal: AbortSignal) => Promise<T>) | null,
): HistoryState<T> {
  const requestRef = useRef(request)
  requestRef.current = request
  const [stored, setStored] = useState<StoredHistory<T>>({ key: null, status: "ready", value: null })

  useEffect(() => {
    if (key === null || requestRef.current === null) {
      setStored({ key: null, status: "ready", value: null })
      return
    }
    setStored((current) => beginHistory(current, key))
    const controller = new AbortController()
    void requestRef.current(controller.signal).then((value) => {
      if (!controller.signal.aborted) setStored((current) => finishHistory(current, key, value))
    }).catch(() => {
      if (!controller.signal.aborted) setStored((current) => failHistory(current, key))
    })
    return () => controller.abort()
  }, [key, revision])

  return visibleHistory(stored, key)
}

export function beginHistory<T>(current: StoredHistory<T>, key: string): StoredHistory<T> {
  return current.key === key
    ? { ...current, status: "loading" }
    : { key, status: "loading", value: null }
}

export function finishHistory<T>(current: StoredHistory<T>, key: string, value: T): StoredHistory<T> {
  return current.key === key
    ? { key, status: "ready", value }
    : current
}

export function failHistory<T>(current: StoredHistory<T>, key: string): StoredHistory<T> {
  return current.key === key
    ? { ...current, status: "error" }
    : current
}

export function visibleHistory<T>(current: StoredHistory<T>, key: string | null): HistoryState<T> {
  if (key === null) return { status: "ready", value: null }
  return current.key === key
    ? { status: current.status, value: current.value }
    : { status: "loading", value: null }
}
