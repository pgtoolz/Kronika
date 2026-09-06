import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importFile } from "./import-module.mjs"

const { emptyHourStatusKey, isCurrentHour, latestTimelineTimestamp, REFRESH_INTERVAL_MS, refreshedCursor, scheduleRefresh } = await importFile("../src/refresh.ts")

const HOUR = 1_800_000_000_000_000

function timeline() {
  return {
    hour: HOUR,
    availableHours: [HOUR],
    availableSections: [],
    findingGroups: [],
    findings: [{ timestamp: HOUR + 40, segmentId: "a", logicalName: "health", typeId: "0", rowOrdinal: "0", fieldOrdinal: 0, kind: "spike" as const, category: null }],
    health: [{ timestamp: HOUR + 20 }],
    lanePoints: [{ timestamp: HOUR + 30 }],
    laneContexts: [],
    lanes: {},
    points: [{ timestamp: HOUR + 25 }],
    segments: [{ id: "a", minTs: HOUR + 10, maxTs: HOUR + 50, sections: [] }],
  } as never
}

test("only the current calendar hour schedules the shared fifteen-second refresh", async () => {
  assert.equal(isCurrentHour(HOUR, HOUR + 1), true)
  assert.equal(isCurrentHour(HOUR - 3_600_000_000, HOUR + 1), false)
  const visibility = fakeVisibility(false)
  const timers = fakeTimers()
  let now = HOUR + 1
  let refreshes = 0
  const dispose = scheduleRefresh(HOUR, async () => { refreshes += 1 }, visibility, timers, () => now)

  assert.deepEqual(timers.pendingDelays(), [REFRESH_INTERVAL_MS])
  timers.advance(REFRESH_INTERVAL_MS)
  await tick()
  assert.equal(refreshes, 1)
  now = HOUR + 3_600_000_000
  timers.advance(REFRESH_INTERVAL_MS)
  await tick()
  assert.equal(refreshes, 1)
  assert.equal(timers.pending(), 0)
  dispose()
  assert.equal(timers.pending(), 0)

  scheduleRefresh(HOUR - 3_600_000_000, async () => { refreshes += 1 }, visibility, timers, () => HOUR + 1)
  assert.equal(timers.pending(), 0)
})

test("an empty open hour stays provisional while a completed hour is definitive", () => {
  assert.equal(emptyHourStatusKey(HOUR, HOUR + 1), "status.no_data_current")
  assert.equal(emptyHourStatusKey(HOUR - 3_600_000_000, HOUR + 1), "status.no_data_completed")
})

test("a hidden page stops polling and refreshes once when it becomes visible", async () => {
  const visibility = fakeVisibility(true)
  const timers = fakeTimers()
  let refreshes = 0
  const dispose = scheduleRefresh(HOUR, async () => { refreshes += 1 }, visibility, timers, () => HOUR + 1)
  assert.equal(timers.pending(), 0)

  visibility.setHidden(false)
  await tick()
  assert.equal(refreshes, 1)
  assert.equal(timers.pending(), 0)
  visibility.setHidden(true)
  assert.equal(timers.pending(), 0)
  dispose()
  assert.equal(visibility.listeners.size, 0)
})

test("the latest cursor includes the newest stored source without leaving the hour", () => {
  assert.equal(latestTimelineTimestamp(timeline()), HOUR + 50)
  assert.equal(latestTimelineTimestamp({ ...timeline(), segments: [{ id: "a", minTs: HOUR, maxTs: HOUR + 4_000_000_000, sections: [] }] } as never), HOUR + 3_600_000_000 - 1)
  assert.equal(refreshedCursor(HOUR + 12, false, timeline()), HOUR + 12)
  assert.equal(refreshedCursor(HOUR + 12, true, timeline()), HOUR + 50)
})

test("a slow refresh cannot overlap or enter a fifteen-second cancellation loop", async () => {
  const visibility = fakeVisibility(false)
  const timers = fakeTimers()
  const first = deferred()
  let requests = 0
  const harness = refreshHarness(() => {
    requests += 1
    return first.promise
  }, visibility, timers, () => HOUR + 1)

  timers.advance(REFRESH_INTERVAL_MS)
  await tick()
  assert.equal(requests, 1)
  timers.advance(REFRESH_INTERVAL_MS * 3)
  await tick()
  assert.equal(requests, 1)
  assert.equal(timers.pending(), 0)
  harness.request()
  assert.equal(requests, 1)

  first.resolve()
  await tick()
  assert.deepEqual(timers.pendingDelays(), [REFRESH_INTERVAL_MS])
  timers.advance(REFRESH_INTERVAL_MS - 1)
  assert.equal(requests, 1)
  timers.advance(1)
  await tick()
  assert.equal(requests, 2)
  harness.dispose()
})

test("refresh keeps a committed cursor stable and reloads latest exactly once when it advances", () => {
  assert.equal(refreshedCursor(HOUR + 12, false, timeline()), HOUR + 12)
  const advanced = refreshedCursor(HOUR + 12, true, timeline())
  assert.equal(advanced, HOUR + 50)
  assert.equal(refreshedCursor(advanced, true, timeline()), advanced)
  assert.equal(refreshedCursor(HOUR + 60, true, timeline()), HOUR + 60)
})

test("a failed following-latest refresh restores one committed view before the retry advances", async () => {
  const source = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  assert.match(source, /readonly previousCursor: number/)
  assert.match(source, /pendingRefresh\.current = \{ timeline, previousCursor: cursor, previousSegments: segmentsRef\.current \}/)
  assert.match(source, /else if \(pending !== null\) \{\s*setSegments\(pending\.previousSegments\)\s*setCursor\(pending\.previousCursor\)\s*\}/)
  const failedBranch = source.match(/else if \(pending !== null\) \{([\s\S]*?)\n    \}/)?.[1] ?? ""
  assert.doesNotMatch(failedBranch, /setTimelineData|setCurrentData/)

  const a = {
    cursor: HOUR + 12,
    data: "A",
    segments: [{ id: "A" }],
    timeline: "A",
  }
  const bTimeline = {
    ...timeline(),
    segments: [{ id: "B", minTs: HOUR + 10, maxTs: HOUR + 80, sections: [] }],
  } as never
  const pending = {
    previousCursor: a.cursor,
    previousSegments: a.segments,
  }
  const next = refreshedCursor(a.cursor, true, bTimeline)
  assert.equal(next, HOUR + 80)

  const awaitingSnapshot = { ...a, cursor: next, segments: [{ id: "B" }] }
  const failed = {
    ...awaitingSnapshot,
    cursor: pending.previousCursor,
    segments: pending.previousSegments,
  }
  assert.deepEqual(failed, a)

  const retry = {
    cursor: refreshedCursor(failed.cursor, true, bTimeline),
    data: "B",
    segments: [{ id: "B" }],
    timeline: "B",
  }
  assert.deepEqual(retry, { cursor: HOUR + 80, data: "B", segments: [{ id: "B" }], timeline: "B" })
  assert.equal(refreshedCursor(a.cursor, false, bTimeline), a.cursor)
})

test("first table settlement gates slow hour products without gating Process rows", async () => {
  const [app, api, summary] = await Promise.all([
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/api.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8"),
  ])
  assert.match(api, /new URLSearchParams\(\{ part: "base" \}\)/)
  assert.doesNotMatch(api, /loadHealthMetadata|Promise\.all\(\[\.\.\.evaluations\]/)
  assert.match(app, /backgroundReadyHour !== backgroundTimeline\.hour/)
  assert.match(app, /backgroundTimeline\.segments\.length === 0/)
  assert.match(app, /backgroundTimeline === null \|\| backgroundTimeline\.hour !== hour/)
  assert.match(app, /backgroundTimelineRef\.current !== backgroundTimeline/)
  assert.match(app, /const \[snapshotReloadVersion, setSnapshotReloadVersion\] = useState\(0\)/)
  assert.match(app, /const foregroundAlreadyLoaded = refreshAwaitingSnapshot\.current/)
  assert.match(app, /if \(!foregroundAlreadyLoaded\) setSnapshotReloadVersion\(\(version\) => version \+ 1\)/)
  assert.match(app, /setBackgroundTimeline\(timeline\)\s+setSnapshotReloadVersion\(\(version\) => version \+ 1\)/)
  const foregroundSnapshotEffect = app.slice(
    app.indexOf('const [snapshotRequest, setSnapshotRequest]'),
    app.indexOf('if (backgroundTimeline === null || backgroundTimeline.segments.length === 0'),
  )
  assert.match(foregroundSnapshotEffect, /\}, \[finishRefresh, foregroundKey, hour, snapshotReloadVersion, snapshotTarget\]\)/)
  assert.doesNotMatch(foregroundSnapshotEffect, /\[backgroundTimeline,/)
  assert.match(app, /enabled=\{processReadyHour === hour && foregroundReadyKey\.current === foregroundKey\}/)
  assert.match(summary, /if \(!enabled \|\| \(state\.hour === hour && state\.status !== "loading"\)\) return/)
  assert.match(app, /foregroundReadyKey\.current === foregroundKey \? 250 : 0/)
  assert.match(app, /foregroundReadyKey\.current = foregroundKey\s+setBackgroundReadyHour\(hour\)\s+if \(visibleSource === "processes"\) setProcessReadyHour\(hour\)/)
  assert.match(app, /foregroundReadyKey\.current = foregroundKey\s+if \(visibleSource !== "events"\) setBackgroundReadyHour\(hour\)/)
  const failedPage = app.match(/const failed = \(reason: unknown\) => \{([\s\S]*?)\n          \}/)?.[1] ?? ""
  assert.doesNotMatch(failedPage, /setBackgroundReadyHour|setProcessReadyHour|foregroundReadyKey/)

  const processFastPath = app.match(/if \(pageCursor === undefined && visibleSource === "processes"\) \{([\s\S]*?)\n          \}/)?.[1] ?? ""
  assert.ok(processFastPath.indexOf("loaded(incoming, null, ordinaryGroups.length !== 0)") >= 0)
  assert.ok(processFastPath.indexOf("loaded(incoming, null, ordinaryGroups.length !== 0)") < processFastPath.indexOf("void base.then"))
  assert.match(processFastPath, /companionPending: false/)
  assert.match(processFastPath, /mergeSnapshotData\(current\.data, companion\)/)

  assert.doesNotMatch(app, /fields: \["mem_total"\]/)
  assert.match(app, /\.\.\.\(lens === "tree" \? \[\{ section: "os_meminfo" \}\] : \[\]\)/)
})

test("the instance label waits for one settled foreground table", async () => {
  const app = await readFile(new URL("../src/app.tsx", import.meta.url), "utf8")
  const ready = app.indexOf('const refreshReady = !loading && cursorState === "ready" && densePageState !== "loading"')
  const requested = app.indexOf("instanceLabelRequest.current = controller")
  const fetch = app.indexOf('apiFetch("/api/instance-label"')
  assert.ok(ready >= 0)
  assert.ok(requested > ready)
  assert.ok(fetch > requested)
  assert.doesNotMatch(app.slice(0, ready), /apiFetch\("\/api\/instance-label"/)

  const gate = app.slice(app.lastIndexOf("useEffect(() => {", fetch), requested)
  assert.match(gate, /instanceLabelRequest\.current !== null/)
  assert.match(gate, /!refreshReady/)
  assert.match(gate, /backgroundReadyHour !== hour/)
  assert.match(gate, /foregroundReadyKey\.current !== foregroundKey/)
  assert.match(app.slice(fetch, app.indexOf("const denseMetadata", fetch)), /\}, \[backgroundReadyHour, foregroundKey, hour, refreshReady\]\)/)
})

function fakeVisibility(initial: boolean) {
  const listeners = new Set<() => void>()
  return {
    hidden: initial,
    listeners,
    addEventListener(_type: "visibilitychange", listener: () => void) { listeners.add(listener) },
    removeEventListener(_type: "visibilitychange", listener: () => void) { listeners.delete(listener) },
    setHidden(hidden: boolean) {
      this.hidden = hidden
      for (const listener of listeners) listener()
    },
  }
}

function fakeTimers() {
  let next = 0
  let now = 0
  const callbacks = new Map<number, () => void>()
  const deadlines = new Map<number, number>()
  return {
    setTimeout(callback: () => void, milliseconds: number) {
      next += 1
      callbacks.set(next, callback)
      deadlines.set(next, now + milliseconds)
      return next
    },
    clearTimeout(id: number) {
      callbacks.delete(id)
      deadlines.delete(id)
    },
    advance(milliseconds: number) {
      now += milliseconds
      for (const [id, deadline] of [...deadlines]) {
        if (deadline > now) continue
        const callback = callbacks.get(id)
        callbacks.delete(id)
        deadlines.delete(id)
        callback?.()
      }
    },
    pending() { return deadlines.size },
    pendingDelays() { return [...deadlines.values()].map((deadline) => deadline - now) },
  }
}

function refreshHarness(action: () => Promise<void>, visibility: ReturnType<typeof fakeVisibility>, timers: ReturnType<typeof fakeTimers>, now: () => number) {
  let busy = false
  let dispose = () => {}
  const render = () => {
    dispose()
    dispose = busy ? () => {} : scheduleRefresh(HOUR, request, visibility, timers, now)
  }
  const request = () => {
    if (busy) return
    busy = true
    render()
    void action().finally(() => { busy = false; render() })
  }
  render()
  return { request, dispose: () => dispose() }
}

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => { resolve = done })
  return { promise, resolve }
}

async function tick() {
  for (let pending = 0; pending < 8; pending += 1) await Promise.resolve()
}
