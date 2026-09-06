import { Select } from "@base-ui/react/select"
import { ChevronDown, Globe } from "lucide-react"

import type { DisplayTimeZone } from "./display-time"
import type { Translate } from "./help"

const ZONES = ["browser", "utc"] as const satisfies readonly DisplayTimeZone[]

// The native select sized its popup by OS rules and clipped the Russian labels
// inside a fixed-width box; the trigger here shrinks to the content and the
// popup is ours, so both locales render in full.
export function TimezoneSelect({ mode, setMode, t }: {
  readonly mode: DisplayTimeZone
  readonly setMode: (mode: DisplayTimeZone) => void
  readonly t: Translate
}) {
  return (
    <Select.Root<DisplayTimeZone> onValueChange={(value) => { if (value !== null) setMode(value) }} value={mode}>
      <Select.Trigger aria-label={t("timezone.switch")} className="timezone-select flex h-7 cursor-pointer items-center gap-1.5 rounded-[var(--radius-sm)] border-0 bg-transparent px-2 text-xs font-medium text-fg3 transition-colors hover:bg-s3 hover:text-fg" data-testid="timezone-select" data-value={mode}>
        <Globe aria-hidden="true" size={13} />
        <Select.Value className="overflow-hidden text-ellipsis whitespace-nowrap" data-testid="timezone-value">{() => t(`timezone.${mode}`)}</Select.Value>
        <Select.Icon aria-hidden="true" className="flex text-fg4"><ChevronDown size={12} /></Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner alignItemWithTrigger={false} className="z-[1150]" side="bottom" sideOffset={4}>
          <Select.Popup className="min-w-[var(--trigger-width)] rounded-[var(--radius-md)] border border-line3 bg-s1 p-1 shadow-[var(--shadow-pop)]">
            {ZONES.map((zone) => (
              <Select.Item className="cursor-pointer select-none whitespace-nowrap rounded-[var(--radius-xs)] px-2 py-1.5 text-xs text-fg2 outline-none data-[highlighted]:bg-s3 data-[highlighted]:text-fg data-[selected]:font-medium data-[selected]:text-accent3" data-testid={`timezone-option-${zone}`} key={zone} value={zone}>
                <Select.ItemText>{t(`timezone.${zone}`)}</Select.ItemText>
              </Select.Item>
            ))}
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  )
}
