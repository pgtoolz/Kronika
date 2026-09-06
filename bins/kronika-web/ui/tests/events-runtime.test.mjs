import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"

import { build } from "esbuild"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

import { parseDictionary } from "../scripts/i18n.mjs"
import { interpolate } from "../src/model.ts"
import { registryPlugin } from "./import-module.mjs"

const directory = dirname(fileURLToPath(import.meta.url))
const databaseColumns = [
  ...Array.from({ length: 16 }, (_, index) => `pad${index}`), "deadlocks",
  "pad17", "pad18", "pad19", "frozen_xid_age", "min_mxid_age",
  "pad22", "pad23", "pad24", "checksum_failures",
  ...Array.from({ length: 6 }, (_, index) => `pad${26 + index}`), "sessions_fatal", "sessions_killed",
]
const compiled = await build({
  bundle: true,
  external: ["react", "react-dom", "react/jsx-runtime"],
  format: "cjs",
  platform: "node",
  plugins: [registryPlugin([{
    columns: databaseColumns,
    identity: ["datid"],
    logicalName: "pg_stat_database",
    typeId: "1005004",
  }])],
  stdin: {
    contents: 'export { entryOf, eventConsoleStatus, eventGroupSelection, EventsEmptyState, EventsIncompleteNotice, groupMarks, MarkGroupRow } from "../src/events-view.tsx"; export { summarizeFindings } from "../src/finding-presentation.ts"; export { entryChips, entrySubtitle, entryTitle, eventDetailTexts, EventEntryRow } from "../src/events-console.tsx"',
    loader: "tsx",
    resolveDir: directory,
  },
  write: false,
})
const loaded = { exports: {} }
new Function("module", "exports", "require", compiled.outputFiles[0].text)(loaded, loaded.exports, createRequire(import.meta.url))
const events = loaded.exports

const translations = {
  "events.boundary.increased": "any increase from the previous value",
  "events.boundary.wraparound": "1,600,000,000 transactions or more",
  "events.source.database": "Databases",
  "pg.field.checksum_failures.help": "Checksum help text.",
  "pg.field.checksum_failures.label": "Checksum failures",
  "pg.field.min_mxid_age.help": "MultiXact help text.",
  "pg.field.min_mxid_age.label": "MultiXact age",
}
const t = (key) => translations[key] ?? key
const hour = Date.UTC(2026, 7, 23, 9) * 1_000

function dictionaryTranslate(dictionary) {
  return (key, slots = {}) => interpolate(dictionary[key] ?? key, slots)
}

function finding(fieldOrdinal, rowOrdinal = "7") {
  return {
    category: null,
    fieldOrdinal,
    kind: "known_bad",
    logicalName: "pg_stat_database",
    rowOrdinal,
    segmentId: "segment-a",
    timestamp: hour + 120_000_000,
    typeId: "1005004",
  }
}

test("Events renders each finding metric label, help and boundary from findingMetric", () => {
  for (const expected of [
    { boundary: "any increase from the previous value", fieldOrdinal: 25, help: "Checksum help text.", label: "Checksum failures" },
    { boundary: "1,600,000,000 transactions or more", fieldOrdinal: 21, help: "MultiXact help text.", label: "MultiXact age" },
  ]) {
    const [group] = events.groupMarks([finding(expected.fieldOrdinal)], hour, t)
    assert.ok(group)
    const markup = renderToStaticMarkup(createElement(events.MarkGroupRow, {
      expanded: true,
      group,
      hour,
      locale: "en",
      onCursor() {},
      onFinding() {},
      onToggle() {},
      t,
    }))
    assert.match(markup, new RegExp(`data-testid="event-mark-label">${expected.label}<`))
    assert.match(markup, new RegExp(`data-testid="event-mark-boundary">${expected.boundary}<`))
    assert.match(markup, new RegExp(`data-testid="event-mark-help">${expected.help}<`))
    assert.match(markup, />Databases</)
  }
})

test("Events groups repeated crossings before rendering metric metadata", () => {
  const later = { ...finding(25, "8"), timestamp: hour + 180_000_000 }
  const [group] = events.groupMarks([later, finding(25)], hour, t)
  assert.ok(group)
  assert.equal(group.findings.length, 2)
  assert.equal(group.metric.label, "Checksum failures")
  assert.equal(group.metric.helpKey, "pg.field.checksum_failures.help")
  assert.equal(group.metric.boundary, "any increase from the previous value")
})

test("Events uses the selected log marks as an exact query interval without double-counting log severity", () => {
  const entry = {
    section: "pg_log_errors",
    detailRef: "opaque-a",
    representativeTs: hour,
  }
  const representative = {
    category: null,
    fieldOrdinal: 0,
    kind: "event",
    logicalName: "pg_log_errors",
    rowOrdinal: "1",
    segmentId: "segment-a",
    timestamp: hour,
    typeId: "2001001",
  }
  const pairedSeverity = { ...representative, kind: "known_bad" }
  const otherMember = { ...representative, rowOrdinal: "2", timestamp: hour + 7 }
  const otherSource = { ...otherMember, logicalName: "pg_log_checkpoints", typeId: "2002001" }
  const threshold = { ...otherMember, kind: "known_bad", logicalName: "os_cpu", typeId: "1102001" }
  const spike = { ...threshold, kind: "spike", rowOrdinal: "3", timestamp: hour + 9 }
  const scope = [spike, pairedSeverity, otherSource, representative, threshold]

  assert.deepEqual(events.summarizeFindings(scope), {
    event: 2,
    from: hour,
    knownBad: 1,
    spike: 1,
    to: hour + 9,
  })
  assert.deepEqual(events.eventGroupSelection(
    ["pg_log_checkpoints", "pg_log_errors", "pgbouncer_events"],
    hour,
    scope,
  ), {
    from: hour,
    sources: ["pg_log_errors", "pg_log_checkpoints"],
    to: hour + 8,
  })
  assert.deepEqual(events.eventGroupSelection(["pg_log_errors", "pgbouncer_events"], hour, null), {
    from: hour,
    sources: ["pg_log_errors", "pgbouncer_events"],
    to: hour + 3_600_000_000,
  })
  assert.equal(events.entryOf([entry], representative), entry)
  assert.equal(events.entryOf([entry], otherMember), null)
  assert.equal(events.entryOf([entry, { ...entry, detailRef: "opaque-b" }], representative), null)
})

test("Events never presents pending or failed log reads as zero", () => {
  const retained = [{ count: 7 }, { count: 8 }]
  assert.deepEqual(events.eventConsoleStatus(null, true, false), { key: "events.console.loading" })
  assert.deepEqual(events.eventConsoleStatus(null, false, true), { key: "events.console.unavailable" })
  assert.deepEqual(events.eventConsoleStatus([], false, false), {
    key: "events.console.count",
    slots: { groups: 0, count: 0 },
  })
  assert.deepEqual(events.eventConsoleStatus(retained, false, true), {
    key: "events.console.update_failed",
    slots: { groups: 2, count: 15 },
  })
})

test("expanded Events group exposes one keyboard action without embedding stored text", () => {
  const entry = {
    key: "error:fixture",
    section: "pg_log_errors",
    tier: "notable",
    label: "fixture pattern",
    count: 2,
    firstTs: hour,
    lastTs: hour,
    minutes: Array.from({ length: 60 }, (_, index) => index === 0 ? 2 : 0),
    stat: { kind: "pg.errors", severity: 0, category: 8, sqlstate: "28P01", database: "db", username: "role" },
    detailRef: "opaque-a",
    representativeTs: hour,
  }
  const originalFetch = globalThis.fetch
  let fetches = 0
  globalThis.fetch = async () => {
    fetches += 1
    throw new Error("expanded group fetched detail eagerly")
  }
  try {
    const markup = renderToStaticMarkup(createElement(events.EventEntryRow, {
      entry,
      expanded: true,
      hour,
      locale: "en",
      onCursor() {},
      onToggle() {},
      t,
    }))
    assert.match(markup, /data-testid="event-representative-detail"/)
    assert.match(markup, /type="button"/)
    assert.doesNotMatch(markup, /exact stored sample/)
    assert.equal(fetches, 0)
  } finally {
    globalThis.fetch = originalFetch
  }

  assert.deepEqual(events.eventDetailTexts({
    sample: { stored_text: "exact stored sample", full_len: "19", truncated: false, sha256: null },
    count: "2",
  }), [{ field: "sample", fullLen: "19", storedText: "exact stored sample", truncated: false }])
})

test("PgBouncer groups lead with the exact message and explain unresolved connection context", async () => {
  const entry = {
    key: "pgbouncer:3:no such database: nope",
    section: "pgbouncer_events",
    tier: "routine",
    label: "no such database: nope",
    count: 19,
    firstTs: hour,
    lastTs: hour + 120_000_000,
    minutes: Array.from({ length: 60 }, (_, index) => index < 2 ? 1 : 0),
    stat: {
      kind: "pgbouncer.events",
      level: 3,
      database: "(nodb)",
      username: "(nouser)",
      host: "10.0.0.7",
      sourceFile: "/var/log/pgbouncer.log",
    },
    detailRef: "opaque-pgbouncer",
    representativeTs: hour,
  }
  const dictionaries = await Promise.all(["en", "ru"].map(async (locale) => parseDictionary(
    await readFile(new URL(`../i18n/${locale}.yaml`, import.meta.url), "utf8"),
    `${locale}.yaml`,
  )))
  const expectedSubtitles = [
    "(nodb) · database not selected · (nouser) · user not resolved · Client 10.0.0.7",
    "(nodb) · база ещё не выбрана · (nouser) · пользователь ещё не определён · Client 10.0.0.7",
  ]

  for (const [index, locale] of ["en", "ru"].entries()) {
    const translate = dictionaryTranslate(dictionaries[index])
    assert.equal(events.entryTitle(entry, translate, locale), "no such database: nope")
    assert.equal(events.entrySubtitle(entry, translate, locale), expectedSubtitles[index])
    assert.deepEqual(events.entryChips(entry, translate), [{ label: "LOG", tone: "neutral" }])
    const markup = renderToStaticMarkup(createElement(events.EventEntryRow, {
      entry,
      expanded: true,
      hour,
      locale,
      onCursor() {},
      onToggle() {},
      t: translate,
    }))
    assert.match(markup, /data-testid="event-entry-title"[^>]*title="no such database: nope"[^>]*>no such database: nope</)
    assert.match(markup, /class="[^"]*text-\[12px\][^"]*font-mono[^"]*" data-testid="event-entry-title"/)
    assert.match(markup, />\(nodb\)</)
    assert.match(markup, />\(nouser\)</)
    assert.match(markup, />10\.0\.0\.7</)
    assert.match(markup, />\/var\/log\/pgbouncer\.log</)
  }
})

test("Events keeps the localized incomplete notice beside a filtered zero state", async () => {
  const dictionaries = await Promise.all(["en", "ru"].map(async (locale) => parseDictionary(
    await readFile(new URL(`../i18n/${locale}.yaml`, import.meta.url), "utf8"),
    `${locale}.yaml`,
  )))
  const expected = [
    "Only the first 5,000 groups are available; this result is incomplete.",
    "Доступны только первые 5 000 групп; результат неполный.",
  ]
  for (const [index, locale] of ["en", "ru"].entries()) {
    const translate = dictionaryTranslate(dictionaries[index])
    const markup = renderToStaticMarkup(createElement("section", null,
      createElement(events.EventsIncompleteNotice, { locale, t: translate }),
      createElement(events.EventsEmptyState, { filtered: true, onClear() {}, t: translate, truncated: true }),
    ))
    assert.match(markup, /data-testid="events-truncated"/)
    assert.ok(markup.includes(expected[index]))
    assert.ok(markup.includes(dictionaries[index]["filter.none"]))
    assert.match(markup, /data-testid="events-clear-filter"/)

    const unfiltered = renderToStaticMarkup(createElement(events.EventsEmptyState, {
      filtered: false,
      onClear() {},
      t: translate,
      truncated: true,
    }))
    assert.ok(unfiltered.includes(dictionaries[index]["events.console.truncated_none"]))

    const selected = renderToStaticMarkup(createElement(events.EventsEmptyState, {
      filtered: false,
      onClear() {},
      selected: true,
      t: translate,
      truncated: false,
    }))
    assert.ok(selected.includes(dictionaries[index]["events.console.empty_selection"]))
  }
})
