import type { DataRow } from "./api"
import { asNumber, value } from "./model"
import { parseSearch, rowMatchesSearch } from "./search"

// Build parent-first order while keeping disconnected lock chains contiguous.

function parsePid(row: DataRow): number | null {
  const cell = value(row, "pid")
  return typeof cell === "number" && Number.isFinite(cell) ? cell : null
}

// blocked_by=0 denotes a prepared transaction and has no row or tree edge.
function parseBlockers(row: DataRow): readonly number[] {
  const cell = value(row, "blocked_by")
  return Array.isArray(cell) ? cell.filter((entry): entry is number => typeof entry === "number" && entry !== 0) : []
}

export function buildLockForest(rows: readonly DataRow[]): readonly DataRow[] {
  const byPid = new Map<number, DataRow>()
  for (const row of rows) {
    const pid = parsePid(row)
    if (pid !== null) byPid.set(pid, row)
  }

  const preparedWaiters = new Set<number>()
  const blockersOf = new Map<number, readonly number[]>()
  const children = new Map<number, number[]>()
  for (const row of rows) {
    const pid = parsePid(row)
    if (pid === null) continue
    const rawCell = value(row, "blocked_by")
    if (Array.isArray(rawCell) && rawCell.includes(0)) preparedWaiters.add(pid)
    const blockers = parseBlockers(row).filter((blocker) => byPid.has(blocker))
    blockersOf.set(pid, blockers)
    for (const blocker of blockers) {
      const list = children.get(blocker)
      if (list === undefined) children.set(blocker, [pid])
      else list.push(pid)
    }
  }

  // Union-find keeps disconnected chains from interleaving.
  const parent = new Map<number, number>()
  const find = (pid: number): number => {
    let root = pid
    while (parent.get(root) !== root) root = parent.get(root) as number
    return root
  }
  for (const pid of byPid.keys()) parent.set(pid, pid)
  for (const [pid, blockers] of blockersOf) {
    for (const blocker of blockers) {
      const left = find(pid)
      const right = find(blocker)
      if (left !== right) parent.set(left, right)
    }
  }

  const components = new Map<number, number[]>()
  for (const pid of byPid.keys()) {
    const root = find(pid)
    const list = components.get(root)
    if (list === undefined) components.set(root, [pid])
    else list.push(pid)
  }

  const output: DataRow[] = []
  const visited = new Set<number>()

  const walk = (pid: number, parentPid: number | null, depth: number, ancestorBars: readonly string[], isLastChild: boolean): void => {
    if (visited.has(pid)) return
    visited.add(pid)
    const row = byPid.get(pid)
    if (row === undefined) return
    const blockers = blockersOf.get(pid) ?? []
    const extraBlockers = parentPid === null ? blockers : blockers.filter((blocker) => blocker !== parentPid)
    const prefix = depth === 1 ? "" : ancestorBars.join("") + (isLastChild ? "└─ " : "├─ ")
    output.push({
      ...row,
      values: {
        ...row.values,
        lock_tree_depth: depth,
        lock_tree_parent_pid: parentPid,
        lock_tree_prefix: prefix,
        lock_tree_extra_blockers: extraBlockers,
        lock_tree_waits_on_prepared: preparedWaiters.has(pid),
      },
    })
    const kids = (children.get(pid) ?? []).filter((kid) => !visited.has(kid)).sort((left, right) => left - right)
    const childBars = depth === 1 ? ancestorBars : [...ancestorBars, isLastChild ? "   " : "│  "]
    kids.forEach((kid, index) => walk(kid, pid, depth + 1, childBars, index === kids.length - 1))
  }

  for (const componentRoot of [...components.keys()].sort((left, right) => left - right)) {
    const members = (components.get(componentRoot) as number[]).slice().sort((left, right) => left - right)
    const roots = members.filter((pid) => (blockersOf.get(pid) ?? []).length === 0)
    // Cycles have no blocker-free root; start at the lowest PID.
    for (const root of roots.length > 0 ? roots : [members[0] as number]) walk(root, null, 1, [], true)
    for (const pid of members) if (!visited.has(pid)) walk(pid, null, 1, [], true)
  }

  return output
}

export function filterLockForest(rows: readonly DataRow[], pattern: string): readonly DataRow[] {
  const parsed = parseSearch(pattern, "pg_locks")
  if (!parsed.ok || parsed.query.canonical === "") return rows
  const byPid = new Map(rows.flatMap((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid === null ? [] : [[pid, row] as const]
  }))
  const included = new Set<number>()
  for (const row of rows) {
    if (!rowMatchesSearch(row, parsed.query, "pg_locks")) continue
    const pid = asNumber(value(row, "pid"))
    const extraCell = value(row, "lock_tree_extra_blockers")
    const pending = [pid, ...(Array.isArray(extraCell) ? extraCell.map((entry) => typeof entry === "number" ? entry : null) : [])]
    while (pending.length > 0) {
      const current = pending.pop() ?? null
      if (current === null || included.has(current)) continue
      included.add(current)
      const parent = byPid.get(current)
      pending.push(parent === undefined ? null : asNumber(value(parent, "lock_tree_parent_pid")))
    }
  }
  return rows.filter((row) => {
    const pid = asNumber(value(row, "pid"))
    return pid !== null && included.has(pid)
  })
}
