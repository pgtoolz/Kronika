import type { ReactNode } from "react"

export type DetailValueRole = "semantic" | "machine"

// Ordinary operator detail docks all use this compact term/value composition.
// Tile dashboards and prose definitions are deliberately separate patterns.
export function DetailList({ children, className }: { readonly children: ReactNode; readonly className?: string | undefined }) {
  return <dl className={`m-0 mt-2${className === undefined ? "" : ` ${className}`}`}>{children}</dl>
}

export function DetailRow({ children, term, valueRole = "semantic" }: {
  readonly children: ReactNode
  readonly term: ReactNode
  readonly valueRole?: DetailValueRole | undefined
}) {
  return <div className="detail-row max-[520px]:detail-row-stacked">
    <dt className="detail-dt">{term}</dt>
    <dd className={valueRole === "machine" ? "detail-dd detail-dd-machine" : "detail-dd"} data-value-role={valueRole}>{children}</dd>
  </div>
}
