// A cut may sum physical counters and scale values with recorded block-size or
// clock-rate metadata. Missing metadata leaves the values as raw counts.
export interface ActivityCut {
  readonly id: string
  readonly fields: readonly string[]
  readonly kind: "milliseconds" | "seconds" | "microseconds" | "nanoseconds" | "count" | "bytes"
  readonly scaleBy?: "block_size" | "clock_ticks" | "kib"
}

export interface ActivityScales {
  readonly blockSize: number | null
  readonly clockTicks: number | null
}

export function cutScale(cut: ActivityCut, scales: ActivityScales): { readonly scale: number; readonly kind: ActivityCut["kind"] } {
  if (cut.scaleBy === "block_size") {
    return scales.blockSize === null || scales.blockSize <= 0
      ? { scale: 1, kind: "count" }
      : { scale: scales.blockSize, kind: cut.kind }
  }
  if (cut.scaleBy === "clock_ticks") {
    return scales.clockTicks === null || scales.clockTicks <= 0
      ? { scale: 1, kind: "count" }
      : { scale: 1 / scales.clockTicks, kind: cut.kind }
  }
  if (cut.scaleBy === "kib") return { scale: 1_024, kind: cut.kind }
  return { scale: 1, kind: cut.kind }
}

export const STATEMENT_CUTS: readonly ActivityCut[] = [
  { id: "exec_time", fields: ["total_exec_time"], kind: "milliseconds" },
  { id: "calls", fields: ["calls"], kind: "count" },
  { id: "rows", fields: ["rows"], kind: "count" },
  { id: "shared_read", fields: ["shared_blks_read"], kind: "bytes", scaleBy: "block_size" },
  { id: "shared_dirtied", fields: ["shared_blks_dirtied"], kind: "bytes", scaleBy: "block_size" },
  { id: "temp_written", fields: ["temp_blks_written"], kind: "bytes", scaleBy: "block_size" },
  { id: "wal_bytes", fields: ["wal_bytes"], kind: "bytes" },
]

// pg_store_plans has no WAL or planning counters shared across its forks;
// execution time is total_time in every fork.
export const PLAN_CUTS: readonly ActivityCut[] = [
  { id: "exec_time", fields: ["total_time"], kind: "milliseconds" },
  { id: "calls", fields: ["calls"], kind: "count" },
  { id: "rows", fields: ["rows"], kind: "count" },
  { id: "shared_read", fields: ["shared_blks_read"], kind: "bytes", scaleBy: "block_size" },
  { id: "temp_written", fields: ["temp_blks_written"], kind: "bytes", scaleBy: "block_size" },
]

export const TABLE_CUTS: readonly ActivityCut[] = [
  { id: "writes", fields: ["n_tup_ins", "n_tup_upd", "n_tup_del"], kind: "count" },
  { id: "seq_read", fields: ["seq_tup_read"], kind: "count" },
  { id: "heap_read", fields: ["heap_blks_read"], kind: "bytes", scaleBy: "block_size" },
  { id: "dead_tuples", fields: ["n_dead_tup"], kind: "count" },
  { id: "autovacuum_time", fields: ["total_autovacuum_time"], kind: "milliseconds" },
]

export const INDEX_CUTS: readonly ActivityCut[] = [
  { id: "idx_scan", fields: ["idx_scan"], kind: "count" },
  { id: "idx_tup_read", fields: ["idx_tup_read"], kind: "count" },
  { id: "idx_blks_read", fields: ["idx_blks_read"], kind: "bytes", scaleBy: "block_size" },
]

// utime and stime are jiffies; the recorded clock rate turns their sum into
// CPU seconds, so one second per second reads as one busy core.
export const PROCESS_CUTS: readonly ActivityCut[] = [
  { id: "cpu", fields: ["utime", "stime"], kind: "seconds", scaleBy: "clock_ticks" },
  { id: "rss", fields: ["rmem_kb"], kind: "bytes", scaleBy: "kib" },
  { id: "io_read", fields: ["read_bytes"], kind: "bytes" },
  { id: "io_write", fields: ["write_bytes"], kind: "bytes" },
  { id: "majflt", fields: ["majflt"], kind: "count" },
  { id: "run_delay", fields: ["rundelay_ns"], kind: "nanoseconds" },
]

export const DATABASE_CUTS: readonly ActivityCut[] = [
  { id: "commits", fields: ["xact_commit"], kind: "count" },
  { id: "rollbacks", fields: ["xact_rollback"], kind: "count" },
  { id: "db_read", fields: ["blks_read"], kind: "bytes", scaleBy: "block_size" },
  { id: "temp_bytes", fields: ["temp_bytes"], kind: "bytes" },
  { id: "deadlocks", fields: ["deadlocks"], kind: "count" },
]

export const CGROUP_CPU_CUTS: readonly ActivityCut[] = [
  { id: "cg_cpu", fields: ["usage_usec"], kind: "microseconds" },
  { id: "cg_throttled", fields: ["throttled_usec"], kind: "microseconds" },
]

export const CGROUP_IO_CUTS: readonly ActivityCut[] = [
  { id: "cg_read", fields: ["rbytes"], kind: "bytes" },
  { id: "cg_write", fields: ["wbytes"], kind: "bytes" },
  { id: "cg_rios", fields: ["rios"], kind: "count" },
  { id: "cg_wios", fields: ["wios"], kind: "count" },
]

export function activityPreview(text: string): string {
  return text.replace(/\s+/g, " ").trim().slice(0, 240)
}
