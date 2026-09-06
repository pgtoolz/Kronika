import { ChartLine, Search, X } from "lucide-react"

import type { Translate } from "./help"

// Phone chrome. The plot and the search form are the two tallest controls on a
// surface and neither is read continuously, so below 520px they become buttons
// here and open at full size. The controls themselves are unchanged: the chart
// is the Inspector's, the search is the surface's own, promoted to a panel.
export function MobileControls({
  filtered,
  onOpenChart,
  onSearch,
  searchOpen,
  t,
}: {
  readonly filtered: boolean
  readonly onOpenChart: () => void
  readonly onSearch: (open: boolean) => void
  readonly searchOpen: boolean
  readonly t: Translate
}) {
  return <>
    <div className="mobile-controls" data-testid="mobile-controls">
      <button className="coarse:min-h-11" onClick={onOpenChart} type="button"><ChartLine aria-hidden="true" size={13} />{t("inspector.chart")}</button>
      <button aria-pressed={filtered} className="coarse:min-h-11" data-testid="mobile-search-open" onClick={() => onSearch(true)} type="button"><Search aria-hidden="true" size={13} />{t("filter.short")}</button>
    </div>
    {searchOpen && <div className="mobile-panel-head" data-testid="mobile-search-head">
      <button aria-label={t("common.close")} onClick={() => onSearch(false)} type="button"><X aria-hidden="true" size={16} /></button>
      <strong>{t("filter.label")}</strong>
    </div>}
  </>
}
