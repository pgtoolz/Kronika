import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { runInNewContext } from "node:vm"
import { build } from "esbuild"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

const directory = dirname(fileURLToPath(import.meta.url))
const template = await readFile(new URL("../src/index.html", import.meta.url), "utf8")
const boot = template.match(/<script>(.*?)<\/script>/s)?.[1]
if (boot === undefined) throw new Error("login theme boot script is missing")
const compiled = await build({
  bundle: true,
  external: ["react", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  stdin: { contents: 'export { Login } from "../src/login.tsx"', loader: "tsx", resolveDir: directory },
  write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const { Login } = loaded.exports
const copy = {
  "app.title": "KRONIKA",
  "auth.expired": "Your session ended. Sign in again.",
  "auth.password": "Password",
  "auth.submit": "Sign in",
  "auth.subtitle": "Credentials stay in memory.",
  "auth.title": "Sign in to Kronika",
  "auth.user": "Username",
  "locale.en": "EN",
  "locale.ru": "RU",
  "locale.switch": "Language",
}
const t = (key) => copy[key] ?? key

test("the login shell is an in-app bilingual form without embedded credentials", () => {
  const html = renderToStaticMarkup(createElement(Login, { expired: true, locale: "en", onLocale: () => {}, t }))
  assert.match(html, />Username</)
  assert.match(html, />Password</)
  assert.match(html, />RU</)
  assert.match(html, />EN</)
  assert.match(html, /Your session ended/)
  assert.doesNotMatch(html, /<form[^>]+action=/)
  assert.doesNotMatch(html, /name="password"[^>]+value="[^"].*"/)
})

test("submitting removes the password from React state before the request settles", async () => {
  const source = await readFile(new URL("../src/login.tsx", import.meta.url), "utf8")
  assert.ok(source.indexOf('setPassword("")') < source.indexOf("await signInBasic"))
})

test("the persisted login theme is applied before styles without writing storage", () => {
  assert.ok(template.indexOf("<script>") < template.indexOf("<style>"))
  for (const theme of ["light", "dark"]) {
    const dataset = {}
    let reads = 0
    let writes = 0
    runInNewContext(boot, {
      document: { documentElement: { dataset } },
      localStorage: {
        get ["kronika.theme"]() {
          reads += 1
          return theme
        },
        setItem() { writes += 1 },
      },
    })
    assert.equal(dataset.theme, theme)
    assert.equal(reads, 1)
    assert.equal(writes, 0)
  }
})

test("login theme boot ignores missing, invalid, and unavailable storage", () => {
  for (const saved of [null, "auto", "LIGHT"]) {
    const dataset = {}
    runInNewContext(boot, {
      document: { documentElement: { dataset } },
      localStorage: { "kronika.theme": saved },
    })
    assert.equal(Object.hasOwn(dataset, "theme"), false)
  }
  assert.doesNotThrow(() => runInNewContext(boot, {
    document: { documentElement: { dataset: {} } },
    localStorage: { get ["kronika.theme"]() { throw new Error("blocked") } },
  }))
})
