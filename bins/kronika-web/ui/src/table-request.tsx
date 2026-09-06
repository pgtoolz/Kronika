import type { Translate } from "./help"

export type SnapshotRequestPhase = "ready" | "loading" | "missing"

export interface SnapshotRequestState {
  readonly target: string | null
  readonly phase: SnapshotRequestPhase
}

export type TableRequestPhase = "ready" | "pending" | "error"

export type TableRequestState =
  | { readonly phase: "ready" }
  | { readonly phase: "pending" | "error"; readonly retained: boolean }

export const READY_SNAPSHOT_REQUEST: SnapshotRequestState = { target: null, phase: "ready" }
export const READY_TABLE_REQUEST: TableRequestState = { phase: "ready" }

export function beginSnapshotRequest(target: string | null): SnapshotRequestState {
  return { target, phase: "loading" }
}

export function settleSnapshotRequest(
  current: SnapshotRequestState,
  target: string | null,
  phase: Exclude<SnapshotRequestPhase, "loading">,
): SnapshotRequestState {
  return current.target === target ? { target, phase } : current
}

// A target change is pending in the render before its effect starts. Keeping
// the target with the phase also prevents an older settlement from describing
// the request currently visible in the address.
export function visibleSnapshotRequest(
  current: SnapshotRequestState,
  target: string | null,
): SnapshotRequestPhase {
  return current.target === target ? current.phase : "loading"
}

export function snapshotRowsVisible(
  storedTarget: string | null,
  target: string | null,
  storedOwner: string | null,
  owner: string,
  retainsDenseRows: boolean,
): boolean {
  return storedTarget === target || retainsDenseRows || target !== null && storedOwner === owner
}

export function tableRequestPhase(
  snapshot: SnapshotRequestPhase,
  page: "idle" | "loading" | "error",
): TableRequestPhase {
  if (snapshot === "loading" || page === "loading") return "pending"
  if (snapshot === "missing" || page === "error") return "error"
  return "ready"
}

export function tableRequestState(phase: TableRequestPhase, retained: boolean): TableRequestState {
  return phase === "ready" ? READY_TABLE_REQUEST : { phase, retained }
}

export function TableRequestMessage({ request, t }: {
  readonly request: TableRequestState
  readonly t: Translate
}) {
  if (request.phase === "ready") return null
  const pending = request.phase === "pending"
  const key = `table.${pending ? "loading" : "load_failed"}${request.retained ? "_retained" : ""}`
  return <span aria-live={pending ? "polite" : undefined} className={`flex min-w-0 flex-1 items-center gap-2${pending ? "" : " text-bad"}`} role={pending ? "status" : "alert"}>
    {pending && <progress aria-hidden="true" style={{ width: 44 }} />}
    <span className="min-w-0 overflow-hidden text-ellipsis">{t(key)}</span>
  </span>
}

export function TableRequestPlaceholder({ className, empty, phase, t, testId }: {
  readonly className?: string | undefined
  readonly empty: string
  readonly phase: TableRequestPhase
  readonly t: Translate
  readonly testId?: string | undefined
}) {
  const request = tableRequestState(phase, false)
  return <div
    aria-busy={request.phase === "pending"}
    className={`table-empty box-border flex h-[72px] min-w-0 items-center overflow-hidden${className === undefined ? "" : ` ${className}`}`}
    data-testid={testId}
  >
    {request.phase === "ready" ? empty : <TableRequestMessage request={request} t={t} />}
  </div>
}
