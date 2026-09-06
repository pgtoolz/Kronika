import assert from "node:assert/strict"
import test from "node:test"

import { DEFAULT_ADDRESS, pgSectionOf, readAddress, sourceOf, viewOf, writeAddress } from "../src/address.ts"

test("an address survives a round trip through the query string", () => {
  const address = {
    ...DEFAULT_ADDRESS,
    at: 1_786_445_580_254_226,
    view: "processes" as const,
    lens: "tree" as const,
    pgLens: "load" as const,
    sort: { column: "utime", descending: true },
    row: "1244346:1784523346370000",
    panel: "detail" as const,
    find: "postgres*",
  }
  const written = writeAddress(address)

  assert.equal(written, "/?at=1786445580254226&sort=-utime&row=1244346%3A1784523346370000&find=postgres*")
  assert.deepEqual(readAddress(written.slice(1)), address)
})

test("the cursor address preserves an exact microsecond observation", () => {
  const at = 1_786_445_580_254_001
  const written = writeAddress({ ...DEFAULT_ADDRESS, at, view: "pg.activity" })

  assert.equal(written, "/?at=1786445580254001&view=pg.activity")
  assert.equal(readAddress(written.slice(1)).at, at)
})

test("a plain screen keeps a plain link", () => {
  assert.equal(writeAddress(DEFAULT_ADDRESS), "/")
  assert.deepEqual(readAddress(""), DEFAULT_ADDRESS)
})

test("relation tabs default to the unfiltered object level", () => {
  for (const view of ["pg.tables", "pg.indexes"] as const) {
    const address = readAddress(`view=${view}`)
    assert.equal(address.pgLevel, "object")
    assert.equal(address.datid, null)
    assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view }), `/?view=${view}`)
  }
})

test("the lens is written only where it means something", () => {
  const written = writeAddress({ ...DEFAULT_ADDRESS, lens: "disk", view: "pg.activity" })
  assert.equal(written, "/?view=pg.activity")
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, lens: "generic" }), "/?lens=generic")
})

test("a process row never leaks into a PostgreSQL address", () => {
  const written = writeAddress({ ...DEFAULT_ADDRESS, row: "1244346:1784523346370000", view: "pg.statements" })
  assert.equal(written, "/?view=pg.statements")
  assert.equal(readAddress("view=pg.statements&row=1244346:1784523346370000").row, null)
})

test("an unreadable value falls back instead of failing", () => {
  const address = readAddress("at=tomorrow&view=hostile.klingon&lens=quantum&sort=")

  assert.equal(address.at, null)
  assert.equal(address.view, "processes")
  assert.equal(address.lens, "tree")
  assert.equal(address.sort, null)
  // Links written before the Host ledger keep landing on the Host page.
  assert.equal(readAddress("view=host.cpu").view, "host")
  assert.equal(readAddress("view=host.overview&metric=cpu_user").metric, "cpu_user")
})

test("ascending sort has no marker and descending has a minus", () => {
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, sort: { column: "rmem_kb", descending: false } }), "/?sort=rmem_kb")
  assert.deepEqual(readAddress("sort=rmem_kb").sort, { column: "rmem_kb", descending: false })
  assert.deepEqual(readAddress("sort=-rmem_kb").sort, { column: "rmem_kb", descending: true })
})

test("PostgreSQL plans have a stable address", () => {
  const address = { ...DEFAULT_ADDRESS, view: "pg.plans" as const }

  assert.equal(writeAddress(address), "/?view=pg.plans")
  assert.equal(readAddress("view=pg.plans").view, "pg.plans")
  assert.equal(sourceOf(address.view), "postgresql")
  assert.equal(pgSectionOf(address.view), "plans")
  assert.equal(viewOf("postgresql", "plans"), "pg.plans")
})

test("PostgreSQL lenses survive navigation only on their tables", () => {
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view: "pg.statements", pgLens: "stability" }), "/?view=pg.statements&pg_lens=stability")
  assert.equal(readAddress("view=pg.statements&pg_lens=stability").pgLens, "stability")
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view: "pg.plans", pgLens: "timing" }), "/?view=pg.plans&pg_lens=timing")
  assert.equal(readAddress("view=pg.plans&pg_lens=timing").pgLens, "timing")
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, view: "host", pgLens: "io" }), "/?view=host")
  assert.equal(readAddress("view=pg.plans&pg_lens=regression").pgLens, "load")
})

test("relation drilldown keeps database-scoped identity", () => {
  const selected = JSON.stringify(["pg_stat_user_indexes", "object", "16384", "app", "public spaces", "24576", "orders", "24577", "orders_pkey"])
  const address = {
    ...DEFAULT_ADDRESS,
    view: "pg.indexes" as const,
    pgLens: "low_activity" as const,
    pgLevel: "object" as const,
    datid: "16384",
    schema: "public spaces",
    relid: "24576",
    indexrelid: "24577",
    row: selected,
    panel: "detail" as const,
  }
  const written = writeAddress(address)
  assert.equal(written, `/?view=pg.indexes&pg_lens=low_activity&datid=16384&schema=public+spaces&relid=24576&indexrelid=24577&${new URLSearchParams({ row: selected })}`)
  assert.deepEqual(readAddress(written.slice(1)), address)
  assert.equal(readAddress("view=pg.tables&level=schema&datid=oops").pgLevel, "schema")
})

test("relation levels round-trip with or without a drill-down context", () => {
  for (const pgLevel of ["object", "schema", "database", "tablespace"] as const) {
    const address = { ...DEFAULT_ADDRESS, view: "pg.indexes" as const, pgLevel, find: "orders*", pgLens: "state" as const }
    const written = writeAddress(address)
    assert.deepEqual(readAddress(written.slice(1)), address)
  }
})

test("tablespace member filters are validated and URL-native", () => {
  const address = {
    ...DEFAULT_ADDRESS,
    at: 1_700_000_000_000_123,
    view: "pg.tables" as const,
    pgLevel: "object" as const,
    tablespaceOid: "4294967295",
    find: "table_name:orders*",
  }
  const written = writeAddress(address)
  assert.equal(written, "/?at=1700000000000123&view=pg.tables&tablespace_oid=4294967295&find=table_name%3Aorders*")
  assert.deepEqual(readAddress(written.slice(1)), address)
  assert.equal(readAddress("view=pg.tables&tablespace_oid=0").tablespaceOid, null)
  assert.equal(readAddress("view=pg.tables&tablespace_oid=4294967296").tablespaceOid, null)
  const group = readAddress("view=pg.tables&level=tablespace&datid=42&tablespace_oid=7")
  assert.equal(group.datid, null)
  assert.equal(group.tablespaceOid, null)
})

test("relation selection is separate from hierarchy filters", () => {
  const selected = JSON.stringify(["pg_stat_user_tables", "object", "16384", "app", "public", "24576", "orders"])
  const written = writeAddress({
    ...DEFAULT_ADDRESS,
    view: "pg.tables",
    pgLens: "changes",
    pgLevel: "object",
    datid: "16384",
    schema: "public",
    row: selected,
  })

  assert.match(written, /row=/)
  assert.equal(readAddress(written.slice(1)).row, selected)
  assert.equal(readAddress("view=pg.statements&row=hidden").row, null)
})

test("Host master/detail modes and structured related search are URL-native and route-scoped", () => {
  const host = writeAddress({ ...DEFAULT_ADDRESS, view: "host", metric: "cpu_used_cores" })
  assert.equal(host, "/?view=host&metric=cpu_used_cores")
  assert.equal(readAddress(host.slice(1)).metric, "cpu_used_cores")
  const mount = writeAddress({ ...DEFAULT_ADDRESS, view: "host", row: "mount:8:1:/", metric: "free_bytes" })
  assert.equal(mount, "/?view=host&row=mount%3A8%3A1%3A%2F&metric=free_bytes")
  assert.equal(readAddress(mount.slice(1)).row, "mount:8:1:/")

  const find = "database:app AND role:reader AND query_id:-9223372036854775808"
  const query = writeAddress({ ...DEFAULT_ADDRESS, at: 1_700_000_000_000_000, view: "pg.statements", find })
  assert.equal(query, "/?at=1700000000000000&view=pg.statements&find=database%3Aapp+AND+role%3Areader+AND+query_id%3A-9223372036854775808")
  assert.equal(readAddress(query).find, find)
  const activityRow = writeAddress({ ...DEFAULT_ADDRESS, view: "pg.activity", row: "1700000000000000:1001004:73" })
  assert.equal(activityRow, "/?view=pg.activity&row=1700000000000000%3A1001004%3A73")
  assert.equal(readAddress(activityRow.slice(1)).row, "1700000000000000:1001004:73")
})

test("Inspector chart and row-only Detail links are URL-native", () => {
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, panel: "chart" }), "/?panel=chart")
  assert.equal(readAddress("panel=chart").panel, "chart")
  assert.equal(readAddress("row=42:1700000000000000").panel, "detail")
  assert.equal(readAddress("panel=detail").panel, null)
  // Relation panels ride the address so Back lands on the tab it left.
  assert.equal(writeAddress({ ...DEFAULT_ADDRESS, panel: "os_process" }), "/?panel=os_process")
  assert.equal(readAddress("panel=os_process").panel, "os_process")
  assert.equal(readAddress("panel=pg_stat_activity").panel, "pg_stat_activity")
  assert.equal(readAddress("panel=nonsense").panel, null)
  // A metric-only Detail link predates the ledger: the metric survives, the
  // panel does not open without a row.
  const hostDetail = writeAddress({ ...DEFAULT_ADDRESS, view: "host", metric: "cpu_used_cores", panel: "detail" })
  assert.equal(hostDetail, "/?view=host&metric=cpu_used_cores")
  assert.equal(readAddress(hostDetail.split("?")[1] ?? "").panel, null)
})
