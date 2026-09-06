import assert from "node:assert/strict"
import test from "node:test"

import { readNdjson } from "../src/wire.ts"
import { importModule, registryPlugin } from "./import-module.mjs"

test("the NDJSON reader handles chunked UTF-8, line endings, and a final line", async () => {
  const body = '\n{"record":"row","value":"Привет"}\r\n\r\n{"record":"page"}'
  const bytes = new TextEncoder().encode(body)
  const response = chunkedResponse(bytes, Array.from({ length: bytes.length - 1 }, (_, index) => index + 1))

  assert.deepEqual(
    await readNdjson(response, "/api/example", new AbortController().signal),
    [{ record: "row", value: "Привет" }, { record: "page" }],
  )
})

test("a streamed error record rejects an otherwise successful response", async () => {
  const body = new TextEncoder().encode('{"record":"rows"}\n{"record":"error","error":"unreadable"}\n')
  await assert.rejects(
    readNdjson(chunkedResponse(body, [5, 23, 47]), "/api/example", new AbortController().signal),
    /unreadable.*\/api\/example/,
  )
})

test("an aborted NDJSON read rejects", async () => {
  const abort = new AbortController()
  const response = new Response(new ReadableStream<Uint8Array>({
    pull(controller) {
      controller.enqueue(new TextEncoder().encode('{"record":"rows"}\n'))
      abort.abort()
    },
  }))
  await assert.rejects(readNdjson(response, "/api/example", abort.signal), { name: "AbortError" })
})

function chunkedResponse(bytes: Uint8Array, cuts: readonly number[]): Response {
  const boundaries = [...new Set([0, ...cuts, bytes.length])].sort((left, right) => left - right)
  return new Response(new ReadableStream<Uint8Array>({
    start(controller) {
      for (let index = 1; index < boundaries.length; index += 1) {
        controller.enqueue(bytes.slice(boundaries[index - 1], boundaries[index]))
      }
      controller.close()
    },
  }))
}

const START = 1_800_000_000_000_000
const TEST_REGISTRY = [
  {
    typeId: "1001001",
    logicalName: "pg_stat_activity",
    columns: ["ts", "pid", "datname", "state", "query", "backend_start", "xact_start", "query_start", "state_change"],
  },
  {
    typeId: "1001004",
    logicalName: "pg_stat_activity",
    columns: ["ts", "pid", "backend_type", "state", "query_start", "xact_start"],
  },
  {
    typeId: "1105001",
    logicalName: "os_loadavg",
    columns: ["ts", "load1", "load5"],
  },
  {
    typeId: "1005001",
    logicalName: "pg_stat_database",
    columns: ["ts", "datid", "datname", "xact_commit"],
  },
  {
    typeId: "2001001",
    logicalName: "pg_log_errors",
    columns: ["ts", "message", "category"],
  },
  {
    typeId: "1002001",
    logicalName: "pg_stat_statements",
    identity: ["queryid", "userid", "dbid"],
    columns: ["ts", "queryid", "userid", "dbid", "datname", "usename", "query", "calls", "total_time", "rows"],
  },
  {
    typeId: "1002002",
    logicalName: "pg_stat_statements",
    identity: ["queryid", "userid", "dbid"],
    columns: ["ts", "queryid", "userid", "dbid", "datname", "usename", "toplevel", "query", "calls", "total_exec_time", "rows", "wal_bytes"],
  },
  {
    typeId: "1003001",
    logicalName: "pg_store_plans",
    identity: ["userid", "dbid", "queryid", "planid"],
    columns: ["ts", "userid", "dbid", "queryid", "planid", "plan", "calls", "total_time"],
  },
  {
    typeId: "1020001",
    logicalName: "pg_wal_storage",
    identity: [],
    columns: ["ts", "wal_files_bytes"],
  },
]

test("snapshot rows use each physical layout's positional columns", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.pathname, "/api/segments/77/snapshot")
    assert.deepEqual(url.searchParams.getAll("section"), ["os_loadavg"])
    return ndjson([
      {
        record: "layout",
        layout: {
          type_id: "1105001",
          logical_name: "os_loadavg",
          columns: [{ name: "ts" }, { name: "load1" }, { name: "load5" }],
        },
      },
      {
        record: "layout",
        layout: {
          type_id: "1105002",
          logical_name: "os_loadavg",
          columns: [{ name: "load5" }, { name: "load1" }],
        },
      },
      {
        record: "row", segment_id: "71", type_id: "1105001", ordinal: "4",
        timestamp: String(START + 1), values: [String(START + 1), 1, 5],
      },
      {
        record: "row", segment_id: "72", type_id: "1105002", ordinal: "8",
        timestamp: String(START + 2), values: [50, 10],
      },
    ])
  }

  try {
    const hour = await api.loadSnapshot("77", START, ["os_loadavg"], new AbortController().signal)
    assert.equal(hour.load.length, 2)
    assert.deepEqual(hour.load.map((row) => row.segmentId), ["71", "72"])
    assert.equal(hour.load[0]?.timestamp, START + 1)
    assert.deepEqual(hour.load[0]?.values, { load1: 1, load5: 5 })
    assert.deepEqual(hour.load[1]?.values, { load5: 50, load1: 10 })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("ten compatible snapshot projections share one request and keep physical columns", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const requests = Array.from({ length: 10 }, (_, index) => ({
    section: `section_${index}`,
    fields: [`metric_${index}`],
  }))
  const originalFetch = globalThis.fetch
  let fetches = 0
  globalThis.fetch = async (input) => {
    fetches += 1
    const url = new URL(String(input), "http://kronika.invalid")
    assert.deepEqual(url.searchParams.getAll("section"), requests.map(({ section }) => section))
    assert.deepEqual(url.searchParams.getAll("field"), requests.map(({ fields }) => fields[0]))
    return ndjson([
      {
        record: "layout",
        layout: {
          type_id: "1", logical_name: "section_0",
          columns: [{ name: "metric_0" }],
        },
      },
      { record: "row", segment_id: "77", type_id: "1", ordinal: "0", timestamp: String(START), values: [10] },
    ])
  }

  try {
    const snapshot = await api.loadSnapshot("77", START, requests, new AbortController().signal)
    assert.equal(fetches, 1)
    assert.deepEqual(snapshot.sections.section_0?.map((row) => row.values), [{ metric_0: 10 }])
    assert.deepEqual(snapshot.sections.section_9, [])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a compatible projection stays untyped after catalog resolution", async () => {
  const api = await bundledApi()
  assert.deepEqual(api.requestsForSegment(
    [{ section: "os_loadavg", fields: ["load1", "missing"] }],
    {
      id: "77", minTs: START, maxTs: START,
      sections: [{ logicalName: "os_loadavg", typeId: "1105001" }],
    },
  ), [{ section: "os_loadavg", fields: ["load1"] }])
})

test("an old Activity layout drops newer optional fields without losing its exact start time", async () => {
  const api = await bundledApi()
  assert.deepEqual(api.requestsForSegment(
    [{ section: "pg_stat_activity", fields: api.ACTIVITY_FIELDS }],
    {
      id: "77", minTs: START, maxTs: START,
      sections: [{ logicalName: "pg_stat_activity", typeId: "1001001" }],
    },
  ), [{
    section: "pg_stat_activity",
    fields: ["pid", "datname", "state", "query", "backend_start", "xact_start", "query_start", "state_change"],
  }])
})

test("a curated snapshot follows the registry layout and physical order", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const statement = {
    section: "pg_stat_statements",
    typeIds: ["1002001", "1002002"],
    pageSize: 200,
    defaultOrder: ["total_time", "total_exec_time"],
    order: { wal_demand: ["wal_bytes"] },
    fallbackOrder: ["calls"],
    fieldsByType: {
      "1002001": ["queryid", "userid", "dbid", "query", "calls", "total_time", "wal_bytes"],
      "1002002": ["queryid", "userid", "dbid", "query", "calls", "total_exec_time", "wal_bytes"],
    },
  }
  const segment = {
    id: "77", minTs: START, maxTs: START + 10,
    sections: [
      { logicalName: "pg_stat_statements", typeId: "1002001" },
      { logicalName: "pg_stat_statements", typeId: "1002002" },
    ],
  }
  const requests = api.requestsForSegment([statement], segment)
  assert.equal(requests.length, 1)
  assert.equal(requests[0]?.typeId, undefined)
  assert.deepEqual(requests[0]?.fields, [
    "queryid", "userid", "dbid", "query", "calls", "total_time", "wal_bytes", "total_exec_time",
  ])
  assert.deepEqual(requests[0]?.defaultOrder, ["total_time", "total_exec_time"])
  assert.deepEqual(requests[0]?.order, { wal_demand: ["wal_bytes"] })
  assert.deepEqual(requests[0]?.fallbackOrder, ["calls"])

  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.deepEqual(url.searchParams.getAll("section"), ["pg_stat_statements"])
    assert.deepEqual(url.searchParams.getAll("field"), [
      "queryid", "userid", "dbid", "query", "calls", "total_time", "wal_bytes", "total_exec_time",
    ])
    assert.deepEqual(url.searchParams.getAll("by"), ["wal_bytes"])
    assert.equal(url.searchParams.get("page_size"), "200")
    assert.equal(url.searchParams.has("top"), false)
    assert.equal(url.searchParams.has("type_id"), false)
    assert.equal(url.searchParams.get("cursor"), "opaque+/=")
    assert.equal(url.searchParams.get("search"), "database:app AND text:vacuum*")
    assert.equal(url.searchParams.get("text"), "160")
    assert.equal(url.searchParams.has("plan"), false)
    return ndjson([
      {
        record: "layout", rates: ["calls", "total_time"],
        layout: {
          type_id: "1002001", logical_name: "pg_stat_statements",
          columns: ["queryid", "userid", "dbid", "query", "calls", "total_time"].map((name) => ({ name })),
        },
      },
      {
        record: "layout", rates: ["calls", "total_exec_time"],
        layout: {
          type_id: "1002002", logical_name: "pg_stat_statements",
          columns: ["queryid", "userid", "dbid", "query", "calls", "wal_bytes", "total_exec_time"].map((name) => ({ name })),
        },
      },
      {
        record: "row", segment_id: "73", type_id: "1002001", ordinal: "3", timestamp: String(START),
        values: ["41", "10", "20", { stored_text: "select 1", original_length: 8 }, 2, 3],
      },
      {
        record: "row", segment_id: "74", type_id: "1002002", ordinal: "4", timestamp: String(START),
        values: ["42", "10", "20", "vacuum t", 5, "9007199254740995", 7],
      },
      {
        record: "snapshot_page", logical_name: "pg_stat_statements", eligible: "4873", excluded: "3", returned: "2",
        has_more: true, truncated: true, next_cursor: "next+/=", page_size: 200,
        order_by: ["wal_bytes", "total_time"], order_direction: "desc",
        from: String(START - 10_000_000), to: String(START),
      },
    ])
  }
  try {
    const hour = await api.loadSnapshot("77", START, requests, new AbortController().signal, {
      column: "wal_demand", descending: true,
    }, {
      cursor: "opaque+/=", search: "database:app AND text:vacuum*",
    })
    assert.equal(hour.sections.pg_stat_statements?.length, 2)
    assert.deepEqual(hour.rateColumns.pg_stat_statements, ["calls", "total_time", "total_exec_time"])
    assert.deepEqual(hour.snapshotRows, [{
      logicalName: "pg_stat_statements", eligible: 4873, excluded: 3, returned: 2,
      hasMore: true, truncated: true, nextCursor: "next+/=", pageSize: 200,
      orderBy: ["wal_bytes", "total_time"], orderDirection: "desc",
      from: START - 10_000_000, to: START,
    }])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("physical execution aliases are selected before the calls fallback", async () => {
  const api = await bundledApi()
  const statement = {
    section: "pg_stat_statements",
    pageSize: 200,
    defaultOrder: ["total_time", "total_exec_time"],
    fallbackOrder: ["calls"],
    fieldsByType: {
      "1002001": ["queryid", "calls", "total_time"],
      "1002002": ["queryid", "calls", "total_exec_time"],
    },
  }
  const v1 = api.requestsForSegment([statement], {
    id: "v1", minTs: START, maxTs: START,
    sections: [{ logicalName: "pg_stat_statements", typeId: "1002001" }],
  })
  const v2 = api.requestsForSegment([statement], {
    id: "v2", minTs: START, maxTs: START,
    sections: [{ logicalName: "pg_stat_statements", typeId: "1002002" }],
  })
  assert.deepEqual(v1[0]?.defaultOrder, ["total_time"])
  assert.deepEqual(v2[0]?.defaultOrder, ["total_exec_time"])

  const seen: URL[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    seen.push(new URL(String(input), "http://kronika.invalid"))
    return ndjson([])
  }
  try {
    await api.loadSnapshot("v1", START, v1, new AbortController().signal)
    await api.loadSnapshot("v2", START, v2, new AbortController().signal)
    await api.loadSnapshot("v2", START, v2, new AbortController().signal, {
      column: "total_exec_time", descending: true,
    })
    await api.loadSnapshot("v2", START, v2, new AbortController().signal, {
      column: "calls", descending: false,
    })
    assert.deepEqual(seen[0]?.searchParams.getAll("by"), ["total_time"])
    assert.deepEqual(seen[1]?.searchParams.getAll("by"), ["total_exec_time"])
    assert.deepEqual(seen[2]?.searchParams.getAll("by"), ["total_exec_time"])
    assert.deepEqual(seen[3]?.searchParams.getAll("by"), ["calls"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("derived snapshot orders survive physical layout resolution", async () => {
  const api = await bundledApi()
  const [resolved] = api.requestsForSegment([{
    section: "pg_stat_statements",
    typeIds: ["1002002"],
    fieldsByType: { "1002002": ["queryid", "calls", "total_exec_time"] },
    pageSize: 200,
    defaultOrder: ["calls"],
    order: {
      query: [],
      mean_exec_ms_per_call: ["derived.mean_exec_ms_per_call"],
    },
  }], {
    id: "v2", minTs: START, maxTs: START,
    sections: [{ logicalName: "pg_stat_statements", typeId: "1002002" }],
  })
  assert.deepEqual(resolved?.order, {
    query: [],
    mean_exec_ms_per_call: ["derived.mean_exec_ms_per_call"],
  })
  assert.ok(resolved)
  const [old] = api.requestsForSegment([{
    section: "pg_stat_statements",
    typeIds: ["1002001", "1002002"],
    fieldsByType: {
      "1002001": ["queryid", "calls", "total_time"],
      "1002002": ["queryid", "calls", "total_exec_time", "wal_bytes"],
    },
    pageSize: 200,
    defaultOrder: ["calls"],
    order: {
      mean_exec_ms_per_call: ["derived.mean_exec_ms_per_call"],
      wal_per_call: ["derived.wal_per_call"],
      plan_time_pct: ["derived.plan_time_pct"],
    },
  }], {
    id: "v1", minTs: START, maxTs: START,
    sections: [{ logicalName: "pg_stat_statements", typeId: "1002001" }],
  })
  assert.deepEqual(old?.order, {
    mean_exec_ms_per_call: ["derived.mean_exec_ms_per_call"],
    wal_per_call: [],
    plan_time_pct: [],
  })

  const seen: URL[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    seen.push(new URL(String(input), "http://kronika.invalid"))
    return ndjson([])
  }
  try {
    await api.loadSnapshot("v2", START, [resolved], new AbortController().signal, {
      column: "mean_exec_ms_per_call", descending: true,
    })
    await api.loadSnapshot("v2", START, [resolved], new AbortController().signal, {
      column: "query", descending: true,
    })
    assert.deepEqual(seen[0]?.searchParams.getAll("by"), ["derived.mean_exec_ms_per_call"])
    assert.deepEqual(seen[1]?.searchParams.getAll("by"), ["calls"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("fixture composite ordering uses interval deltas instead of cumulative operands", async () => {
  const api = await bundledApi()
  const row = (ordinal: string, timestamp: number, values: Readonly<Record<string, number | string>>) => ({
    segmentId: "v2", logicalName: "pg_stat_statements", typeId: "1002002", ordinal, timestamp, values: {
      queryid: ordinal, userid: 1, dbid: 1, ...values,
    },
  })
  const beforeA = row("a", START, { calls: 0, total_exec_time: 0 })
  const afterA = row("a", START + 1_000_000, { calls: 10, total_exec_time: 100 })
  const beforeB = row("b", START, { calls: 0, total_exec_time: 0 })
  const afterB = row("b", START + 1_000_000, { calls: 2, total_exec_time: 30 })
  const rows = [beforeA, afterA, beforeB, afterB]
  assert.equal(api.fixtureDerivedOrderValue(afterA, rows, "mean_exec_ms_per_call"), 10)
  assert.equal(api.fixtureDerivedOrderValue(afterB, rows, "mean_exec_ms_per_call"), 15)
})

test("snapshot pages append once in server order and deduplicate physical coordinates", async () => {
  const api = await bundledApi()
  const make = (ordinal: string, timestamp: number, typeId = "1002002") => ({
    segmentId: "77", logicalName: "pg_stat_statements", typeId, ordinal, timestamp, values: { queryid: ordinal },
  })
  const first = [make("1", START), make("2", START)]
  const next = [make("2", START), make("3", START), make("3", START + 1)]
  const appended = api.appendSnapshotRows(first, next)
  assert.deepEqual(appended.map((row) => [row.typeId, row.ordinal, row.timestamp]), [
    ["1002002", "1", START],
    ["1002002", "2", START],
    ["1002002", "3", START],
  ])
  assert.deepEqual(api.appendSnapshotRows(appended, next), appended)
  const sameCoordinateFromAnotherSegment = { ...first[0]!, segmentId: "78" }
  assert.deepEqual(
    api.appendSnapshotRows([first[0]!], [sameCoordinateFromAnotherSegment]).map((row) => row.segmentId),
    ["77", "78"],
  )
})

test("relation pages preserve same-named objects from different databases", async () => {
  const api = await bundledApi()
  const make = (datid: string, datname: string) => ({
    segmentId: "77", logicalName: "pg_stat_user_indexes", typeId: "1014003", ordinal: datid, timestamp: START,
    values: { datid, datname, schemaname: "public", relid: "42", relname: "orders", indexrelid: "43", indexrelname: "orders_pkey" },
    relation: { group: "object" },
  })
  const first = make("11", "one")
  const second = make("12", "two")
  const appended = api.appendSnapshotRows([first], [first, second])
  assert.deepEqual(appended, [first, second])
  assert.deepEqual(api.appendSnapshotRows(appended, [first, second]), appended)
})

test("paged entity context intersects search and clears independently", async () => {
  const api = await bundledApi()
  const seen: URL[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(url)
    return ndjson([
      { record: "layout", rates: ["calls"], layout: { type_id: "1002001", logical_name: "pg_stat_statements", columns: ["queryid", "userid", "dbid", "query", "calls"].map((name) => ({ name })) } },
      { record: "snapshot_page", logical_name: "pg_stat_statements", eligible: "0", returned: "0", has_more: false, truncated: false, next_cursor: null, page_size: 200, order_by: ["calls"], order_direction: "desc", from: null, to: null },
    ])
  }
  const request = { section: "pg_stat_statements", fields: ["queryid", "userid", "dbid", "query", "calls"], pageSize: 200, defaultOrder: ["calls"] }
  try {
    await api.loadSnapshot("77", START, [request], new AbortController().signal, undefined, {
      filters: { queryid: "9007199254740997", userid: "10", dbid: "11" },
      search: "text:vacuum*", typeId: "1002001",
    })
    await api.loadSnapshot("77", START, [request], new AbortController().signal, undefined, { search: "text:vacuum*" })
  } finally {
    globalThis.fetch = originalFetch
  }
  assert.equal(seen.length, 2)
  assert.equal(seen[0]?.searchParams.get("type_id"), "1002001")
  assert.equal(seen[0]?.searchParams.get("where.queryid"), "9007199254740997")
  assert.equal(seen[0]?.searchParams.get("where.userid"), "10")
  assert.equal(seen[0]?.searchParams.get("where.dbid"), "11")
  assert.deepEqual(seen.map((url) => url.searchParams.get("search")), ["text:vacuum*", "text:vacuum*"])
  assert.equal(seen[1]?.searchParams.has("type_id"), false)
  assert.equal([...seen[1]!.searchParams.keys()].some((key) => key.startsWith("where.")), false)
})

test("exact V1 and V2 query text requests carry only query and exact identity", async () => {
  const api = await bundledApi()
  const originalFetch = globalThis.fetch
  const seen: URL[] = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(url)
    const typeId = url.searchParams.get("type_id")
    return ndjson([
      {
        record: "layout",
        layout: { type_id: typeId, logical_name: "pg_stat_statements", columns: [{ name: "query" }] },
      },
      { record: "row", segment_id: "77", type_id: typeId, ordinal: "1", timestamp: String(START), values: ["select exact"] },
    ])
  }
  try {
    for (const typeId of ["1002001", "1002002"]) {
      const hour = await api.loadSnapshot(
        "77",
        START,
        [{ section: "pg_stat_statements", fields: ["query"], typeId }],
        new AbortController().signal,
        undefined,
        { filters: { queryid: "9007199254740999", userid: "10", dbid: "20" }, typeId, fullText: true },
      )
      assert.equal(hour.sections.pg_stat_statements?.[0]?.values.query, "select exact")
    }
    for (const [index, typeId] of ["1002001", "1002002"].entries()) {
      const url = seen[index]
      assert.equal(url?.searchParams.get("type_id"), typeId)
      assert.deepEqual(url?.searchParams.getAll("field"), ["query"])
      assert.equal(url?.searchParams.has("text"), false)
      assert.equal(url?.searchParams.get("where.queryid"), "9007199254740999")
      assert.equal(url?.searchParams.get("where.userid"), "10")
      assert.equal(url?.searchParams.get("where.dbid"), "20")
      assert.equal(url?.searchParams.has("where.toplevel"), false)
    }
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("an exact locator uses the generic projected snapshot contract", async () => {
  const api = await bundledApi()
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.deepEqual(url.searchParams.getAll("field"), ["queryid", "query"])
    assert.equal(url.searchParams.get("type_id"), "1002002")
    assert.equal(url.searchParams.get("row_ordinal"), "9223372036854775807")
    assert.equal(url.searchParams.get("text"), "160")
    assert.equal(url.searchParams.has("top"), false)
    assert.equal(url.searchParams.has("page_size"), false)
    assert.equal(url.searchParams.has("cursor"), false)
    assert.equal(url.searchParams.has("search"), false)
    assert.equal(url.searchParams.has("by"), false)
    return ndjson([
      {
        record: "layout",
        layout: {
          type_id: "1002002", logical_name: "pg_stat_statements",
          columns: [{ name: "queryid" }, { name: "query" }],
        },
      },
      {
        record: "row", segment_id: "77", type_id: "1002002", ordinal: "9223372036854775807",
        timestamp: String(START), values: ["9", "select located"],
      },
    ])
  }
  try {
    const hour = await api.loadSnapshot(
      "77",
      START,
      [{ section: "pg_stat_statements", typeId: "1002002", fields: ["queryid", "query"] }],
      new AbortController().signal,
      undefined,
      { rowOrdinal: "9223372036854775807" },
    )
    assert.equal(hour.sections.pg_stat_statements?.[0]?.ordinal, "9223372036854775807")
  } finally {
    globalThis.fetch = originalFetch
  }
})

async function bundledApi() {
  const api = await importModule(
    'export * from "../src/api.ts"; export { signInBasic } from "../src/session.ts"',
    { plugins: [registryPlugin(TEST_REGISTRY)] },
  )
  const originalFetch = globalThis.fetch
  globalThis.fetch = async () => new Response(null, { status: 204 })
  try {
    await api.signInBasic("test", "test", new AbortController().signal)
  } finally {
    globalThis.fetch = originalFetch
  }
  return api
}

async function activityWireApi() {
  const api = await importModule(
    'export { loadSeries } from "../src/api.ts"; export { ACTIVITY_COLUMNS, postgresMetricHistoryRequest, postgresMetricHistorySamples } from "../src/postgres-view.tsx"; export { signInBasic } from "../src/session.ts"',
    { plugins: [registryPlugin(TEST_REGISTRY)] },
  )
  const originalFetch = globalThis.fetch
  globalThis.fetch = async () => new Response(null, { status: 204 })
  try {
    await api.signInBasic("test", "test", new AbortController().signal)
  } finally {
    globalThis.fetch = originalFetch
  }
  return api
}

function ndjson(records: readonly unknown[]): Response {
  return new Response(`${records.map((record) => JSON.stringify(record)).join("\n")}\n`, { status: 200 })
}

test("event groups use one half-open request and validate the server-owned shape", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  let request: URL | null = null
  const minutes = Array.from({ length: 60 }, () => 0)
  minutes[3] = 2
  globalThis.fetch = async (input) => {
    request = new URL(String(input), "http://kronika.invalid")
    return ndjson([
      { record: "events", representation: "groups", truncated: true },
      {
        record: "event_group", key: "slow:select ?", section: "pg_log_slow_queries", tier: "notable",
        label: "select ?", count: 2, firstTs: START + 180_000_000, lastTs: START + 180_000_000,
        minutes, stat: { kind: "pg.slow", maxMs: 800, totalMs: 1_200, thresholdMs: 500 },
        detail_ref: "opaque-ref", representativeTs: START + 180_000_000,
      },
    ])
  }
  try {
    const groups = await api.loadEventGroups(
      START,
      START + 300_000_001,
      ["pg_log_errors", "pg_log_slow_queries"],
      new AbortController().signal,
    )
    assert.equal(request?.pathname, "/api/events")
    assert.equal(request?.searchParams.get("from"), String(START))
    assert.equal(request?.searchParams.get("to"), String(START + 300_000_001))
    assert.equal(request?.searchParams.get("representation"), "groups")
    assert.equal(request?.searchParams.get("limit"), "5000")
    assert.deepEqual(request?.searchParams.getAll("source"), ["pg_log_errors", "pg_log_slow_queries"])
    assert.equal(groups.truncated, true)
    assert.equal(groups.rows[0]?.stat.kind, "pg.slow")
    assert.equal(groups.rows[0]?.label, "select ?")
    assert.equal(groups.rows[0]?.detailRef, "opaque-ref")
    assert.equal(groups.rows[0]?.representativeTs, START + 180_000_000)

    globalThis.fetch = async () => ndjson([
      { record: "events", representation: "groups", truncated: false },
      {
        record: "event_group", key: "pgbouncer:3:no such database: nope", section: "pgbouncer_events", tier: "routine",
        label: "no such database: nope", count: 19, firstTs: START, lastTs: START + 1, minutes,
        stat: {
          kind: "pgbouncer.events", level: 3, database: "(nodb)", username: "(nouser)",
          host: "10.0.0.7", sourceFile: "/var/log/pgbouncer.log",
        },
        detail_ref: "opaque-pgbouncer", representativeTs: START,
      },
    ])
    const [pgbouncer] = (await api.loadEventGroups(
      START,
      START + 2,
      ["pgbouncer_events"],
      new AbortController().signal,
    )).rows
    assert.equal(pgbouncer?.label, "no such database: nope")
    assert.deepEqual(pgbouncer?.stat, {
      kind: "pgbouncer.events",
      level: 3,
      database: "(nodb)",
      username: "(nouser)",
      host: "10.0.0.7",
      sourceFile: "/var/log/pgbouncer.log",
    })

    const pgbouncerWithoutOptionalContext = {
      record: "event_group", key: "pgbouncer:3:closing because: client close request", section: "pgbouncer_events", tier: "routine",
      label: "closing because: client close request", count: 1, firstTs: START, lastTs: START, minutes,
      stat: { kind: "pgbouncer.events", level: 3 },
      detail_ref: "opaque-pgbouncer-without-context", representativeTs: START,
    }
    globalThis.fetch = async () => ndjson([
      { record: "events", representation: "groups", truncated: false },
      pgbouncerWithoutOptionalContext,
    ])
    const [withoutOptionalContext] = (await api.loadEventGroups(
      START,
      START + 1,
      ["pgbouncer_events"],
      new AbortController().signal,
    )).rows
    assert.deepEqual(withoutOptionalContext?.stat, {
      kind: "pgbouncer.events",
      level: 3,
      database: null,
      username: null,
      host: null,
      sourceFile: null,
    })

    for (const [field, label] of [
      ["database", "database"],
      ["username", "username"],
      ["host", "host"],
      ["sourceFile", "source file"],
    ] as const) {
      globalThis.fetch = async () => ndjson([
        { record: "events", representation: "groups", truncated: false },
        { ...pgbouncerWithoutOptionalContext, stat: { kind: "pgbouncer.events", level: 3, [field]: 7 } },
      ])
      await assert.rejects(
        api.loadEventGroups(START, START + 1, ["pgbouncer_events"], new AbortController().signal),
        new RegExp(`PgBouncer ${label} is invalid`),
      )
    }

    globalThis.fetch = async () => ndjson([
      { record: "events", representation: "groups", truncated: false },
      {
        record: "event_group", key: "broken", section: "pg_log_errors", tier: "notable", label: null,
        count: 1, firstTs: START, lastTs: START, minutes, stat: { kind: "pg.errors", severity: 0, category: null, sqlstate: null, database: null, username: null },
        detail_ref: "", representativeTs: START,
      },
    ])
    await assert.rejects(
      api.loadEventGroups(START, START + 1, ["pg_log_errors"], new AbortController().signal),
      /event detail reference/,
    )
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("event representative detail uses one opaque bounded request", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  const requested: URL[] = []
  globalThis.fetch = async (input) => {
    requested.push(new URL(String(input), "http://kronika.invalid"))
    return ndjson([{
      record: "row_detail",
      section: "pg_log_errors",
      at: START + 180_000_000,
      fields: {
        sample: { stored_text: "exact stored sample", full_len: "19", truncated: false, sha256: null },
      },
    }])
  }
  try {
    const detail = await api.loadEventRowDetail("opaque+/= ref", new AbortController().signal)
    assert.equal(requested.length, 1)
    assert.equal(requested[0]?.pathname, "/api/row-detail")
    assert.equal(requested[0]?.searchParams.get("detail_ref"), "opaque+/= ref")
    assert.deepEqual([...requested[0]?.searchParams.keys() ?? []], ["detail_ref"])
    assert.equal(detail.section, "pg_log_errors")
    assert.equal(detail.at, START + 180_000_000)
    assert.deepEqual(detail.fields.sample, { stored_text: "exact stored sample", full_len: "19", truncated: false, sha256: null })

    let failedRequests = 0
    globalThis.fetch = async () => {
      failedRequests += 1
      throw new TypeError("detail transport failed")
    }
    await assert.rejects(
      api.loadEventRowDetail("opaque-ref", new AbortController().signal),
      /detail transport failed/,
    )
    assert.equal(failedRequests, 1)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("the bundled review fixture answers detail reads without HTTP", async () => {
  const api = await bundledApi()
  Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: apiFixture() })
  const originalFetch = globalThis.fetch
  let fetches = 0
  globalThis.fetch = async () => {
    fetches += 1
    throw new Error("the bundled fixture reached HTTP")
  }
  try {
    const timeline = await api.loadTimeline(START, new AbortController().signal)
    assert.deepEqual(
      [...new Set(timeline.lanePoints.map((point) => point.lane))].sort(),
      ["cpu_busy", "memory", "pg_oldest_xact", "pg_running", "pg_waiting"],
    )
    assert.equal(timeline.lanePoints.find((point) => point.lane === "memory")?.value, 40)
    const series = await api.loadSeries(
      START,
      "os_process",
      { pid: "41" },
      ["read_bytes"],
      new AbortController().signal,
    )
    assert.equal(series.length, 2)
    assert.deepEqual(series[1]?.values, { read_bytes: 160 })
    const snapshot = await api.loadSnapshot(
      "segment-a",
      START + 2,
      [{ section: "pg_stat_activity", fields: ["query"], typeId: "1001004" }],
      new AbortController().signal,
      undefined,
      { filters: { pid: "7" }, typeId: "1001004", fullText: true },
    )
    assert.deepEqual(snapshot.activities[0]?.values, { query: "select exact" })
    const beforeFirstSample = await api.loadSnapshot(
      "segment-a",
      START,
      ["pg_stat_activity"],
      new AbortController().signal,
    )
    assert.equal(beforeFirstSample.activities.length, 0)
    const repeated = await api.loadSnapshot(
      "segment-a",
      START + 2,
      [
        { section: "pg_stat_activity", fields: ["pid"], typeId: "1001004" },
        { section: "pg_stat_activity", fields: ["query"], typeId: "1001004" },
      ],
      new AbortController().signal,
    )
    assert.deepEqual(repeated.activities.map((row) => row.values), [{ pid: 7 }, { query: "select exact" }])
    const boundary = apiFixture()
    const secondHour = START + 3_600_000_000
    boundary.meta.captureToUs = String(secondHour + 2)
    boundary.os.snapshots.push({
      segment_id: "segment-a",
      ts: String(secondHour + 1),
      type_id: "1100001",
      rows: [[String(secondHour + 1), "6", 42, "100", 170]],
    })
    boundary.system.cpuBusy.push([String(secondHour + 1), 35, "segment-a"])
    Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: boundary })
    const selectedHour = await api.loadTimeline(secondHour, new AbortController().signal)
    assert.equal(selectedHour.hour, secondHour)
    assert.ok(selectedHour.lanes.os_process?.every((row) =>
      row.timestamp >= secondHour && row.timestamp < secondHour + 3_600_000_000))
    assert.ok(selectedHour.points.every((point) =>
      point.timestamp >= secondHour && point.timestamp < secondHour + 3_600_000_000))
    assert.equal(fetches, 0)
  } finally {
    globalThis.fetch = originalFetch
    Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  }
})

function apiFixture() {
  const point = (offset: number, value: number | null) => [String(START + offset), value, "segment-a"]
  return {
    meta: {
      captureFromUs: String(START),
      captureToUs: String(START + 2),
      segments: 1,
    },
    findings: [],
    os: {
      columns: ["ts", "ordinal", "pid", "starttime", "read_bytes"],
      snapshots: [
        {
          segment_id: "segment-a",
          ts: String(START + 1),
          type_id: "1100001",
          rows: [[String(START + 1), "4", 41, "99", 100]],
        },
        {
          segment_id: "segment-a",
          ts: String(START + 2),
          type_id: "1100001",
          rows: [[String(START + 2), "5", 41, "99", 160]],
        },
      ],
    },
    pg: {
      columns: [
        "ts", "ordinal", "pid", "leader_pid", "backend_type", "state",
        "wait_event_type", "xact_start", "query",
      ],
      snapshots: [{
        segment_id: "segment-a",
        ts: String(START + 1),
        type_id: "1001004",
        rows: [[String(START + 1), "8", 7, null, "client backend", "active", null, String(START), "select exact"]],
      }],
    },
    system: {
      cpuBusy: [point(1, 25)],
      health: [point(1, 75)],
      load1: [point(1, 1)],
      memAvailable: [point(1, 60)],
      minFsFree: [point(1, 80)],
      oom: [point(1, 0)],
      psi: {},
    },
  }
}

test("a snapshot is keyed on the stored sample the cursor rests on", async () => {
  const api = await bundledApi()
  const line = [START + 3_000_000, START + 6_000_000, START + 9_000_000].map((timestamp, index) => ({
    segmentId: "7", logicalName: "health", typeId: "0", ordinal: String(index), timestamp, values: {},
  }))
  assert.equal(api.sampleAt(line, START + 7_500_000), START + 6_000_000, "between samples the earlier one answers")
  assert.equal(api.sampleAt(line, START + 6_000_000), START + 6_000_000, "on a sample it is itself")
  assert.equal(api.sampleAt(line, START + 1_000_000), START + 3_000_000, "before the first, the first")
  assert.equal(api.sampleAt([], START), null)
})

test("the current view replaces every prior snapshot while the hour line remains", async () => {
  const api = await bundledApi()
  const row = (logicalName: string, timestamp: number) => ({
    segmentId: "7", logicalName, typeId: "1100001", ordinal: "0", timestamp, values: { pid: 1 },
  })
  const health = row("health", START + 1)
  const timeline = api.hourOf({
    hour: START, availableHours: [START], segments: [], lanes: { health: [health] }, health: [health],
    points: [], lanePoints: [], laneContexts: [], findings: [], availableSections: ["os_process", "pg_stat_activity"],
  })
  const processView = api.viewData(timeline, {
    sections: { os_process: [row("os_process", START + 2)] }, availableSections: ["os_process"],
    points: [], lanePoints: [], laneContexts: [], findings: [],
  })
  const activityView = api.viewData(timeline, {
    sections: { pg_stat_activity: [row("pg_stat_activity", START + 3)] }, availableSections: ["pg_stat_activity"],
    points: [], lanePoints: [], laneContexts: [], findings: [],
  })
  assert.equal(processView.processes[0]?.timestamp, START + 2)
  assert.equal(activityView.processes.length, 0)
  assert.equal(activityView.activities[0]?.timestamp, START + 3)
  assert.deepEqual(activityView.health, [health])
  assert.deepEqual(activityView.availableSections, ["os_process", "pg_stat_activity"])
})

test("the timeline carries every finding without per-section index requests", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  const seen: URL[] = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(url)
    return ndjson([
      { record: "hour", from: String(START + 1), to: String(START + 9), available_hours: [String(START)] },
      {
        record: "finished_segment", id: "7", min_ts: String(START), max_ts: String(START + 10),
        sections: [
          { logical_name: "os_process", type_id: "1100001" },
          { logical_name: "pg_stat_activity", type_id: "1001004" },
          { logical_name: "pg_stat_statements", type_id: "1002002" },
          { logical_name: "pg_log_errors", type_id: "2001001" },
        ],
      },
      { record: "index", segment: { id: "7" }, logical_name: "health", checksum: null },
      { record: "finding", logical_name: "os_process", kind: "spike", type_id: "1100001", field_ordinal: 33, row_ordinal: "1", ts: String(START + 1) },
      { record: "finding", logical_name: "pg_stat_activity", kind: "known_bad", type_id: "1001004", field_ordinal: 0, row_ordinal: "2", ts: String(START + 2) },
      { record: "finding", logical_name: "pg_stat_statements", kind: "spike", type_id: "1002002", field_ordinal: 10, row_ordinal: "3", ts: String(START + 3) },
      { record: "finding", logical_name: "pg_log_errors", kind: "event", type_id: "2001001", field_ordinal: 0, row_ordinal: "4", ts: String(START + 4), category: 8 },
    ])
  }
  try {
    const timeline = await api.loadTimeline(
      START,
      new AbortController().signal,
      undefined,
      { from: START + 1, toExclusive: START + 10 },
    )
    assert.equal(seen.length, 1)
    assert.equal(seen[0]?.pathname, "/api/hour")
    assert.equal(seen[0]?.searchParams.get("part"), "base")
    assert.equal(seen[0]?.searchParams.get("from"), String(START + 1))
    assert.equal(seen[0]?.searchParams.get("to"), String(START + 9))
    assert.equal(timeline.hour, START)
    assert.deepEqual(timeline.findings.map((item) => [item.segmentId, item.logicalName, item.rowOrdinal]), [
      ["7", "os_process", "1"],
      ["7", "pg_stat_activity", "2"],
      ["7", "pg_stat_statements", "3"],
      ["7", "pg_log_errors", "4"],
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("timeline base is lane-free and the atomic background result retains segment and null", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async () => ndjson([
    { record: "hour", from: String(START), to: String(START + 3_600_000_000 - 1), available_hours: [String(START)] },
    {
      record: "finished_segment", id: "segment-a", min_ts: String(START), max_ts: String(START + 10),
      sections: [{ logical_name: "pg_stat_statements", type_id: "1002002" }],
    },
    { record: "index", segment: { id: "segment-a" }, logical_name: "health", checksum: null },
    { record: "point", type_id: "0", series: "overall_health", ts: String(START + 2), identity: {}, value: null },
    { record: "point", type_id: "0", series: "overall_health", ts: String(START + 2), identity: {}, value: 42 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(START + 3), identity: {}, value: 42 },
    { record: "point", type_id: "0", series: "overall_health", ts: String(START + 3), identity: {}, value: null },
    { record: "lane", segment_id: "segment-a", lane: "cpu_busy", ts: String(START + 1), value: null },
  ])
  try {
    const timeline = await api.loadTimeline(START, new AbortController().signal)
    assert.deepEqual(timeline.lanePoints, [])
    const lanes = await api.loadTimelineLanes(timeline, new AbortController().signal)
    assert.deepEqual(lanes.points, [{ segmentId: "segment-a", lane: "cpu_busy", timestamp: START + 1, value: null }])
    const complete = api.withTimelineLanes(api.hourOf(timeline), lanes)
    assert.deepEqual(complete.lanePoints, lanes.points)
    assert.deepEqual(timeline.health.map((row) => row.values.overall_health), [null, null])
    assert.deepEqual(timeline.segments[0]?.sections, [{ logicalName: "pg_stat_statements", typeId: "1002002" }])
    assert.equal(api.fieldNameForLocator({ typeId: "0", fieldOrdinal: 0 }), "os_health")
    assert.equal(api.fieldNameForLocator({ typeId: "0", fieldOrdinal: 1 }), "overall_health")
    assert.equal(api.fieldNameForLocator({ typeId: "0", fieldOrdinal: 2 }), null)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("background lanes carry the exact ordered base segments and lossless active WAL pin", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  const seen: URL[] = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(url)
    if (url.searchParams.get("part") === "lanes") return ndjson([
      { record: "hour", from: String(START), to: String(START + 3_600_000_000 - 1) },
    ])
    return ndjson([
      { record: "hour", from: String(START), to: String(START + 3_600_000_000 - 1), available_hours: [String(START)] },
      { record: "finished_segment", id: "41", min_ts: String(START), max_ts: String(START + 1), sections: [] },
      {
        record: "active_segment", id: "42", min_ts: String(START + 2), max_ts: String(START + 3), sections: [],
        cursor: { segment_id: "42", wal_position: "18446744073709551615" },
      },
    ])
  }
  try {
    const timeline = await api.loadTimeline(START, new AbortController().signal)
    assert.equal(timeline.segments[1]?.activeWalPosition, "18446744073709551615")
    await api.loadTimelineLanes(
      timeline,
      new AbortController().signal,
      { from: START + 1, toExclusive: START + 10 },
    )
    const background = seen[1]
    assert.equal(background?.searchParams.get("segments"), "41,42")
    assert.equal(background?.searchParams.get("active"), "42,18446744073709551615")
    assert.equal(background?.searchParams.get("from"), String(START + 1))
    assert.equal(background?.searchParams.get("to"), String(START + 9))
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("timeline source presence does not trust a straddling segment inventory", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async () => ndjson([
    { record: "hour", from: String(START), to: String(START + 3_600_000_000 - 1), available_hours: [String(START)] },
    {
      record: "catalog", from: String(START), to: String(START + 3_600_000_000 - 1), demo: "synthetic",
      source_families: [{ name: "postgresql", configured: false, present: true, metrics_present: false }],
    },
    {
      record: "finished_segment", id: "segment-a", min_ts: String(START - 1), max_ts: String(START + 1),
      sections: [{ logical_name: "pg_stat_database", type_id: "1005001" }],
    },
  ])
  try {
    const timeline = await api.loadTimeline(START, new AbortController().signal)
    assert.deepEqual(timeline.availableSections, ["pg_stat_database"])
    assert.equal(timeline.postgresqlConfigured, false)
    assert.equal(timeline.postgresqlPresent, false)
    assert.equal(timeline.syntheticDemo, true)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("health rows align raw PostgreSQL points to stored nonfuture evaluations", async () => {
  const api = await bundledApi()
  const point = (segmentId: string, series: string, timestamp: number, value: number | null) => ({
    identity: {}, logicalName: "health", segmentId, series, timestamp, typeId: "0", value,
  })
  const metadata = [{ segmentId: "a", postgresqlIntervalSeconds: 1, environment: null }]
  const rows = api.healthRows([
    point("a", "postgres_health", START + 100, 72),
    point("a", "os_health", START + 101, 91),
    point("a", "os_health", START + 103, 90),
    point("a", "overall_health", START + 103, 62),
    point("a", "postgres_health", START + 106, 55),
    point("a", "os_health", START + 109, 90),
    point("a", "overall_health", START + 109, 45),
    point("a", "os_health", START + 112, 90),
    point("a", "overall_health", START + 112, null),
    point("a", "postgres_health", START + 113, 44),
    point("a", "os_health", START + 114, null),
    point("a", "overall_health", START + 114, null),
    point("b", "os_health", START + 115, 88),
    point("b", "overall_health", START + 115, 88),
    point("a", "postgres_health", START + 116, null),
    point("a", "os_health", START + 116, null),
    point("a", "overall_health", START + 116, null),
    point("a", "postgres_health", START + 120, 1),
    point("a", "os_health", START + 2_000_121, null),
    point("a", "overall_health", START + 2_000_121, null),
  ], metadata)
  assert.deepEqual(rows.map((row) => [row.segmentId, row.timestamp, row.values]), [
    ["a", START + 103, { overall_health: 62, os_health: 90, postgres_health: 72 }],
    ["a", START + 109, { overall_health: 45, os_health: 90, postgres_health: 55 }],
    ["a", START + 112, { overall_health: null, os_health: 90, postgres_health: null }],
    ["a", START + 114, { overall_health: null, os_health: null, postgres_health: 44 }],
    ["b", START + 115, { overall_health: 88, os_health: 88 }],
    ["a", START + 116, { overall_health: null, os_health: null, postgres_health: null }],
    ["a", START + 2_000_121, { overall_health: null, os_health: null, postgres_health: null }],
  ])
  assert.equal(rows.some((row) => row.timestamp === START + 101), false)
  assert.equal(Object.hasOwn(rows[4]!.values, "postgres_health"), false)
})

test("timeline reads the stored PostgreSQL freshness interval for combined health", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  const seen: string[] = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(`${url.pathname}?${url.searchParams}`)
    if (url.searchParams.get("part") === "lanes") return ndjson([
      { record: "hour", from: String(START), to: String(START + 3_600_000_000 - 1) },
      { record: "lane_context", segment_id: "a", postgresql_interval_seconds: "1", environment: 1 },
    ])
    return ndjson([
      { record: "hour", from: String(START), to: String(START + 3_600_000_000 - 1), available_hours: [String(START)] },
      { record: "finished_segment", id: "a", min_ts: String(START), max_ts: String(START + 2_000_001), sections: [] },
      { record: "index", segment: { id: "a" }, logical_name: "health", checksum: null },
      { record: "point", type_id: "0", series: "postgres_health", ts: String(START), identity: {}, value: 72 },
      { record: "point", type_id: "0", series: "os_health", ts: String(START + 500_000), identity: {}, value: null },
      { record: "point", type_id: "0", series: "overall_health", ts: String(START + 500_000), identity: {}, value: null },
      { record: "point", type_id: "0", series: "os_health", ts: String(START + 2_000_001), identity: {}, value: null },
      { record: "point", type_id: "0", series: "overall_health", ts: String(START + 2_000_001), identity: {}, value: null },
    ])
  }
  try {
    const timeline = await api.loadTimeline(START, new AbortController().signal)
    assert.deepEqual(timeline.health.map((row) => row.values), [
      { overall_health: null, os_health: null, postgres_health: null },
      { overall_health: null, os_health: null, postgres_health: null },
    ])
    const lanes = await api.loadTimelineLanes(timeline, new AbortController().signal)
    const complete = api.withTimelineLanes(api.hourOf(timeline), lanes)
    assert.deepEqual(complete.health.map((row) => row.values), [
      { overall_health: null, os_health: null, postgres_health: 72 },
      { overall_health: null, os_health: null, postgres_health: null },
    ])
    assert.equal(seen.length, 2)
    const base = new URL(seen[0]!, "http://kronika.invalid")
    const background = new URL(seen[1]!, "http://kronika.invalid")
    assert.equal(base.searchParams.get("part"), "base")
    assert.equal(background.searchParams.get("part"), "lanes")
    assert.equal(background.searchParams.get("segments"), "a")
    assert.equal(seen.some((request) => request.includes("/snapshot")), false)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("snapshot segment selection uses the newest overlap without jumping from blank early time", async () => {
  const api = await bundledApi()
  const segments = [
    { id: "100", minTs: 100, maxTs: 300, sections: [] },
    { id: "200", minTs: 150, maxTs: 300, sections: [] },
    { id: "300", minTs: 301, maxTs: 400, sections: [] },
  ]
  assert.equal(api.segmentBoundAt(segments, 50), null)
  assert.equal(api.segmentBoundAt(segments, 150)?.id, "200")
  assert.equal(api.segmentBoundAt(segments, 250)?.id, "200")
  assert.equal(api.segmentBoundAt(segments, 350)?.id, "300")
})

test("snapshot requests choose and group the newest compatible layout anchors", async () => {
  const api = await bundledApi()
  const segments = [
    {
      id: "100", minTs: START - 10, maxTs: START,
      sections: [
        { logicalName: "pg_stat_activity", typeId: "1001001" },
        { logicalName: "os_loadavg", typeId: "1105001" },
      ],
    },
    {
      id: "200", minTs: START - 10, maxTs: START,
      sections: [{ logicalName: "pg_stat_activity", typeId: "1001004" }],
    },
    {
      id: "300", minTs: START - 5, maxTs: START,
      sections: [
        { logicalName: "os_loadavg", typeId: "1105001" },
        { logicalName: "pg_stat_statements", typeId: "1002002" },
      ],
    },
    {
      id: "400", minTs: START + 1, maxTs: START + 10,
      sections: [{ logicalName: "pg_stat_activity", typeId: "1001001" }],
    },
  ]
  const groups = api.snapshotRequestGroups(segments, START, [
    { section: "os_loadavg", fields: ["load1"] },
    {
      section: "pg_stat_activity",
      typeIds: ["1001001", "1001004"],
      fieldsByType: { "1001001": ["pid"], "1001004": ["pid", "backend_type"] },
    },
    {
      section: "pg_stat_statements",
      typeIds: ["1002001", "1002002"],
      fieldsByType: { "1002001": ["queryid", "calls"], "1002002": ["queryid", "calls"] },
      pageSize: 20,
      defaultOrder: ["calls"],
    },
  ])
  assert.deepEqual(groups.map((group) => group.anchor.id), ["300", "100", "200"])
  assert.deepEqual(groups.map((group) => group.requests.map((request) => [
    request.section,
    request.typeId ?? null,
    request.pageSize ?? null,
  ])), [
    [["os_loadavg", null, null], ["pg_stat_statements", null, 20]],
    [["pg_stat_activity", "1001001", null]],
    [["pg_stat_activity", "1001004", null]],
  ])

  const exactOldLayout = api.snapshotRequestGroups(segments, START, [{
    section: "pg_stat_activity", typeId: "1001001", fields: ["pid"],
  }])
  assert.deepEqual(exactOldLayout.map((group) => group.anchor.id), ["100"])
})

test("heatmaps use compact automatic named labels without a label selector", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.pathname, "/api/heatmap")
    assert.deepEqual(url.searchParams.getAll("field"), ["total_exec_time"])
    assert.equal(url.searchParams.has("label"), false)
    return ndjson([
      {
        record: "heatmap", class: "cumulative", summary: "sum", entity_count: "1", others_count: "0",
        labels: ["datname", "usename"],
        intervals: [{ start: String(START), end: String(START + 3_600_000_000 - 1) }],
      },
      {
        record: "heatmap_row", type_id: "1002002", identity: ["42", "10", "5"],
        labels: ["app", "reporter"],
        total: 10, cells: [1],
      },
      { record: "heatmap_band", band: "totals", total: 10, cells: [1] },
      { record: "heatmap_band", band: "others", total: null, cells: [null] },
    ])
  }
  try {
    const view = await api.loadHeatmap(
      START,
      "pg_stat_statements",
      ["total_exec_time"],
      1,
      25,
      new AbortController().signal,
    )
    assert.deepEqual(view.rows[0]?.labels, {
      datname: "app",
      usename: "reporter",
    })
    assert.equal(view.summary, "sum")
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("heatmaps preserve summary modes and reject unknown or still-missing modes after one reload", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  let summary: string | undefined = "mean"
  const cacheModes: (RequestCache | undefined)[] = []
  globalThis.fetch = async (_input, init) => {
    cacheModes.push(init?.cache)
    return ndjson([{ record: "heatmap", class: "gauge", summary, intervals: [] }])
  }
  const load = () => api.loadHeatmap(START, "os_process", ["rmem_kb"], 1, 25, new AbortController().signal)
  try {
    assert.equal((await load()).summary, "mean")
    summary = "max"
    assert.equal((await load()).summary, "max")
    summary = "unknown"
    await assert.rejects(load(), /heatmap summary is invalid/)
    assert.deepEqual(cacheModes, ["default", "default", "default"])
    summary = undefined
    await assert.rejects(load(), /heatmap summary is invalid/)
    assert.deepEqual(cacheModes.slice(3), ["default", "reload"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("heatmaps replace a cached legacy response once at the same URL", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  const calls: { path: string; cache: RequestCache | undefined }[] = []
  globalThis.fetch = async (input, init) => {
    calls.push({ path: String(input), cache: init?.cache })
    return ndjson([
      { record: "heatmap", class: "gauge", ...(calls.length === 1 ? {} : { summary: "mean" }), intervals: [] },
      { record: "heatmap_band", band: "totals", total: calls.length === 1 ? 900 : 300, cells: [] },
    ])
  }
  try {
    const load = () => api.loadHeatmap(START, "os_process", ["rmem_kb"], 1, 25, new AbortController().signal)
    const view = await load()
    assert.equal(view.summary, "mean")
    assert.equal(view.totals.total, 300)
    assert.equal(calls.length, 2)
    assert.equal(calls[0]?.path, calls[1]?.path)
    assert.deepEqual(calls.map(({ cache }) => cache), ["default", "reload"])
    await load()
    assert.deepEqual(calls.map(({ cache }) => cache), ["default", "reload", "default"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("related plan query text uses one bounded query-ID-only first match", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const segments = [
    {
      id: "100", minTs: START - 20, maxTs: START - 1,
      sections: [{ logicalName: "pg_stat_statements", typeId: "1002001" }],
    },
    {
      id: "200", minTs: START, maxTs: START + 20,
      sections: [{ logicalName: "pg_stat_statements", typeId: "1002002" }],
    },
  ]
  const originalFetch = globalThis.fetch
  const seen: URL[] = []
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(url)
    const columns = ["query"]
    return ndjson([
      {
        record: "layout", layout: { type_id: "1002002", logical_name: "pg_stat_statements", columns: columns.map((name) => ({ name })) },
      },
      {
        record: "row", segment_id: "200", type_id: "1002002", ordinal: "42", timestamp: String(START),
        values: ["  select current\nfrom jobs"],
      },
      {
        record: "snapshot_page", logical_name: "pg_stat_statements", eligible: "1", returned: "1",
        has_more: true, truncated: false, next_cursor: "must-not-be-read", page_size: 1,
        order_by: [], order_direction: "desc", from: null, to: String(START),
      },
    ])
  }
  try {
    const row = await api.loadRelatedStatementTextRow(segments, START, "42", new AbortController().signal)
    assert.deepEqual(row === null ? null : [row.segmentId, row.timestamp, row.values.query], [
      "200", START, "  select current\nfrom jobs",
    ])
    assert.equal(seen.length, 1)
    const url = seen[0]
    assert.equal(url?.pathname, "/api/segments/200/snapshot")
    assert.equal(url?.searchParams.get("at"), String(START))
    assert.equal(url?.searchParams.get("search"), "query_id:42")
    assert.equal(url?.searchParams.get("page_size"), "1")
    assert.equal(url?.searchParams.get("first_match"), "1")
    assert.deepEqual(url?.searchParams.getAll("field"), ["query"])
    assert.equal(url?.searchParams.has("text"), false)
    assert.equal(url?.searchParams.has("type_id"), false)
    assert.equal(url?.searchParams.has("cursor"), false)
    assert.equal(url?.searchParams.has("by"), false)
    assert.equal(url?.searchParams.get("search")?.includes("database"), false)
    assert.equal(url?.searchParams.get("search")?.includes("role"), false)
    assert.equal([...(url?.searchParams.keys() ?? [])].some((key) => key.startsWith("where.")), false)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("grouped snapshot loads merge equal-time layouts and retain physical row sources", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const segments = [
    {
      id: "100", minTs: START, maxTs: START,
      sections: [{ logicalName: "pg_stat_activity", typeId: "1001001" }],
    },
    {
      id: "200", minTs: START, maxTs: START,
      sections: [{ logicalName: "pg_stat_activity", typeId: "1001004" }],
    },
  ]
  const groups = api.snapshotRequestGroups(segments, START, [{
    section: "pg_stat_activity",
    typeIds: ["1001001", "1001004"],
    fieldsByType: { "1001001": ["pid", "state"], "1001004": ["pid", "backend_type"] },
  }])
  const seen: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    seen.push(`${url.pathname}:${url.searchParams.get("type_id")}`)
    const typeId = url.searchParams.get("type_id")!
    const old = typeId === "1001001"
    return ndjson([
      {
        record: "layout", rates: old ? ["state"] : ["backend_type"],
        layout: {
          type_id: typeId, logical_name: "pg_stat_activity",
          columns: (old ? ["pid", "state"] : ["pid", "backend_type"]).map((name) => ({ name })),
        },
      },
      {
        record: "row", segment_id: old ? "91" : "191", type_id: typeId, ordinal: "0",
        timestamp: String(START), values: old ? [1, "active"] : [2, "client backend"],
      },
    ])
  }
  try {
    const snapshot = await api.loadSnapshotGroups(groups, START, new AbortController().signal)
    assert.deepEqual(seen, [
      "/api/segments/100/snapshot:1001001",
      "/api/segments/200/snapshot:1001004",
    ])
    assert.deepEqual(snapshot.sections.pg_stat_activity?.map((row) => [row.segmentId, row.typeId]), [
      ["91", "1001001"],
      ["191", "1001004"],
    ])
    assert.deepEqual(snapshot.rateColumns.pg_stat_activity, ["state", "backend_type"])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("projected history retains segment provenance and exact type", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.pathname, "/api/hour")
    assert.equal(url.searchParams.get("from"), String(START))
    assert.equal(url.searchParams.get("to"), String(START + 3_600_000_000 - 1))
    assert.deepEqual(url.searchParams.getAll("field"), ["calls", "total_exec_time"])
    assert.equal(url.searchParams.get("type_id"), "1002002")
    assert.equal(url.searchParams.get("where.queryid"), "41")
    return ndjson([
      { record: "series_segment", segment: { id: "segment-a" } },
      {
        record: "layout",
        layout: {
          type_id: "1002002", logical_name: "pg_stat_statements",
          columns: [{ name: "calls" }, { name: "total_exec_time" }],
        },
      },
      { record: "row", type_id: "1002002", ordinal: "2", timestamp: String(START + 1), values: [2, 8] },
      { record: "series_segment", segment: { id: "segment-b" } },
      { record: "row", type_id: "1002002", ordinal: "3", timestamp: String(START + 2), values: [3, 12] },
    ])
  }
  try {
    const rows = await api.loadSeries(
      START + 5,
      "pg_stat_statements",
      { queryid: "41" },
      ["calls", "total_exec_time"],
      new AbortController().signal,
      "1002002",
    )
    assert.deepEqual(rows.map((row) => [row.segmentId, row.typeId]), [
      ["segment-a", "1002002"],
      ["segment-b", "1002002"],
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a read-only resource retries one response-body transport failure", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  let attempts = 0
  globalThis.fetch = async () => {
    attempts += 1
    if (attempts === 1) {
      return new Response(new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(`${JSON.stringify({ record: "padding", value: "x".repeat(9_000) })}\n`))
          controller.error(new TypeError("response body closed during journal rollover"))
        },
      }))
    }
    return ndjson([
      { record: "series_segment", segment: { id: "segment-a" } },
      {
        record: "layout",
        layout: {
          type_id: "1105001", logical_name: "os_loadavg", columns: [{ name: "load1" }],
        },
      },
      { record: "row", type_id: "1105001", ordinal: "0", timestamp: String(START), values: [1.5] },
    ])
  }
  try {
    const rows = await api.loadSeries(
      START,
      "os_loadavg",
      {},
      ["load1"],
      new AbortController().signal,
    )
    assert.equal(attempts, 2)
    assert.deepEqual(rows.map((row) => row.values), [{ load1: 1.5 }])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("a read-only resource does not retry a client refusal", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  let attempts = 0
  globalThis.fetch = async () => {
    attempts += 1
    return new Response(null, { status: 400 })
  }
  try {
    await assert.rejects(api.loadSeries(
      START,
      "os_loadavg",
      {},
      ["load1"],
      new AbortController().signal,
    ), /HTTP 400/)
    assert.equal(attempts, 1)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("Activity history uses its exact production projection and yields numeric durations", async () => {
  const api = await activityWireApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const selected = {
    logicalName: "pg_stat_activity", ordinal: "8", segmentId: "segment-a",
    timestamp: START + 20_000_000, typeId: "1001004",
    values: {
      pid: 4242,
      state: "active",
      query_start: String(START + 12_000_000),
      query_duration_ms: 8_000,
    },
  }
  const column = api.ACTIVITY_COLUMNS.find(({ field }) => field === "query_duration_ms")
  assert.ok(column)
  const request = api.postgresMetricHistoryRequest(selected, "pg_stat_activity", column)
  assert.deepEqual(request.fields, ["pid", "state", "query_start"])
  assert.deepEqual(request.filters, { pid: "4242" })

  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.pathname, "/api/hour")
    assert.equal(url.searchParams.get("from"), String(START))
    assert.equal(url.searchParams.get("to"), String(START + 3_600_000_000 - 1))
    assert.equal(url.searchParams.get("section"), "pg_stat_activity")
    assert.deepEqual(url.searchParams.getAll("field"), ["pid", "state", "query_start"])
    assert.deepEqual([...url.searchParams.keys()].filter((name) => name.startsWith("where.")), ["where.pid"])
    assert.equal(url.searchParams.get("where.pid"), "4242")
    assert.equal(url.searchParams.has("type_id"), false)
    return ndjson([
      { record: "series_segment", segment: { id: "segment-a" } },
      {
        record: "layout",
        layout: {
          type_id: "1001004", logical_name: "pg_stat_activity",
          columns: request.fields.map((name) => ({ name })),
        },
      },
      {
        record: "row", type_id: "1001004", ordinal: "7", timestamp: String(START + 10_000_000),
        values: [4242, "active", String(START + 5_000_000)],
      },
      {
        record: "row", type_id: "1001004", ordinal: "8", timestamp: String(START + 20_000_000),
        values: [4242, "active", String(START + 12_000_000)],
      },
    ])
  }
  try {
    const rows = await api.loadSeries(
      selected.timestamp,
      "pg_stat_activity",
      request.filters,
      request.fields,
      new AbortController().signal,
    )
    assert.deepEqual(rows.map(({ values }) => values), [
      { pid: 4242, state: "active", query_start: String(START + 5_000_000) },
      { pid: 4242, state: "active", query_start: String(START + 12_000_000) },
    ])
    assert.deepEqual(rows.map(({ values }) => Object.keys(values)), [request.fields, request.fields])
    const samples = api.postgresMetricHistorySamples(rows, selected, "pg_stat_activity", column, request)
    assert.deepEqual(samples.map(({ value }) => value), [5_000, 8_000])
    assert.equal(samples.every(({ value }) => typeof value === "number" && Number.isFinite(value)), true)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("projected history accepts a synthetic layout logical name", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async () => ndjson([
    { record: "series_segment", segment: { id: "segment-a" } },
    {
      record: "layout",
      layout: {
        type_id: "0", logical_name: "os_process_summary",
        columns: [{ name: "processes" }, { name: "user_cores" }],
      },
    },
    { record: "row", type_id: "0", ordinal: "0", timestamp: String(START), values: [205, 3.5] },
  ])
  try {
    const rows = await api.loadSeries(
      START,
      "os_process_summary",
      {},
      ["processes", "user_cores"],
      new AbortController().signal,
    )
    assert.equal(rows[0]?.logicalName, "os_process_summary")
    assert.deepEqual(rows[0]?.values, { processes: 205, user_cores: 3.5 })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("grouped relation history sends its full identity and parses semantic rows", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.pathname, "/api/hour")
    assert.equal(url.searchParams.get("from"), String(START))
    assert.equal(url.searchParams.get("to"), String(START + 3_600_000_000 - 1))
    assert.equal(url.searchParams.get("group"), "schema")
    assert.equal(url.searchParams.get("type_id"), null)
    assert.deepEqual(url.searchParams.getAll("field"), ["dml_total", "dead_pct"])
    assert.equal(url.searchParams.get("where.datid"), "42")
    assert.equal(url.searchParams.get("where.schemaname"), "public")
    return ndjson([
      {
        record: "relation_layout", logical_name: "pg_stat_user_tables", group: "schema",
        columns: [
          { name: "dml_total", kind: "number", unit: "per_second", nullable: true },
          { name: "dead_pct", kind: "number", unit: "percent", nullable: true },
        ],
      },
      { record: "series_segment", segment: { id: "segment-a" } },
      {
        record: "relation", logical_name: "pg_stat_user_tables", group: "schema",
        key: { datid: "42", datname: "app", schemaname: "public" },
        values: { dml_total: 3.5, dead_pct: null }, sample_from: String(START - 5), sample_to: String(START), source: null,
      },
      { record: "series_segment", segment: { id: "segment-b" } },
      {
        record: "relation", logical_name: "pg_stat_user_tables", group: "schema",
        key: { datid: "42", datname: "app", schemaname: "public" },
        values: { dml_total: 7, dead_pct: 12.5 }, sample_from: String(START), sample_to: String(START + 5), source: null,
      },
    ])
  }
  try {
    const rows = await api.loadSeries(
      START,
      "pg_stat_user_tables",
      { datid: "42", schemaname: "public" },
      ["dml_total", "dead_pct"],
      new AbortController().signal,
      undefined,
      "schema",
    )
    assert.deepEqual(rows.map((row) => ({
      segmentId: row.segmentId,
      timestamp: row.timestamp,
      relation: row.relation,
      typeId: row.typeId,
      ordinal: row.ordinal,
      values: row.values,
    })), [
      {
        segmentId: "segment-a", timestamp: START, relation: { group: "schema" }, typeId: "", ordinal: "",
        values: { datid: "42", datname: "app", schemaname: "public", dml_total: 3.5, dead_pct: null },
      },
      {
        segmentId: "segment-b", timestamp: START + 5, relation: { group: "schema" }, typeId: "", ordinal: "",
        values: { datid: "42", datname: "app", schemaname: "public", dml_total: 7, dead_pct: 12.5 },
      },
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("tablespace history sends only the cluster-wide OID identity", async () => {
  const api = await bundledApi()
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (input) => {
    const url = new URL(String(input), "http://kronika.invalid")
    assert.equal(url.searchParams.get("group"), "tablespace")
    assert.equal(url.searchParams.get("where.tablespace_oid"), "16384")
    assert.equal(url.searchParams.get("where.datid"), null)
    return ndjson([
      {
        record: "relation_layout", logical_name: "pg_stat_user_indexes", group: "tablespace",
        columns: [{ name: "main_fork_bytes", kind: "number", unit: "bytes", nullable: true }],
      },
      { record: "series_segment", segment: { id: "segment-a" } },
      {
        record: "relation", logical_name: "pg_stat_user_indexes", group: "tablespace",
        key: { tablespace_oid: "16384" }, values: { main_fork_bytes: 8192 },
        sample_from: String(START - 5), sample_to: String(START), source: null,
      },
    ])
  }
  try {
    const rows = await api.loadSeries(
      START,
      "pg_stat_user_indexes",
      { tablespace_oid: "16384" },
      ["main_fork_bytes"],
      new AbortController().signal,
      undefined,
      "tablespace",
    )
    assert.deepEqual(rows[0]?.values, { tablespace_oid: "16384", main_fork_bytes: 8192 })
    assert.deepEqual(rows[0]?.relation, { group: "tablespace" })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("the recorded environment comes from the newest lane context that states it", async () => {
  const api = await importModule(
    'export { recordedEnvironmentFromContexts } from "../src/api.ts"',
    { plugins: [registryPlugin(TEST_REGISTRY)] },
  )
  const context = (segmentId, environment) => ({ segmentId, postgresqlIntervalSeconds: null, environment })
  assert.equal(api.recordedEnvironmentFromContexts([]), null)
  assert.equal(api.recordedEnvironmentFromContexts([context("a", null)]), null)
  assert.equal(api.recordedEnvironmentFromContexts([context("a", 0)]), "machine")
  assert.equal(api.recordedEnvironmentFromContexts([context("a", 0), context("b", 1), context("c", null)]), "container")
  assert.equal(api.recordedEnvironmentFromContexts([context("a", 7)]), null)
})
