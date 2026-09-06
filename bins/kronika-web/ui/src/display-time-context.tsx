import { createContext, useContext, useMemo, type ReactNode } from "react"

import { browserTimeZone, createDisplayTimeFormatter, type DisplayTimeFormatter, type DisplayTimeZone } from "./display-time"
import type { Locale } from "./model"

interface DisplayTimeValue extends DisplayTimeFormatter {
  readonly setMode: (mode: DisplayTimeZone) => void
}

const localZone = browserTimeZone()
const fallback = createDisplayTimeFormatter("en", "browser", localZone)
const DisplayTimeContext = createContext<DisplayTimeValue>({
  ...fallback,
  setMode: () => {},
})

export function DisplayTimeProvider({ children, locale, mode, setMode }: {
  readonly children: ReactNode
  readonly locale: Locale
  readonly mode: DisplayTimeZone
  readonly setMode: (mode: DisplayTimeZone) => void
}) {
  const value = useMemo<DisplayTimeValue>(() => {
    const formatter = createDisplayTimeFormatter(locale, mode, localZone)
    return { ...formatter, setMode }
  }, [locale, mode, setMode])
  return <DisplayTimeContext.Provider value={value}>{children}</DisplayTimeContext.Provider>
}

// The selected hour establishes one civil date for every descendant. Ordinary
// timestamp/range rendering becomes compact only when the value is unambiguous
// in that date; numeric instants remain exact.
export function DisplayTimeScope({ children, hour }: { readonly children: ReactNode; readonly hour: number | null }) {
  const parent = useContext(DisplayTimeContext)
  const value = useMemo<DisplayTimeValue>(() => hour === null ? parent : ({
    ...parent,
    range: (from, to) => parent.range(from, to, hour),
    startTime: (timestamp) => parent.startTime(timestamp, hour),
    timestamp: (timestamp) => parent.timestamp(timestamp, hour),
  }), [hour, parent])
  return <DisplayTimeContext.Provider value={value}>{children}</DisplayTimeContext.Provider>
}

export function useDisplayTime(): DisplayTimeValue {
  return useContext(DisplayTimeContext)
}
