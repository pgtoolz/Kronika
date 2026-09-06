import type { DataRow } from "./api"
import { asNumber, rawText, value } from "./model"
import { exactCounterDelta } from "./postgres-metrics"

// The vacuum ledger: one recorded hour of pg_stat_progress_vacuum, grouped
// into episodes for display. Grouping is presentation over recorded values,
// the same shape as the events console: rows group by recorded identity
// fields, and nothing is inferred about time nobody recorded.

// Two samples of one key continue an episode when they are no further apart
// than this many recorded sampling intervals: collection drifts, so exactly
// one interval would split ordinary runs. Without a recorded interval the
// time condition is not applied — nothing is invented in its place.
export const EPISODE_ADJACENCY_FACTOR = 2.5

// How many consecutive samples with an unchanged designated counter make the
// "No movement" reading.
export const NO_MOVEMENT_SAMPLES = 3

export type VacuumRisk = "ordinary" | "heavy" | "dangerous"

// Fixed by phase name, never computed from observed load. `truncating heap`
// is dangerous unconditionally: the phase is set before the conditional
// AccessExclusiveLock attempt.
const PHASE_RISK: Readonly<Record<string, VacuumRisk>> = {
  "cleaning up indexes": "heavy",
  "initializing": "ordinary",
  "performing final cleanup": "ordinary",
  "scanning heap": "ordinary",
  "truncating heap": "dangerous",
  "vacuuming heap": "heavy",
  "vacuuming indexes": "heavy",
}

export function phaseRisk(phase: string | null): VacuumRisk {
  return phase === null ? "ordinary" : PHASE_RISK[phase] ?? "ordinary"
}

export interface VacuumEpisode {
  /// Recorded samples, ascending by timestamp.
  readonly rows: readonly DataRow[]
  readonly last: DataRow
  /// The trailing run of samples sharing the last row's phase and index
  /// cycle: a cycle increment starts a new span even when the phase string
  /// is unchanged.
  readonly phaseRows: readonly DataRow[]
  readonly noMovement: { readonly samples: number; readonly spanUs: number } | null
}

// Counters that only grow within one vacuum run. A row where any of them is
// lower than in the previous row is a different run and starts a new episode.
const MONOTONE_FIELDS = ["index_vacuum_count", "heap_blks_scanned", "heap_blks_vacuumed"] as const

function episodeKey(row: DataRow): string {
  return [row.typeId, rawText(value(row, "pid")) ?? "", rawText(value(row, "datid")) ?? "", rawText(value(row, "relid")) ?? ""].join(":")
}

export function buildVacuumEpisodes(rows: readonly DataRow[], intervalSeconds: number | null): readonly VacuumEpisode[] {
  const streams = new Map<string, DataRow[]>()
  for (const row of [...rows].sort((left, right) => left.timestamp - right.timestamp)) {
    const key = episodeKey(row)
    const stored = streams.get(key)
    if (stored === undefined) streams.set(key, [row])
    else stored.push(row)
  }
  const limit = intervalSeconds === null || intervalSeconds <= 0
    ? null
    : intervalSeconds * 1_000_000 * EPISODE_ADJACENCY_FACTOR
  const episodes: VacuumEpisode[] = []
  for (const stream of streams.values()) {
    let current: DataRow[] = []
    for (const row of stream) {
      const previous = current[current.length - 1]
      const continues = previous !== undefined
        && (limit === null || row.timestamp - previous.timestamp <= limit)
        && MONOTONE_FIELDS.every((field) => {
          const before = asNumber(value(previous, field))
          const after = asNumber(value(row, field))
          return before === null || after === null || after >= before
        })
      if (!continues && current.length > 0) {
        episodes.push(finishEpisode(current))
        current = []
      }
      current.push(row)
    }
    if (current.length > 0) episodes.push(finishEpisode(current))
  }
  return episodes
}

function finishEpisode(rows: readonly DataRow[]): VacuumEpisode {
  const last = rows[rows.length - 1] as DataRow
  const phase = rawText(value(last, "phase"))
  const cycle = asNumber(value(last, "index_vacuum_count"))
  let start = rows.length - 1
  while (start > 0) {
    const candidate = rows[start - 1] as DataRow
    if (rawText(value(candidate, "phase")) !== phase) break
    if (asNumber(value(candidate, "index_vacuum_count")) !== cycle) break
    start -= 1
  }
  const phaseRows = rows.slice(start)
  return { rows, last, phaseRows, noMovement: noMovement(phaseRows, last) }
}

// The counter whose movement says the phase is progressing. Layout 1_012_004
// (PG10-16) does not record index progress, so its index phases never claim
// stillness.
function movementField(phase: string | null, typeId: string): string | null {
  if (phase === "scanning heap") return "heap_blks_scanned"
  if (phase === "vacuuming heap") return "heap_blks_vacuumed"
  if (phase === "vacuuming indexes" || phase === "cleaning up indexes") {
    return typeId === "1012004" ? null : "indexes_processed"
  }
  if (phase === "truncating heap") return "phase"
  return null
}

function noMovement(phaseRows: readonly DataRow[], last: DataRow): VacuumEpisode["noMovement"] {
  const field = movementField(rawText(value(last, "phase")), last.typeId)
  if (field === null) return null
  let start = phaseRows.length - 1
  if (field !== "phase") {
    const reading = asNumber(value(last, field))
    if (reading === null) return null
    while (start > 0 && asNumber(value(phaseRows[start - 1] as DataRow, field)) === reading) start -= 1
  } else {
    start = 0
  }
  const still = phaseRows.slice(start)
  if (still.length < NO_MOVEMENT_SAMPLES) return null
  const first = still[0] as DataRow
  return { samples: still.length, spanUs: last.timestamp - first.timestamp }
}

// The moment of the collection pass the cursor stands on: the newest recorded
// progress timestamp at or before the cursor. An episode whose last sample
// carries this timestamp was seen at the cursor; every other episode was last
// seen at its own recorded moment.
export function vacuumAtTimestamp(rows: readonly DataRow[], cursor: number): number | null {
  let at: number | null = null
  for (const row of rows) {
    if (row.timestamp <= cursor && (at === null || row.timestamp > at)) at = row.timestamp
  }
  return at
}

const RISK_ORDER: Readonly<Record<VacuumRisk, number>> = { dangerous: 0, heavy: 1, ordinary: 2 }

// At-sample episodes first, riskiest first, longest phase first; everything
// last seen earlier follows, newest first.
export function sortVacuumEpisodes(episodes: readonly VacuumEpisode[], atTs: number | null): readonly VacuumEpisode[] {
  return [...episodes].sort((left, right) => {
    const leftAt = atTs !== null && left.last.timestamp === atTs
    const rightAt = atTs !== null && right.last.timestamp === atTs
    if (leftAt !== rightAt) return leftAt ? -1 : 1
    if (leftAt) {
      const risk = RISK_ORDER[phaseRisk(rawText(value(left.last, "phase")))] - RISK_ORDER[phaseRisk(rawText(value(right.last, "phase")))]
      if (risk !== 0) return risk
      const span = phaseSpanUs(right) - phaseSpanUs(left)
      if (span !== 0) return span
      const cycles = (asNumber(value(right.last, "index_vacuum_count")) ?? 0) - (asNumber(value(left.last, "index_vacuum_count")) ?? 0)
      if (cycles !== 0) return cycles
    }
    return right.last.timestamp - left.last.timestamp
  })
}

export function phaseSpanUs(episode: VacuumEpisode): number {
  const first = episode.phaseRows[0]
  return first === undefined ? 0 : episode.last.timestamp - first.timestamp
}

// Whether any loaded row's layout defines the column: PG17 index progress
// exists from 1_012_005 on, the PG18 cost delay only on 1_012_006. In an
// hour without such layouts the columns are omitted rather than all-N/A.
export function vacuumLayoutHas(rows: readonly DataRow[], field: "indexes_total" | "delay_time"): boolean {
  return rows.some((row) => field === "delay_time" ? row.typeId === "1012006" : row.typeId !== "1012004")
}

// The scan-progress percent of every episode sample that carries a positive
// heap total, in recorded order: what the row's progress sparkline draws.
// Samples with no usable total are skipped, not zero-filled.
export function progressSeries(episode: VacuumEpisode): readonly number[] {
  const series: number[] = []
  for (const row of episode.rows) {
    const scanned = asNumber(value(row, "heap_blks_scanned"))
    const total = asNumber(value(row, "heap_blks_total"))
    if (scanned === null || total === null || total <= 0) continue
    series.push(Math.max(0, Math.min(100, (scanned / total) * 100)))
  }
  return series
}

// The PG18 cost-delay delta between the episode's last two samples, when the
// layout records it. Cumulative milliseconds; adjacent samples only.
export function delayDelta(episode: VacuumEpisode): number | null {
  if (episode.last.typeId !== "1012006") return null
  const previous = episode.rows[episode.rows.length - 2]
  if (previous === undefined) return null
  const before = asNumber(value(previous, "delay_time"))
  const after = asNumber(value(episode.last, "delay_time"))
  return before === null || after === null || after < before ? null : after - before
}

export interface VacuumProcessLoad {
  readonly before: DataRow
  readonly after: DataRow
  /// User plus system CPU time, in jiffies (scale by the recorded clock rate).
  readonly cpuTicks: bigint | null
  /// Time blocked on I/O, in jiffies.
  readonly blockWaitTicks: bigint | null
  /// Time waiting for a CPU slot, nanoseconds.
  readonly runDelayNs: bigint | null
  /// Bytes actually read from storage, not page-cache hits.
  readonly readBytes: bigint | null
  readonly writeBytes: bigint | null
  readonly majorFaults: bigint | null
}

// Use this PID's latest samples at or before the episode's first and last
// recorded moments. Counters include all work by that PID within the span.
export function vacuumProcessLoad(processRows: readonly DataRow[], episode: VacuumEpisode): VacuumProcessLoad | null {
  const start = episode.rows[0]?.timestamp
  if (start === undefined) return null
  const before = latestAtOrBefore(processRows, start)
  const after = latestAtOrBefore(processRows, episode.last.timestamp)
  if (before === null || after === null || after.timestamp <= before.timestamp) return null
  const delta = (field: string) => exactCounterDelta(value(before, field), value(after, field))
  return {
    before,
    after,
    cpuTicks: sumDeltas(before, after, ["utime", "stime"]),
    blockWaitTicks: delta("blkdelay_ticks"),
    runDelayNs: delta("rundelay_ns"),
    readBytes: delta("read_bytes"),
    writeBytes: delta("write_bytes"),
    majorFaults: delta("majflt"),
  }
}

export interface VacuumLoadShares {
  readonly cpuMs: number | null
  readonly cpuShare: number | null
  readonly blockWaitMs: number | null
  readonly readBytes: number | null
  readonly writeBytes: number | null
  readonly readShare: number | null
  readonly majorFaults: number | null
}

// The display numbers derived from a raw load delta: CPU and block-wait
// converted from ticks, and each byte delta's share of what its natural
// comparison covers — CPU against the span's own wall-clock time, bytes read
// against what PG itself reports scanning. `scannedBytes` is null when the
// recorded block size is unknown, and the read share is then withheld rather
// than compared against a guessed size.
export function vacuumLoadShares(load: VacuumProcessLoad, ticksPerSecond: number | null, scannedBytes: number | null): VacuumLoadShares {
  const spanSeconds = (load.after.timestamp - load.before.timestamp) / 1_000_000
  const cpuMs = ticksToMs(load.cpuTicks, ticksPerSecond)
  const readBytes = load.readBytes === null ? null : Number(load.readBytes)
  return {
    cpuMs,
    cpuShare: cpuMs !== null && spanSeconds > 0 ? Math.min(100, cpuMs / 1000 / spanSeconds * 100) : null,
    blockWaitMs: ticksToMs(load.blockWaitTicks, ticksPerSecond),
    readBytes,
    writeBytes: load.writeBytes === null ? null : Number(load.writeBytes),
    readShare: readBytes !== null && scannedBytes !== null && scannedBytes > 0
      ? Math.min(100, readBytes / scannedBytes * 100)
      : null,
    majorFaults: load.majorFaults === null ? null : Number(load.majorFaults),
  }
}

function ticksToMs(ticks: bigint | null, ticksPerSecond: number | null): number | null {
  return ticks === null || ticksPerSecond === null || ticksPerSecond <= 0 ? null : (Number(ticks) / ticksPerSecond) * 1000
}

function sumDeltas(before: DataRow, after: DataRow, fields: readonly string[]): bigint | null {
  let total = 0n
  for (const field of fields) {
    const delta = exactCounterDelta(value(before, field), value(after, field))
    if (delta === null) return null
    total += delta
  }
  return total
}

function latestAtOrBefore(rows: readonly DataRow[], target: number): DataRow | null {
  let best: DataRow | null = null
  for (const row of rows) {
    if (row.timestamp <= target && (best === null || row.timestamp > best.timestamp)) best = row
  }
  return best
}
