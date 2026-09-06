import { createContext, useContext, useMemo, type ReactNode } from "react"

// The Export dialog describes a selection on the hour timeline, so the
// timeline draws that selection while the dialog is open. The app owns the
// range; views need no new props.
export interface ExportSelection {
  readonly from: number
  readonly to: number
}

const ExportContext = createContext<ExportSelection | null>(null)

export function ExportProvider({ children, selection }: {
  readonly children: ReactNode
  readonly selection: ExportSelection | null
}) {
  const value = useMemo(() => selection, [selection])
  return <ExportContext.Provider value={value}>{children}</ExportContext.Provider>
}

export function useExportSelection(): ExportSelection | null {
  return useContext(ExportContext)
}
