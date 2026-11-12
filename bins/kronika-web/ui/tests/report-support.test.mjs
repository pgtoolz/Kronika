import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { parseDictionary, validateDictionaries } from "../scripts/i18n.mjs"
import { missingReportFeatures, reportSupportLocale } from "../src/report-support.ts"

const source = (path) => readFile(new URL(path, import.meta.url), "utf8")

function withGlobals(descriptors, action) {
  const originals = new Map()
  for (const [name, descriptor] of Object.entries(descriptors)) {
    originals.set(name, Object.getOwnPropertyDescriptor(globalThis, name))
    if (descriptor === undefined) Reflect.deleteProperty(globalThis, name)
    else Object.defineProperty(globalThis, name, { configurable: true, ...descriptor })
  }
  try {
    return action()
  } finally {
    for (const [name, descriptor] of originals) {
      if (descriptor === undefined) Reflect.deleteProperty(globalThis, name)
      else Object.defineProperty(globalThis, name, descriptor)
    }
  }
}

const storage = (locale) => ({
  getItem: (key) => key === "kronika.locale" ? locale : null,
  setItem() {},
  removeItem() {},
})

const browser = (languages) => ({
  language: languages[0] ?? "en-US",
  languages,
})

test("report feature detection sees the supported API set", () => {
  assert.deepEqual(missingReportFeatures(), [])
})

test("report feature detection names missing decompression and WASM APIs", () => {
  withGlobals({ DecompressionStream: { value: undefined, writable: true, enumerable: true } }, () => {
    assert.deepEqual(missingReportFeatures(), ['DecompressionStream("gzip") + ReadableStream.pipeThrough()'])
  })
  const original = Object.getOwnPropertyDescriptor(WebAssembly, "compile")
  try {
    Object.defineProperty(WebAssembly, "compile", { configurable: true, value: undefined, writable: true })
    assert.ok(missingReportFeatures().includes("WebAssembly.compile()"))
  } finally {
    if (original !== undefined) Object.defineProperty(WebAssembly, "compile", original)
  }
})

test("support locale prefers the saved UI locale, then browser languages", () => {
  const getter = (locale) => (key) => storage(locale).getItem(key)
  withGlobals({
    navigator: { value: browser(["en-US"]), writable: true, enumerable: true },
  }, () => assert.equal(reportSupportLocale(getter("ru")), "ru"))
  withGlobals({
    navigator: { value: browser(["ru-RU"]), writable: true, enumerable: true },
  }, () => assert.equal(reportSupportLocale(getter("en")), "en"))
  withGlobals({
    navigator: { value: browser(["ru-RU", "en-US"]), writable: true, enumerable: true },
  }, () => assert.equal(reportSupportLocale(getter(null)), "ru"))
  withGlobals({
    navigator: { value: browser(["de-DE"]), writable: true, enumerable: true },
  }, () => assert.equal(reportSupportLocale(() => { throw new Error("blocked storage") }), "en"))
  withGlobals({ navigator: undefined }, () => assert.equal(reportSupportLocale(getter(null)), "en"))
})

test("offline support messages stay localized, actionable and placeholder-safe", async () => {
  const [englishSource, russianSource] = await Promise.all([
    source("../i18n/en.yaml"),
    source("../i18n/ru.yaml"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  const keys = [
    "support.offline.title",
    "support.offline.body",
    "support.offline.update",
    "support.offline.missing",
    "support.offline.failed",
  ]
  for (const key of keys) {
    assert.ok(english[key]?.length > 0, key)
    assert.ok(russian[key]?.length > 0, key)
  }
  assert.match(english["support.offline.body"], /Safari 16\.4.*macOS 13\.3.*iOS\/iPadOS 16\.4/)
  assert.match(russian["support.offline.body"], /Safari 16\.4.*macOS 13\.3.*iOS\/iPadOS 16\.4/)
  assert.match(english["support.offline.update"], /Update Safari or the operating system/)
  assert.match(russian["support.offline.update"], /Обновите Safari или операционную систему/)
  assert.match(english["support.offline.missing"], /\{features\}/)
  assert.match(russian["support.offline.missing"], /\{features\}/)
  assert.match(russian["support.offline.title"], /[А-Яа-яЁё]/u)
})

test("report support is visible, promise-aware, report-only and UA-agnostic", async () => {
  const [support, app, html, build, buildRs] = await Promise.all([
    source("../src/report-support.ts"),
    source("../src/app.tsx"),
    source("../src/index.html"),
    source("../scripts/build.mjs"),
    source("../../build.rs"),
  ])
  assert.match(support, /export function missingReportFeatures/)
  assert.match(support, /DecompressionStream\("gzip"\)/)
  assert.match(support, /WebAssembly\.compile/)
  assert.match(support, /Blob\.stream/)
  assert.match(support, /BigInt/)
  assert.match(support, /setAttribute\("data-testid", "report-support-error"\)/)
  assert.match(support, /setAttribute\("role", "alert"\)/)
  assert.match(support, /document\.body\.appendChild\(overlay\)/)
  assert.match(support, /promise\.catch\(\(\) => fail\(\)\)/)
  assert.doesNotMatch(support, /userAgent|appVersion|navigator\.vendor/i)
  assert.doesNotMatch(support, /\blocalStorage\b/)
  assert.match(app, /import \{ installReportSupport \} from "\.\/report-support"/)
  assert.match(app, /if \(KRONIKA_REPORT\) \{\s*if \(!installReportSupport\(translation, \(key\) => persistentStorage\.getItem\(key\)\)\) render\(\)/)
  assert.match(app, /void bootstrapSession\(\)\.then\(render\)/)
  assert.equal((html.match(/<script>/g) ?? []).length, 2)
  assert.match(build, /scriptStarts !== 2/)
  assert.match(build, /reportMode \? \["export\."\] : \["support\."\]/)
  assert.match(buildRs, /must contain two complete inline scripts/)
})

test("user documentation declares the support floor and separates headless checks from device checks", async () => {
  const paths = [
    "../../../../README.md",
    "../../../../README.ru.md",
    "../../../../INSTALL.md",
    "../../../../INSTALL.ru.md",
    "../../../../docs/features.md",
    "../../../../docs/features.ru.md",
  ]
  const documents = await Promise.all(paths.map(async (path) => [path, await source(path)]))
  for (const [path, text] of documents) {
    assert.match(text, /Safari 16\.4/, path)
    assert.match(text, /DecompressionStream/, path)
  }
  for (const [path, text] of documents.slice(0, 4)) {
    assert.match(text, /Playwright\s+WebKit/, path)
    assert.match(text, /not covered|separate manual checks|не входят|отдельными/iu, path)
  }
})
