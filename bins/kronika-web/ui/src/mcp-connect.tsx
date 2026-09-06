import { Check, Copy } from "lucide-react"
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react"

import { copyText } from "./clipboard"
import type { Translate } from "./help"
import { type Auth, claudePrompt, codexPrompt, credentialsKey, cursorPrompt } from "./mcp-prompts"
import { apiFetch } from "./session"

const CLIENTS = [
  { builder: claudePrompt, id: "claude", label: "Claude Code", sectionKey: "mcp.section.claude" },
  { builder: codexPrompt, id: "codex", label: "Codex CLI", sectionKey: "mcp.section.codex" },
  { builder: cursorPrompt, id: "cursor", label: "Cursor", sectionKey: "mcp.section.cursor" },
] as const

export function McpPanel({ database, onClose, t }: {
  readonly database: string | null
  readonly onClose: () => void
  readonly t: Translate
}) {
  const dialog = useRef<HTMLDialogElement>(null)
  const closeButton = useRef<HTMLButtonElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const titleId = useId()
  const [auth, setAuth] = useState<Auth | null>(null)
  const [client, setClient] = useState<(typeof CLIENTS)[number]["label"]>("Claude Code")
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return undefined
    const timer = window.setTimeout(() => setCopied(false), 2_000)
    return () => window.clearTimeout(timer)
  }, [copied])
  useEffect(() => {
    const controller = new AbortController()
    apiFetch("/api/mcp-access", { signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) throw new Error("mcp-access unavailable")
        const body: { authorization?: string | null } = await response.json()
        setAuth(
          typeof body.authorization === "string"
            ? { kind: "header", value: body.authorization }
            : { kind: "open" },
        )
      })
      .catch(() => setAuth({ kind: "placeholder" }))
    return () => controller.abort()
  }, [])
  useLayoutEffect(() => {
    const active = document.activeElement
    if (active instanceof HTMLElement) opener.current = active
    dialog.current?.showModal()
    closeButton.current?.focus({ preventScroll: true })
    return () => {
      if (dialog.current?.open) dialog.current.close()
      const target = opener.current
      requestAnimationFrame(() => {
        if (target?.isConnected) target.focus({ preventScroll: true })
      })
    }
  }, [])

  const dismiss = () => onClose()

  const url = `${window.location.origin}/mcp`
  const selected = CLIENTS.find((candidate) => candidate.label === client) ?? CLIENTS[0]
  const prompt = auth === null ? null : selected.builder(url, auth, t, database)
  return (
    <dialog aria-labelledby={titleId} aria-modal="true" className="mcp-dialog fixed bottom-0 left-auto right-0 top-0 z-[100] m-0 h-auto max-h-none w-[min(92vw,430px)] max-w-[430px] overflow-auto border-0 border-l border-line4 bg-s1 p-[18px] text-fg shadow-[-20px_0_50px_var(--color-shadow-a)]" data-testid="mcp-panel" onCancel={(event) => { event.preventDefault(); dismiss() }} onKeyDown={(event) => event.stopPropagation()} ref={dialog} role="dialog">
      <div className="flex items-center justify-between">
        <div>
          <p className="m-0 text-xs text-fg4">MCP</p>
          <h2 id={titleId}>{t("mcp.title")}</h2>
        </div>
        <button aria-label={t("mcp.close")} className="icon-button" onClick={dismiss} ref={closeButton} type="button">×</button>
      </div>
      <p className="border border-line3 bg-s2 p-2 text-sm text-accent2">{t("mcp.endpoint")}: <code>{url}</code></p>
      {auth !== null && <p className="text-sm leading-[1.6] text-fg3">{t(credentialsKey(auth))}</p>}
      <div aria-label={t("mcp.client_label")} className="lens-tabs mt-2 w-full [&>button]:min-w-0 [&>button]:flex-1 [&>button]:px-1" role="group">
        {CLIENTS.map((candidate) => (
          <button aria-pressed={client === candidate.label} data-testid={`mcp-tab-${candidate.id}`} key={candidate.id} onClick={() => { setClient(candidate.label); setCopied(false) }} type="button">{candidate.label}</button>
        ))}
      </div>
      {prompt !== null && (
        <section className="mt-3">
          <p className="mb-1 mt-0 text-xs text-fg4">{t(selected.sectionKey)}</p>
          <pre className="m-0 select-all whitespace-pre-wrap break-words rounded-[var(--radius-sm)] border border-line3 bg-s2 p-2 text-xs leading-[1.5] text-fg2">{prompt}</pre>
          <button aria-label={`${t("mcp.copy")} — ${selected.label}`} className="mt-2 inline-flex w-full cursor-pointer items-center justify-center gap-1.5 rounded-[var(--radius-sm)] border border-line3 bg-s2 px-2 py-1.5 text-sm font-medium text-accent3 transition-colors hover:bg-s3" onClick={() => void copyText(prompt, t("clipboard.manual")).then(setCopied)} type="button">{copied ? <Check aria-hidden="true" size={14} /> : <Copy aria-hidden="true" size={14} />}{t(copied ? "mcp.copied" : "mcp.copy")}</button>
        </section>
      )}
      <p className="mt-3 text-xs text-fg4">{t("mcp.docs")}</p>
    </dialog>
  )
}
