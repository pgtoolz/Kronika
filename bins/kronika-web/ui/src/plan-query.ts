import { useEffect, useMemo, useState } from "react"

import { loadRelatedStatementTextRow, type DataRow, type SegmentBound } from "./api"
import { rawText, value } from "./model"
import { statementTextForPlan } from "./statement-navigation"

export type PlanQueryTextState =
  | { readonly status: "ready"; readonly text: string }
  | { readonly status: "loading" | "unavailable" | "no_bridge" | "error"; readonly text: null }

type LoadedPlanQueryTextState = PlanQueryTextState & { readonly key: string }

export function usePlanQueryText(
  row: DataRow,
  cursor: number,
  segments: readonly SegmentBound[],
  revision: number,
): PlanQueryTextState & { readonly retry: () => void } {
  const target = useMemo(() => statementTextForPlan(row), [row])
  const targetKey = target === null
    ? `no-bridge:${row.typeId}:${row.segmentId}:${row.ordinal}:${row.timestamp}`
    : JSON.stringify([row.typeId, row.segmentId, row.ordinal, cursor, target.queryId, revision, segments])
  const [retryRevision, setRetryRevision] = useState(0)
  const requestKey = `${targetKey}:${retryRevision}`
  const [loaded, setLoaded] = useState<LoadedPlanQueryTextState>(() => target === null
    ? { key: requestKey, status: "no_bridge", text: null }
    : { key: requestKey, status: "loading", text: null })

  useEffect(() => {
    if (target === null) {
      setLoaded({ key: requestKey, status: "no_bridge", text: null })
      return
    }
    const controller = new AbortController()
    void loadRelatedStatementTextRow(segments, cursor, target.queryId, controller.signal)
      .then((row) => {
        if (controller.signal.aborted) return
        const text = row === null ? null : rawText(value(row, "query"))
        setLoaded(text === null
          ? { key: requestKey, status: "unavailable", text: null }
          : { key: requestKey, status: "ready", text })
      })
      .catch(() => {
        if (!controller.signal.aborted) setLoaded({ key: requestKey, status: "error", text: null })
      })
    return () => controller.abort()
  }, [cursor, requestKey, segments, target])

  const current = loaded.key === requestKey
    ? loaded
    : target === null
      ? { key: requestKey, status: "no_bridge" as const, text: null }
      : { key: requestKey, status: "loading" as const, text: null }
  const state: PlanQueryTextState = current.status === "ready"
    ? { status: "ready", text: current.text }
    : { status: current.status, text: null }
  return useMemo(() => ({
    ...state,
    retry: () => setRetryRevision((value) => value + 1),
  }), [state.status, state.text])
}
