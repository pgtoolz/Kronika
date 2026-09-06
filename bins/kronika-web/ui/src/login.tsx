import { Activity, LockKeyhole } from "lucide-react"
import { useEffect, useRef, useState, type FormEvent } from "react"

import type { Translate } from "./help"
import type { Locale } from "./model"
import { signInBasic } from "./session"

export function Login({ expired, locale, onLocale, t }: {
  readonly expired: boolean
  readonly locale: Locale
  readonly onLocale: (locale: Locale) => void
  readonly t: Translate
}) {
  const [user, setUser] = useState("")
  const [password, setPassword] = useState("")
  const [error, setError] = useState<"invalid" | "unavailable" | null>(null)
  const [busy, setBusy] = useState(false)
  const request = useRef<AbortController | null>(null)
  useEffect(() => () => request.current?.abort(), [])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    request.current?.abort()
    const controller = new AbortController()
    request.current = controller
    setBusy(true)
    setError(null)
    setPassword("")
    try {
      const result = await signInBasic(user, password, controller.signal)
      if (controller.signal.aborted) return
      if (result === "invalid") {
        setError("invalid")
      }
    } catch {
      if (!controller.signal.aborted) {
        setError("unavailable")
      }
    } finally {
      if (!controller.signal.aborted) setBusy(false)
    }
  }

  const message = error === null ? expired ? t("auth.expired") : null : t(`auth.${error}`)
  return <main className="grid min-h-screen items-center p-6 max-[760px]:p-2.5">
    <section className="mx-auto w-full max-w-[440px] rounded-[var(--radius-lg)] border border-line2 bg-s1 p-5 shadow-[var(--shadow-pop)] max-[760px]:p-4" data-testid="login-card">
      <header className="flex min-h-[30px] items-center border-b border-line2 pb-3">
        <span className="mr-2 flex items-center text-accent2"><Activity aria-hidden="true" size={18} /></span>
        <strong className="text-sm font-semibold tracking-[.06em] text-fg-hi">{t("app.title")}</strong>
        <div aria-label={t("locale.switch")} className="locale-switch ml-auto" role="group">
          {(["ru", "en"] as const).map((choice) => <button aria-pressed={locale === choice} key={choice} onClick={() => onLocale(choice)} type="button">{t(`locale.${choice}`)}</button>)}
        </div>
      </header>
      <div className="grid grid-cols-[22px_minmax(0,1fr)] items-start gap-[11px] px-0.5 pb-[18px] pt-6">
        <LockKeyhole aria-hidden="true" className="mt-px text-accent2" size={20} />
        <div>
          <h1 className="text-xl">{t("auth.title")}</h1>
          <p className="mt-[7px] text-sm leading-[1.55] text-fg3">{t("auth.subtitle")}</p>
        </div>
      </div>
      <form autoComplete="off" className="grid gap-3" onSubmit={(event) => { void submit(event) }}>
        <label>
          <span className="mb-1.5 block text-xs font-medium text-fg3">{t("auth.user")}</span>
          <input autoCapitalize="none" autoComplete="username" autoFocus className="h-9 w-full rounded-[var(--radius-sm)] border border-line3 bg-bg px-2.5 text-fg outline-none transition-colors focus:border-accent focus:shadow-[0_0_0_1px_var(--color-accent-line)]" disabled={busy} name="username" onChange={(event) => setUser(event.target.value)} required spellCheck={false} type="text" value={user} />
        </label>
        <label>
          <span className="mb-1.5 block text-xs font-medium text-fg3">{t("auth.password")}</span>
          <input autoComplete="current-password" className="h-9 w-full rounded-[var(--radius-sm)] border border-line3 bg-bg px-2.5 text-fg outline-none transition-colors focus:border-accent focus:shadow-[0_0_0_1px_var(--color-accent-line)]" disabled={busy} name="password" onChange={(event) => setPassword(event.target.value)} required type="password" value={password} />
        </label>
        {message !== null && <p aria-live="polite" className="border-l-2 border-warn px-2 py-[5px] text-sm leading-[1.45] text-fg2" data-testid="login-message">{message}</p>}
        <button className="mt-0.5 h-9 cursor-pointer rounded-[var(--radius-sm)] border-0 bg-accent text-sm font-semibold text-bg transition-colors hover:bg-accent2 disabled:cursor-wait disabled:opacity-65" disabled={busy} type="submit">{t(busy ? "auth.signing_in" : "auth.submit")}</button>
      </form>
    </section>
  </main>
}
