/**
 * A storage surface that never makes an unavailable origin fatal.
 *
 * Some WebKit configurations can make the `window.localStorage` getter itself
 * throw, and opaque origins can make individual operations throw. Every access
 * therefore goes through one guarded getter and one guarded method call. When
 * persistence is unavailable the UI keeps its defaults and current-session
 * state in React; it deliberately does not claim durable storage.
 */
export interface StorageTarget {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem?(key: string): void
}

export interface SafeStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

export type StorageGetter = () => StorageTarget | null | undefined

function domStorage(): StorageTarget | null {
  try {
    const candidate: unknown = (globalThis as { readonly localStorage?: unknown }).localStorage
    return candidate === null || typeof candidate !== "object" ? null : candidate as StorageTarget
  } catch {
    return null
  }
}

export function safeStorage(getTarget: StorageGetter = domStorage): SafeStorage {
  return {
    getItem(key) {
      try {
        return getTarget()?.getItem(key) ?? null
      } catch {
        return null
      }
    },
    setItem(key, value) {
      try {
        getTarget()?.setItem(key, value)
      } catch {
        // Defaults and current-session state remain usable.
      }
    },
    removeItem(key) {
      try {
        getTarget()?.removeItem?.(key)
      } catch {
        // A failed invalidation must not break the interaction that requested it.
      }
    },
  }
}

/** Shared instance for first-party UI preferences. */
export const persistentStorage: SafeStorage = safeStorage()
