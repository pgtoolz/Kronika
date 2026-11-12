import assert from "node:assert/strict"
import { readFile, readdir } from "node:fs/promises"
import test from "node:test"
import { runInNewContext } from "node:vm"

import { loadDisplayTimeZone, saveDisplayTimeZone } from "../src/display-time.ts"
import { readLastExportSeconds, writeLastExportSeconds } from "../src/export-range.ts"
import { persistentStorage, safeStorage } from "../src/storage.ts"

function fakeStorage(values = new Map(), failures = {}) {
  return {
    getItem(key) {
      if (failures.getItem) throw new Error("read blocked")
      return values.get(key) ?? null
    },
    setItem(key, value) {
      if (failures.setItem) throw new Error("write blocked")
      values.set(key, value)
    },
    removeItem(key) {
      if (failures.removeItem) throw new Error("remove blocked")
      values.delete(key)
    },
  }
}

function withGlobalStorage(descriptor, action) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage")
  try {
    Object.defineProperty(globalThis, "localStorage", { configurable: true, ...descriptor })
    return action()
  } finally {
    if (previous === undefined) Reflect.deleteProperty(globalThis, "localStorage")
    else Object.defineProperty(globalThis, "localStorage", previous)
  }
}

test("safeStorage survives a throwing getter and every failing storage operation", () => {
  const getter = safeStorage(() => { throw new Error("opaque origin") })
  assert.equal(getter.getItem("kronika.theme"), null)
  assert.doesNotThrow(() => getter.setItem("kronika.theme", "dark"))
  assert.doesNotThrow(() => getter.removeItem("kronika.theme"))

  const operations = safeStorage(() => fakeStorage(new Map(), {
    getItem: true,
    setItem: true,
    removeItem: true,
  }))
  assert.equal(operations.getItem("kronika.theme"), null)
  assert.doesNotThrow(() => operations.setItem("kronika.theme", "dark"))
  assert.doesNotThrow(() => operations.removeItem("kronika.theme"))

  const incomplete = safeStorage(() => ({}))
  assert.equal(incomplete.getItem("kronika.theme"), null)
  assert.doesNotThrow(() => incomplete.setItem("kronika.theme", "dark"))
  assert.doesNotThrow(() => incomplete.removeItem("kronika.theme"))
})

test("safeStorage delegates reads, writes and invalidation when storage is available", () => {
  const values = new Map([["kronika.timezone", "utc"]])
  const storage = safeStorage(() => fakeStorage(values))
  assert.equal(storage.getItem("kronika.timezone"), "utc")
  storage.setItem("kronika.theme", "dark")
  assert.equal(values.get("kronika.theme"), "dark")
  storage.removeItem("kronika.theme")
  assert.equal(values.has("kronika.theme"), false)
})

test("the shared instance protects the global localStorage getter itself", () => {
  withGlobalStorage({ get() { throw new Error("blocked") } }, () => {
    assert.equal(persistentStorage.getItem("kronika.theme"), null)
    assert.doesNotThrow(() => persistentStorage.setItem("kronika.theme", "dark"))
    assert.doesNotThrow(() => persistentStorage.removeItem("kronika.theme"))
  })
})

test("the shared instance still persists settings when localStorage is available", () => {
  const values = new Map([["kronika.timezone", "utc"]])
  withGlobalStorage({ value: fakeStorage(values) }, () => {
    assert.equal(persistentStorage.getItem("kronika.timezone"), "utc")
    persistentStorage.setItem("kronika.theme", "dark")
    assert.equal(values.get("kronika.theme"), "dark")
    persistentStorage.removeItem("kronika.theme")
    assert.equal(values.has("kronika.theme"), false)
  })
})

test("corrupt preference values fall back without losing current-session writes", () => {
  const values = new Map([
    ["kronika.timezone", "garbage"],
    ["kronika.export-seconds", "garbage"],
  ])
  const storage = safeStorage(() => fakeStorage(values))
  assert.equal(loadDisplayTimeZone(storage), "browser")
  assert.equal(readLastExportSeconds(storage), null)
  saveDisplayTimeZone(storage, "utc")
  writeLastExportSeconds(storage, 8.26)
  assert.equal(values.get("kronika.timezone"), "utc")
  assert.equal(values.get("kronika.export-seconds"), "8.3")
  assert.equal(loadDisplayTimeZone(storage), "utc")
  assert.equal(readLastExportSeconds(storage), 8.3)

  values.set("kronika.timezone", "unexpected")
  values.set("kronika.export-seconds", "-4")
  assert.equal(loadDisplayTimeZone(storage), "browser")
  assert.equal(readLastExportSeconds(storage), null)
})

test("the pre-render theme boot is already guarded against a throwing storage getter", async () => {
  const html = await readFile(new URL("../src/index.html", import.meta.url), "utf8")
  const boot = /<script>([\s\S]*?)<\/script>/.exec(html)?.[1]
  assert.ok(boot, "the inline theme bootstrap script is present")
  assert.ok(boot.startsWith("try{"), "storage access is inside a try block")
  assert.ok(boot.endsWith("}catch{}"), "storage failures are swallowed")
  const dataset = {}
  assert.doesNotThrow(() => runInNewContext(boot, {
    document: { documentElement: { dataset } },
    get localStorage() { throw new Error("blocked") },
  }))
  assert.equal(Object.hasOwn(dataset, "theme"), false)
})

test("all UI storage callers use the guarded shared instance", async () => {
  const src = new URL("../src/", import.meta.url)
  const files = (await readdir(src)).filter((name) => /\.(?:ts|tsx)$/.test(name))
  for (const name of files) {
    if (name === "storage.ts") continue
    const source = await readFile(new URL(name, src), "utf8")
    assert.doesNotMatch(source, /\blocalStorage\b/, `${name} must not touch the storage getter directly`)
  }

  const app = await readFile(new URL("app.tsx", src), "utf8")
  assert.match(app, /import { persistentStorage } from "\.\/storage"/)
  assert.match(app, /loadDisplayTimeZone\(persistentStorage\)/)
  assert.match(app, /saveDisplayTimeZone\(persistentStorage, displayZone\)/)
  assert.match(app, /persistentStorage\.setItem\("kronika\.locale", locale\)/)
  assert.match(app, /persistentStorage\.setItem\("kronika\.theme", theme\)/)
  assert.match(app, /persistentStorage\.getItem\(LOAD_SECONDS_KEY\)/)
  assert.match(app, /persistentStorage\.setItem\(LOAD_SECONDS_KEY,/)
  assert.match(app, /persistentStorage\.getItem\("kronika\.theme"\)/)
  assert.match(app, /persistentStorage\.getItem\("kronika\.locale"\)/)

  const activity = await readFile(new URL("activity.tsx", src), "utf8")
  assert.match(activity, /import { persistentStorage } from "\.\/storage"/)
  assert.match(activity, /persistentStorage\.getItem\(storageKey\)/)
  assert.match(activity, /persistentStorage\.setItem\(storageKey, open \? "1" : "0"\)/)

  const exportDialog = await readFile(new URL("export-dialog.tsx", src), "utf8")
  assert.match(exportDialog, /import { persistentStorage } from "\.\/storage"/)
  assert.match(exportDialog, /readLastExportSeconds\(persistentStorage\)/)
  assert.match(exportDialog, /writeLastExportSeconds\(persistentStorage, seconds\)/)

  const inspector = await readFile(new URL("inspector.tsx", src), "utf8")
  assert.match(inspector, /import { persistentStorage } from "\.\/storage"/)
  assert.match(inspector, /loadInspectorWidth\(persistentStorage\)/)
  assert.match(inspector, /persistentStorage\.setItem\(WIDTH_KEY, String\(width\)\)/)
  assert.match(inspector, /Number\.parseInt\(storage\.getItem\(WIDTH_KEY\) \?\? "", 10\)/)
  assert.match(inspector, /return Number\.isFinite\(stored\) \? clampWidth\(stored\) : DEFAULT_WIDTH/)
})
