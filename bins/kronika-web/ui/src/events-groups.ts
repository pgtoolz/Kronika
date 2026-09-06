export type EventTier = "critical" | "notable" | "routine"

export const EVENT_TIERS: readonly EventTier[] = ["critical", "notable", "routine"]
export const MINUTE_COLUMNS = 60

export type EventStat =
  | { readonly kind: "pg.errors"; readonly severity: number; readonly category: number | null; readonly sqlstate: string | null; readonly database: string | null; readonly username: string | null }
  | { readonly kind: "pg.slow"; readonly maxMs: number; readonly totalMs: number; readonly thresholdMs: number | null }
  | { readonly kind: "pg.autovacuum"; readonly analyze: boolean; readonly runs: number; readonly totalMs: number | null; readonly tuplesRemoved: number | null; readonly tuplesDead: number | null }
  | { readonly kind: "pg.checkpoints"; readonly completes: number; readonly timed: number; readonly requested: number; readonly maxSyncMs: number | null; readonly buffers: number | null }
  | { readonly kind: "pg.checkpoint_warning"; readonly secondsApart: number | null }
  | { readonly kind: "pg.locks"; readonly holders: string | null; readonly acquired: boolean; readonly waiters: number; readonly maxMs: number | null; readonly targets: readonly string[] }
  | { readonly kind: "pg.lifecycle"; readonly lifecycle: number; readonly pid: number | null; readonly signal: number | null; readonly mode: string | null }
  | {
    readonly kind: "pgbouncer.events"
    readonly level: number
    readonly database: string | null
    readonly username: string | null
    readonly host: string | null
    readonly sourceFile: string | null
  }

export interface EventEntry {
  readonly key: string
  readonly section: string
  readonly tier: EventTier
  readonly label: string | null
  readonly count: number
  readonly firstTs: number
  readonly lastTs: number
  readonly minutes: readonly number[]
  readonly stat: EventStat
  readonly detailRef: string
  readonly representativeTs: number
}
