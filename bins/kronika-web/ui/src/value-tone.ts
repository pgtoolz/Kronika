import type { Cell, DataRow } from "./api"

export type ValueTone = "good" | "warning" | "critical" | "inactive"

export function semanticValueTone(field: string, cell: Cell, rate = false, row?: DataRow): ValueTone | null {
  const text = typeof cell === "string" ? cell.trim() : null
  // process_stat is the ps STAT string: the state letter carries the tone,
  // the flags after it do not.
  if (field === "state" || field === "process_stat") {
    if (text === "idle in transaction (aborted)") return "critical"
    if (text === "idle in transaction") return "warning"
    // Process state may be a character or its ASCII code.
    const stateChar = (text ?? asciiChar(cell))?.charAt(0)
    if (stateChar === "R") return "good"
    if (stateChar === "D") return "warning"
    if (stateChar === "Z") return "critical"
    if (stateChar === "I") return "inactive"
  }
  if (field === "wait_event_type" && text !== null && text !== "") return "warning"

  const number = numericCell(cell)
  if (number === null) return null
  if (rate && number === 0) return "inactive"

  if ((field === "query_duration_ms" || field === "transaction_duration_ms")
    && row !== undefined
    && !isActiveClient(row)) return null

  switch (field) {
    case "mean_exec_ms_per_call":
    case "mean_exec_time_ms":
      return number >= 5_000 ? "critical" : null
    case "query_duration_ms":
      if (number >= 5_000) return "critical"
      return number >= 1_000 ? "warning" : null
    case "transaction_duration_ms":
      if (number >= 60_000) return "critical"
      return number >= 5_000 ? "warning" : null
    case "hit_pct":
      if (number < 90) return "critical"
      return number < 99 ? "warning" : "good"
    case "cv":
      if (number < 1) return "good"
      return number < 3 ? "warning" : "critical"
    case "plan_time_pct":
      if (number < 50) return "good"
      return number < 80 ? "warning" : "critical"
    case "cpu_percent":
      if (number >= 90) return "critical"
      return number >= 50 ? "warning" : null
    default:
      return null
  }
}

function isActiveClient(row: DataRow): boolean {
  return textCell(row.values.backend_type) === "client backend" && textCell(row.values.state) === "active"
}

function textCell(cell: Cell | undefined): string | null {
  return typeof cell === "string" || typeof cell === "number" || typeof cell === "boolean"
    ? String(cell).trim()
    : null
}

function asciiChar(cell: Cell): string | null {
  return typeof cell === "number" && Number.isFinite(cell) ? String.fromCharCode(cell) : null
}

function numericCell(cell: Cell): number | null {
  if (typeof cell === "number") return Number.isFinite(cell) ? cell : null
  if (typeof cell !== "string" || cell.trim() === "") return null
  const number = Number(cell)
  return Number.isFinite(number) ? number : null
}
