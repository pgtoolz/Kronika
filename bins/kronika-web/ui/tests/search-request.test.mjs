import assert from "node:assert/strict"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import test from "node:test"

import { importFile } from "./import-module.mjs"

const request = await importFile("../src/search-request.tsx")
const copy = {
  en: {
    "filter.searching": "Searching…",
    "filter.searching_retained": "Searching… Rows retained.",
    "filter.search_failed": "Search failed.",
    "filter.search_failed_retained": "Search failed. Rows retained.",
  },
  ru: {
    "filter.searching": "Идёт поиск…",
    "filter.searching_retained": "Идёт поиск… Строки сохранены.",
    "filter.search_failed": "Ошибка поиска.",
    "filter.search_failed_retained": "Ошибка поиска. Строки сохранены.",
  },
}

test("pending search renders an unmistakable localized live status without a fake percentage", () => {
  for (const locale of ["en", "ru"]) {
    const state = request.beginSearchRequest("os_process", false)
    const markup = renderToStaticMarkup(createElement(request.SearchRequestMessage, {
      request: state,
      t: (key) => copy[locale][key] ?? key,
    }))
    assert.match(markup, /role="status"/)
    assert.match(markup, /aria-live="polite"/)
    assert.match(markup, /<progress/)
    assert.ok(markup.includes(copy[locale]["filter.searching"]))
    assert.doesNotMatch(markup, /%|Loaded|Загружено|No rows match|Под фильтр/)
  }
})

test("retained rows and failure are explicit", () => {
  const newest = request.beginSearchRequest("os_process", true)
  const failed = { ...newest, phase: "error" }
  assert.equal(failed.phase, "error")
  assert.equal(failed.retained, true)
  const markup = renderToStaticMarkup(createElement(request.SearchRequestMessage, {
    request: failed,
    t: (key) => copy.en[key] ?? key,
  }))
  assert.match(markup, /role="alert"/)
  assert.match(markup, /Rows retained/)
})
