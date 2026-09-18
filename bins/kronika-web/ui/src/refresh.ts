import type { TimelineData } from "./api"
import { floorHour } from "./model"

export const REFRESH_INTERVAL_MS = 15_000

interface VisibilityTarget { readonly hidden: boolean; addEventListener(type: "visibilitychange", listener: () => void): void; removeEventListener(type: "visibilitychange", listener: () => void): void }
interface TimerTarget { setTimeout(handler: () => void, milliseconds: number): number; clearTimeout(id: number): void }

export function isCurrentHour(hour: number, now = Date.now() * 1_000): boolean {
  return hour === floorHour(now)
}

export function emptyHourStatusKey(hour: number, now = Date.now() * 1_000): "status.no_data_current" | "status.no_data_completed" {
  return isCurrentHour(hour, now) ? "status.no_data_current" : "status.no_data_completed"
}

export function latestTimelineTimestamp(timeline: TimelineData): number {
  const end = timeline.hour + 3_600_000_000
  let latest = timeline.hour
  const take = (timestamp: number) => { if (timestamp >= timeline.hour && timestamp < end) latest = Math.max(latest, timestamp) }
  for (const segment of timeline.segments) take(Math.min(segment.maxTs, end - 1))
  for (const row of timeline.health) take(row.timestamp)
  for (const point of timeline.points) take(point.timestamp)
  for (const point of timeline.lanePoints) take(point.timestamp)
  for (const finding of timeline.findings) take(finding.timestamp)
  return latest
}

export function refreshedCursor(current: number, followsLatest: boolean, timeline: TimelineData): number {
  return followsLatest ? Math.max(current, latestTimelineTimestamp(timeline)) : current
}

export function scheduleRefresh(
  hour: number,
  refresh: () => void,
  visibility: VisibilityTarget = document,
  timers: TimerTarget = window,
  now: () => number = () => Date.now() * 1_000,
): () => void {
  let timer: number | null = null
  const stop = () => {
    if (timer !== null) timers.clearTimeout(timer)
    timer = null
  }
  // The timer re-arms itself after every tick: what a refresh did, or
  // whether it ever finished, never decides whether the next one is asked for.
  const arm = () => {
    stop()
    if (visibility.hidden || !isCurrentHour(hour, now())) return
    timer = timers.setTimeout(tick, REFRESH_INTERVAL_MS)
  }
  const tick = () => {
    timer = null
    try {
      if (!visibility.hidden && isCurrentHour(hour, now())) refresh()
    } finally {
      arm()
    }
  }
  const changed = () => { if (visibility.hidden) stop(); else tick() }
  arm()
  visibility.addEventListener("visibilitychange", changed)
  return () => {
    stop()
    visibility.removeEventListener("visibilitychange", changed)
  }
}
