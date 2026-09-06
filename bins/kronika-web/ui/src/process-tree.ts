import type { DataRow } from "./api"
import { asNumber, value } from "./model"
import { parseSearch, rowMatchesSearch } from "./search"

// The tree lens supplies an unpaged snapshot for parent-first ordering.

// A snapshot row carries the cumulative columns as rates: the layout record
// names them under `rates`, so utime and stime arrive as jiffies per second.
// Lifetime CPU time arrives separately, as cpu_time_ticks.
function metrics(row: DataRow, memTotalKb: number | null, ticksPerSecond: number | null): Record<string, number | string | null> {
  const cores = processQuantity(row, "cpu_cores", ticksPerSecond)
  const rmemKb = asNumber(value(row, "rmem_kb"))
  const lifetimeTicks = asNumber(value(row, "cpu_time_ticks"))
  return {
    cpu_percent: cores === null ? null : cores * 100,
    cpu_time_seconds: lifetimeTicks === null || ticksPerSecond === null || ticksPerSecond <= 0 ? null : lifetimeTicks / ticksPerSecond,
    mem_percent: rmemKb === null || memTotalKb === null || memTotalKb <= 0 ? null : (rmemKb / memTotalKb) * 100,
    process_stat: processStat(row),
  }
}

// The STAT column of ps: the state letter, then the flags this collection can
// tell. Session leadership and foreground group are not recorded, so `s` and
// `+` never appear.
function processStat(row: DataRow): string | null {
  const state = asNumber(value(row, "state"))
  if (state === null) return null
  const nice = asNumber(value(row, "nice"))
  const threads = asNumber(value(row, "num_threads"))
  const priority = nice === null || nice === 0 ? "" : nice < 0 ? "<" : "N"
  return `${String.fromCharCode(state)}${priority}${threads !== null && threads > 1 ? "l" : ""}`
}

function processQuantity(row: DataRow, field: string, ticksPerSecond: number | null): number | null {
  const stored = (name: string) => asNumber(value(row, name))
  const sum = (...names: readonly string[]) => {
    const values = names.map(stored)
    return values.some((entry) => entry === null) ? null : values.reduce<number>((total, entry) => total + (entry ?? 0), 0)
  }
  switch (field) {
    case "rss": return stored("rmem_kb") === null ? null : stored("rmem_kb")! * 1_024
    case "vsz": return stored("vmem_kb") === null ? null : stored("vmem_kb")! * 1_024
    case "swap": return stored("vswap_kb") === null ? null : stored("vswap_kb")! * 1_024
    case "threads": return stored("num_threads")
    case "cpu_cores": case "user_cpu_cores": case "system_cpu_cores": {
      if (ticksPerSecond === null || ticksPerSecond <= 0) return null
      const ticks = field === "user_cpu_cores" ? stored("utime") : field === "system_cpu_cores" ? stored("stime") : sum("utime", "stime")
      return ticks === null ? null : ticks / ticksPerSecond
    }
    case "disk_read_rate": return stored("read_bytes")
    case "disk_write_rate": return stored("write_bytes")
    case "logical_read_rate": return stored("rchar")
    case "logical_write_rate": return stored("wchar")
    case "read_syscall_rate": return stored("syscr")
    case "write_syscall_rate": return stored("syscw")
    case "major_fault_rate": return stored("majflt")
    case "minor_fault_rate": return stored("minflt")
    case "context_switch_rate": return sum("nvcsw", "nivcsw")
    case "voluntary_context_switch_rate": return stored("nvcsw")
    case "involuntary_context_switch_rate": return stored("nivcsw")
    case "run_delay": return stored("rundelay_ns") === null ? null : stored("rundelay_ns")! / 1_000_000
    case "block_io_delay": {
      const ticks = stored("blkdelay_ticks")
      return ticks === null || ticksPerSecond === null || ticksPerSecond <= 0 ? null : ticks * 1_000 / ticksPerSecond
    }
    default: return null
  }
}

export function filterProcessForest(rows: readonly DataRow[], pattern: string, ticksPerSecond: number | null): readonly DataRow[] {
  const parsed = parseSearch(pattern, "os_process")
  if (!parsed.ok || parsed.query.canonical === "") return rows
  const byPid = new Map(rows.flatMap((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid === null ? [] : [[pid, row] as const]
  }))
  const included = new Set<number>()
  for (const row of rows) {
    if (!rowMatchesSearch(row, parsed.query, "os_process", (candidate, field) => processQuantity(candidate, field, ticksPerSecond))) continue
    let pid = asNumber(value(row, "pid"))
    while (pid !== null && !included.has(pid)) {
      included.add(pid)
      const parent = byPid.get(pid)
      pid = parent === undefined ? null : asNumber(value(parent, "process_tree_parent_pid"))
    }
  }
  return rows.filter((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid !== null && included.has(pid)
  })
}

export function buildProcessForest(
  rows: readonly DataRow[],
  memTotalKb: number | null,
  ticksPerSecond: number | null,
): readonly DataRow[] {
  const byPid = new Map<number, DataRow>()
  for (const row of rows) {
    const pid = asNumber(value(row, "pid"))
    if (pid !== null) byPid.set(pid, row)
  }

  const children = new Map<number, number[]>()
  const roots: number[] = []
  for (const [pid, row] of byPid) {
    const ppid = asNumber(value(row, "ppid"))
    if (ppid !== null && ppid !== pid && byPid.has(ppid)) {
      const list = children.get(ppid)
      if (list === undefined) children.set(ppid, [pid])
      else list.push(pid)
    } else {
      roots.push(pid)
    }
  }
  roots.sort((left, right) => left - right)

  const output: DataRow[] = []
  const visited = new Set<number>()

  const walk = (pid: number, ancestorBars: readonly string[], isLastChild: boolean, depth: number): void => {
    if (visited.has(pid)) return
    visited.add(pid)
    const row = byPid.get(pid)
    if (row === undefined) return
    // ps draws one leading space, four columns per ancestor, then `\_`.
    const prefix = depth === 0 ? "" : ` ${ancestorBars.join("")}\\_ `
    output.push({
      ...row,
      values: {
        ...row.values,
        ...metrics(row, memTotalKb, ticksPerSecond),
        process_tree_depth: depth,
        process_tree_parent_pid: depth === 0 ? null : asNumber(value(row, "ppid")),
        process_tree_prefix: prefix,
      },
    })
    const kids = (children.get(pid) ?? []).sort((left, right) => left - right)
    const childBars = depth === 0 ? ancestorBars : [...ancestorBars, isLastChild ? "    " : "|   "]
    kids.forEach((kid, index) => walk(kid, childBars, index === kids.length - 1, depth + 1))
  }

  for (const root of roots) walk(root, [], true, 0)
  // Walk unvisited PIDs so cycles cannot drop rows.
  for (const pid of byPid.keys()) if (!visited.has(pid)) walk(pid, [], true, 0)

  return output
}
