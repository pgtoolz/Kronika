import { useEffect, useLayoutEffect, useRef, type RefObject } from "react"

const PRIORITY_ESCAPE_TARGET = '[role="dialog"], [role="tooltip"], [data-testid="help-panel"]'

export function detailEscapeDismisses(key: string, defaultPrevented: boolean, priorityOpen: boolean): boolean {
  return key === "Escape" && !defaultPrevented && !priorityOpen
}

export function useDetailDismiss(onClose: () => void, identity: string): RefObject<HTMLElement | null> {
  const detail = useRef<HTMLElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const close = useRef(onClose)

  useLayoutEffect(() => {
    close.current = onClose
  }, [onClose])

  useLayoutEffect(() => {
    const active = document.activeElement
    if (active instanceof HTMLElement && active !== document.body && !detail.current?.contains(active)) opener.current = active
  }, [identity])

  useEffect(() => {
    const dismiss = (event: KeyboardEvent) => {
      const priorityOpen = document.querySelector(PRIORITY_ESCAPE_TARGET) !== null
      if (!detailEscapeDismisses(event.key, event.defaultPrevented, priorityOpen)) return
      event.preventDefault()
      const target = opener.current
      close.current()
      requestAnimationFrame(() => {
        if (target?.isConnected) target.focus({ preventScroll: true })
      })
    }
    window.addEventListener("keydown", dismiss)
    return () => window.removeEventListener("keydown", dismiss)
  }, [])

  return detail
}
