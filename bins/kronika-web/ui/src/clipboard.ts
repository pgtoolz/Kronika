const MANUAL_COPY = "[data-kronika-manual-copy]"
const MODAL_SURFACE = 'dialog:modal, [role="dialog"][aria-modal="true"]'

// Copies through the Clipboard API where it works, falling back to the
// legacy copy event — the only automatic route on plain-http origins,
// Kronika's normal LAN deployment, and the second chance when the API
// rejects (unfocused document, denied permission).
export async function copyText(text: string, manualInstruction: string): Promise<boolean> {
  const previous = document.activeElement
  document.querySelector(MANUAL_COPY)?.remove()
  if (navigator.clipboard !== undefined) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // fall through to the selection path
    }
  }
  const modal = previous instanceof Element ? previous.closest(MODAL_SURFACE) : null
  const target = modal ?? document.querySelector(MODAL_SURFACE) ?? document.body
  const area = document.createElement("textarea")
  area.value = text
  area.readOnly = true
  area.tabIndex = -1
  area.style.position = "fixed"
  area.style.opacity = "0"
  area.style.pointerEvents = "none"
  target.append(area)
  let exactCopyEvent = false
  const onCopy = (event: ClipboardEvent) => {
    if (event.clipboardData === null) return
    try {
      event.clipboardData.setData("text/plain", text)
      event.preventDefault()
      exactCopyEvent = event.defaultPrevented && event.clipboardData.getData("text/plain") === text
    } catch {
      exactCopyEvent = false
    }
  }
  area.addEventListener("copy", onCopy, { once: true })
  area.focus({ preventScroll: true })
  area.select()
  area.setSelectionRange(0, text.length)
  const exactSelection = document.activeElement === area
    && area.selectionStart === 0
    && area.selectionEnd === text.length
  let commandSucceeded = false
  try {
    if (exactSelection) commandSucceeded = document.execCommand("copy")
  } catch {
    commandSucceeded = false
  }
  area.removeEventListener("copy", onCopy)
  if (commandSucceeded && exactCopyEvent) {
    area.remove()
    if (previous instanceof HTMLElement && previous.isConnected) previous.focus({ preventScroll: true })
    return true
  }

  const fallback = document.createElement("label")
  fallback.dataset.kronikaManualCopy = ""
  fallback.dataset.testid = "manual-copy-fallback"
  fallback.className = "fixed bottom-3 right-3 z-[110] grid w-[min(25rem,calc(100vw-1.5rem))] gap-1.5 rounded-[var(--radius-sm)] border border-warn bg-s1 p-2 text-xs leading-[1.45] text-fg2"
  const note = document.createElement("span")
  note.setAttribute("role", "alert")
  note.textContent = manualInstruction
  area.removeAttribute("style")
  area.className = "min-h-24 w-full resize-y rounded-[var(--radius-xs)] border border-line3 bg-s2 p-2 font-mono text-xs leading-[1.45] text-fg"
  area.dataset.testid = "manual-copy-text"
  area.setAttribute("aria-label", manualInstruction)
  area.spellcheck = false
  area.tabIndex = 0
  fallback.append(note, area)
  target.append(fallback)
  fallback.addEventListener("focusout", (event) => {
    if (!(event.relatedTarget instanceof Node) || !fallback.contains(event.relatedTarget)) fallback.remove()
  })
  area.focus({ preventScroll: true })
  area.select()
  area.setSelectionRange(0, text.length)
  return false
}
