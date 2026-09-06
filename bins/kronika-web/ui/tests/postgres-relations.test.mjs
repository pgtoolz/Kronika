import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { parseDictionary, validateDictionaries } from "../scripts/i18n.mjs"
import { importModule, registryPlugin } from "./import-module.mjs"

const relation = await importModule('export * from "../src/postgres-relations.ts"', { plugins: [registryPlugin([])] })
const view = await importModule(
  'export { relationChartableColumn, relationColumns, relationDataRows, relationDetailColumns, relationHistoryFilters, relationHistoryRequestFields, relationMetricHistory } from "../src/postgres-relations-view.tsx"; export { display } from "../src/postgres-view.tsx"',
  { plugins: [registryPlugin([])] },
)

test("equivalent relation rate fields keep Inspector chart inputs stable", async () => {
  const source = await readFile(new URL("../src/postgres-relations-view.tsx", import.meta.url), "utf8")
  assert.match(source, /const rateFieldsKey = \(data\.rateColumns\[section\] \?\? NO_RATE_FIELDS\)\.join\("\\u0000"\)/)
  assert.match(source, /const rateFields = useMemo\(\(\) => rateFieldsKey === "" \? NO_RATE_FIELDS : rateFieldsKey\.split\("\\u0000"\), \[rateFieldsKey\]\)/)
})

test("relation chart controls require exact physical operands and a numeric semantic", () => {
  const physical = ["seq_scan", "seq_tup_read", "idx_tup_fetch", "main_fork_bytes", "last_seq_scan", "indisvalid"]
  assert.equal(view.relationChartableColumn("pg_stat_user_tables", { field: "seq_scan", kind: "number", rate: true }, physical), true)
  assert.equal(view.relationChartableColumn("pg_stat_user_tables", { field: "main_fork_bytes", kind: "bytes" }, physical), true)
  assert.equal(view.relationChartableColumn("pg_stat_user_tables", { field: "tuple_throughput", kind: "number", rate: true }, physical), true)
  assert.equal(view.relationChartableColumn("pg_stat_user_tables", { field: "sequential_share_pct", kind: "percent" }, physical), false)
  assert.equal(view.relationChartableColumn("pg_stat_user_tables", { field: "last_seq_scan", kind: "timestamp" }, physical), false)
  assert.equal(view.relationChartableColumn("pg_stat_user_indexes", { field: "indisvalid", kind: "boolean" }, physical), false)
})

test("aggregate history charts request semantic fields once with complete hierarchy identity", () => {
  const tableCount = { field: "table_count", kind: "number", label: "pg.field.table_count.label" }
  const dml = { field: "dml_total", kind: "number", rate: true, label: "pg.field.dml_total.label" }
  const dead = { field: "dead_pct", kind: "percent", label: "pg.field.dead_pct.label" }
  const timestamp = { field: "last_vacuum_oldest", kind: "timestamp", label: "pg.field.last_vacuum_oldest.label" }
  assert.equal(view.relationChartableColumn("pg_stat_user_tables", tableCount, [], "schema"), true)
  assert.equal(view.relationChartableColumn("pg_stat_user_tables", timestamp, [], "schema"), false)
  assert.deepEqual(view.relationHistoryRequestFields("pg_stat_user_tables", "schema", [tableCount, dml, dead], []), ["table_count", "dml_total", "dead_pct"])
  assert.deepEqual(view.relationHistoryRequestFields(
    "pg_stat_user_tables", "object", [dml, dead],
    ["n_tup_ins", "n_tup_upd", "n_tup_del", "n_live_tup", "n_dead_tup"],
  ), ["n_tup_ins", "n_tup_upd", "n_tup_del", "n_live_tup", "n_dead_tup"])

  const database = parseRow(relationRecord(
    "pg_stat_user_tables", "database", { datid: "42", datname: "app" }, { dml_total: 7 },
  ), layout(layoutRecord("pg_stat_user_tables", "database", [column("dml_total", "number", "per_second")])))
  const schema = parseRow(relationRecord(
    "pg_stat_user_tables", "schema", { datid: "42", datname: "app", schemaname: "public" }, { dml_total: 7 },
  ), layout(layoutRecord("pg_stat_user_tables", "schema", [column("dml_total", "number", "per_second")])))
  assert.deepEqual(view.relationHistoryFilters(database), { datid: "42" })
  assert.deepEqual(view.relationHistoryFilters(schema), { datid: "42", schemaname: "public" })
  const tablespace = parseRow(relationRecord(
    "pg_stat_user_tables", "tablespace", { tablespace_oid: "16384" }, { tablespace: "fast_ssd", table_count: 7, dml_total: 11 },
  ), layout(layoutRecord("pg_stat_user_tables", "tablespace", [column("tablespace", "text", "none"), column("table_count"), column("dml_total", "number", "per_second")])))
  assert.deepEqual(view.relationHistoryFilters(tablespace), { tablespace_oid: "16384" })

  const later = { ...schema, segmentId: "segment-b", timestamp: 3_000_000, values: { ...schema.values, dml_total: 9 } }
  assert.deepEqual(view.relationMetricHistory([later, schema], dml, "schema"), [
    { segmentId: "segment-a", timestamp: 2_000_000, value: 7 },
    { segmentId: "segment-b", timestamp: 3_000_000, value: 9 },
  ])
})

const column = (name, kind = "number", unit = "count", nullable = true) => ({ name, kind, unit, nullable })
const layoutRecord = (logical_name, group, columns) => ({ record: "relation_layout", logical_name, group, columns })

function layout(record) {
  const parsed = relation.parseRelationLayout(record)
  assert.notEqual(parsed, null)
  return parsed
}

function relationRecord(logical_name, group, key, values, source = null) {
  return {
    record: "relation",
    logical_name,
    group,
    key,
    values,
    sample_from: "1000000",
    sample_to: "2000000",
    source: source === null ? null : { segment_id: "1709164800000000", ...source },
  }
}

function parseRow(record, storedLayout, segmentId = "segment-a") {
  const layouts = new Map([[relation.relationLayoutKey(storedLayout), storedLayout]])
  const parsed = relation.parseRelationRow(record, layouts, segmentId)
  assert.notEqual(parsed, null)
  return parsed
}

test("relation requests keep hierarchy separate from fixed metric lenses", () => {
  assert.deepEqual(relation.TABLE_LENSES, ["access", "changes", "maintenance", "size_buffers", "freeze"])
  assert.deepEqual(relation.INDEX_LENSES, ["usage", "low_activity", "size_buffers", "state"])

  const access = relation.relationRequest("pg_stat_user_tables", "access", "schema")
  assert.equal(access.group, "schema")
  assert.equal(access.pageSize, 200)
  assert.deepEqual(access.defaultOrder, ["derived.tuple_throughput"])
  assert.deepEqual(access.order.tuple_throughput, ["derived.tuple_throughput"])
  assert.deepEqual(access.order.sequential_share_pct, ["derived.sequential_share_pct"])
  assert.equal(access.fields.includes("table_count"), true)
  assert.equal(access.fields.includes("last_seq_scan_oldest"), true)
  assert.equal(access.fields.includes("last_seq_scan_latest"), true)
  assert.equal(access.fields.includes("last_seq_scan_never_count"), true)

  const changes = relation.relationRequest("pg_stat_user_tables", "changes", "object")
  assert.deepEqual(changes.defaultOrder, ["derived.dml_total"])
  for (const field of ["insert_share_pct", "update_share_pct", "delete_share_pct", "dead_pct", "hot_pct", "new_page_pct"]) {
    assert.deepEqual(changes.order[field], [`derived.${field}`])
  }

  const size = relation.relationRequest("pg_stat_user_tables", "size_buffers", "object")
  assert.deepEqual(size.defaultOrder, ["derived.displayed_storage_bytes"])
  for (const field of ["toast_share_pct", "toast_dead_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct", "buffer_hit_pct"]) {
    assert.deepEqual(size.order[field], [`derived.${field}`])
  }

  const low = relation.relationRequest("pg_stat_user_indexes", "low_activity", "object")
  assert.deepEqual(low.defaultOrder, ["main_fork_bytes"])
  assert.equal(low.fields.includes("no_scans"), true)
  assert.deepEqual(low.filters, { no_scans: "true" })

  const state = relation.relationRequest("pg_stat_user_indexes", "state", "database")
  assert.deepEqual(state.defaultOrder, ["derived.state_severity"])
  assert.equal(state.fields.includes("invalid_count"), true)
  assert.equal(state.fields.includes("unready_count"), true)
  assert.equal(state.fields.includes("indexdef"), false)

  const tablespace = relation.relationRequest("pg_stat_user_tables", "size_buffers", "tablespace")
  assert.deepEqual(tablespace.fields.slice(0, 3), ["tablespace", "tablespace_oid", "table_count"])
  assert.equal(tablespace.fields.includes("datid"), false)
})

test("relation histories default to each selected lens's meaningful quantitative metric", () => {
  assert.equal(relation.relationHistoryField("pg_stat_user_tables", "access"), "tuple_throughput")
  assert.equal(relation.relationHistoryField("pg_stat_user_tables", "changes"), "dml_total")
  assert.equal(relation.relationHistoryField("pg_stat_user_tables", "size_buffers"), "displayed_storage_bytes")
  assert.equal(relation.relationHistoryField("pg_stat_user_indexes", "state"), "state_severity")
  assert.deepEqual(relation.relationHistoryFields("pg_stat_user_tables", "tuple_throughput", ["seq_tup_read", "idx_tup_fetch"]), ["seq_tup_read", "idx_tup_fetch"])
  assert.deepEqual(relation.relationHistoryFields("pg_stat_user_tables", "tuple_throughput", ["seq_tup_read"]), [])
})

test("rendered relation columns hide numeric identity while requests retain it", () => {
  const hidden = ["datid", "relid", "indexrelid", "tablespace_oid"]
  for (const section of relation.RELATION_SECTIONS) {
    const lenses = section === "pg_stat_user_tables" ? relation.TABLE_LENSES : relation.INDEX_LENSES
    for (const lens of lenses) {
      for (const group of relation.RELATION_GROUPS) {
        const request = relation.relationRequest(section, lens, group)
        assert.equal(request.fields.includes("datid"), group !== "tablespace", `${section}:${lens}:${group}:datid projection`)
        assert.equal(request.fields.includes("tablespace_oid"), group === "tablespace", `${section}:${lens}:${group}:tablespace projection`)
        assert.equal(request.fields.includes("relid"), group === "object", `${section}:${lens}:${group}:relid projection`)
        assert.equal(request.fields.includes("indexrelid"), section === "pg_stat_user_indexes" && group === "object", `${section}:${lens}:${group}:indexrelid projection`)
        assert.deepEqual(Object.keys(request.order).filter((field) => hidden.includes(field)), [], `${section}:${lens}:${group}:visible sorts`)
        const fields = view.relationColumns(section, lens, group).map(({ field }) => field)
        assert.deepEqual(fields.filter((field) => hidden.includes(field)), [], `${section}:${lens}:${group}`)
        assert.equal(fields.includes("datname"), group !== "tablespace", `${section}:${lens}:${group}:database name`)
        assert.equal(fields.includes("schemaname"), group !== "database" && group !== "tablespace", `${section}:${lens}:${group}:schema name`)
        assert.equal(fields.includes("tablespace"), group === "tablespace" || group === "object", `${section}:${lens}:${group}:tablespace name`)
        assert.equal(fields.includes("relname"), group === "object", `${section}:${lens}:${group}:table name`)
        assert.equal(fields.includes("indexrelname"), section === "pg_stat_user_indexes" && group === "object", `${section}:${lens}:${group}:index name`)

        const detail = view.relationDetailColumns(section, lens, group).map(({ field }) => field)
        assert.deepEqual(detail.filter((field) => hidden.includes(field)), group === "tablespace" ? ["tablespace_oid"] : [], `${section}:${lens}:${group}:detail ids`)
        assert.deepEqual(detail.filter((field) => ["datname", "schemaname", "relname", "indexrelname"].includes(field)), [], `${section}:${lens}:${group}:detail identity`)
      }
    }
  }
})

test("wire rows preserve explicit aggregate identity and never invent a physical locator", () => {
  const databaseLayout = layout(layoutRecord("pg_stat_user_tables", "database", [
    column("table_count", "number", "count", false),
    column("main_fork_bytes", "number", "bytes", false),
  ]))
  const row = parseRow(relationRecord(
    "pg_stat_user_tables",
    "database",
    { datid: "42", datname: "app" },
    { table_count: 12, main_fork_bytes: 4096 },
  ), databaseLayout)
  assert.deepEqual(row.relation, { group: "database" })
  assert.deepEqual(row.values, { datid: "42", datname: "app", table_count: 12, main_fork_bytes: 4096 })
  assert.equal(row.typeId, "")
  assert.equal(row.ordinal, "")
  assert.equal(row.timestamp, 2_000_000)
})

test("the wire unit matrix is the only source of relation rates", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_indexes", "database", [
    column("index_count", "number", "count", false),
    column("invalid_count", "number", "count", false),
    column("unready_count", "number", "count", false),
    column("unique_count", "number", "count", false),
    column("primary_count", "number", "count", false),
    column("idx_scan", "number", "per_second", true),
    column("tuples_per_scan", "number", "none", true),
    column("main_fork_bytes", "number", "bytes", true),
    column("buffer_hit_pct", "number", "percent", true),
  ]))
  assert.deepEqual(relation.relationRateFields(storedLayout), ["idx_scan"])
  const columns = view.relationColumns("pg_stat_user_indexes", "state", "database", relation.relationRateFields(storedLayout))
  for (const field of ["index_count", "invalid_count", "unready_count", "unique_count", "primary_count"]) {
    assert.equal(columns.find((column) => column.field === field)?.rate, false, field)
  }

  const usage = view.relationColumns("pg_stat_user_indexes", "usage", "database", relation.relationRateFields(storedLayout))
  assert.equal(usage.find((column) => column.field === "idx_scan")?.rate, true)
  assert.equal(usage.find((column) => column.field === "tuples_per_scan")?.rate, false)
})

test("only relation row gauges receive the estimated-row presentation", () => {
  const fields = ["reltuples", "n_live_tup", "n_dead_tup", "toast_n_live_tup", "toast_n_dead_tup", "n_mod_since_analyze", "n_ins_since_vacuum"]
  for (const field of fields) assert.equal(relation.relationFieldKind(field), "estimated_rows", field)
  for (const field of ["table_count", "seq_scan", "seq_tuples_per_scan", "xid_age", "datid"]) {
    assert.notEqual(relation.relationFieldKind(field), "estimated_rows", field)
  }
  const t = (key, slots = {}) => key.endsWith(".one") ? `≈${slots.value} row` : `≈${slots.value} rows`
  const rendered = view.display("9007199254740993", { field: "reltuples", kind: "estimated_rows", label: "reltuples" }, "en", t)
  const output = rendered.type(rendered.props)
  assert.equal(output.props.children, "≈9.01E15 rows")
  assert.equal(output.props.title, "≈9,007,199,254,740,993 rows")
  assert.equal(output.props["aria-label"], "≈9,007,199,254,740,993 rows")
  assert.equal(view.display("9007199254740993", { field: "datid", kind: "id", label: "datid" }, "en", t), "9007199254740993")
  assert.equal(view.display(713456, { field: "seq_scan", kind: "number", label: "seq_scan", rate: true }, "en", () => "/s"), "713K/s")
})

test("same schema names in different databases remain distinct", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "schema", [column("table_count")]))
  const first = parseRow(relationRecord(
    "pg_stat_user_tables", "schema",
    { datid: "11", datname: "one", schemaname: "public" },
    { table_count: 2 },
  ), storedLayout)
  const second = parseRow(relationRecord(
    "pg_stat_user_tables", "schema",
    { datid: "12", datname: "two", schemaname: "public" },
    { table_count: 3 },
  ), storedLayout)
  assert.equal(relation.relationRowKey(first), JSON.stringify(["pg_stat_user_tables", "schema", "11", "one", "public"]))
  assert.equal(relation.relationRowKey(second), JSON.stringify(["pg_stat_user_tables", "schema", "12", "two", "public"]))
  assert.deepEqual(relation.relationDrill(first), {
    section: "pg_stat_user_tables",
    group: "object",
    filters: { datid: "11", schemaname: "public" },
    selectedKey: null,
  })
})

test("tablespace identity is OID-only and drills to exact matching objects", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "tablespace", [
    column("tablespace", "text", "none"), column("table_count", "number", "count", false),
  ]))
  const row = parseRow(relationRecord(
    "pg_stat_user_tables", "tablespace", { tablespace_oid: "16384" }, { tablespace: null, table_count: 4 },
  ), storedLayout)
  assert.equal(relation.relationRowKey(row), JSON.stringify(["pg_stat_user_tables", "tablespace", "16384"]))
  assert.deepEqual(relation.relationDrill(row), {
    section: "pg_stat_user_tables",
    group: "object",
    filters: { tablespace_oid: "16384" },
    selectedKey: null,
  })
  const columns = view.relationColumns("pg_stat_user_tables", "access", "tablespace")
  assert.deepEqual(columns.slice(0, 2).map(({ field }) => field), ["tablespace", "table_count"])
  assert.equal(columns[0].renderNull(row), "OID 16384")
  assert.deepEqual(view.relationDetailColumns("pg_stat_user_tables", "access", "tablespace").slice(0, 3).map(({ field }) => field), ["tablespace", "tablespace_oid", "table_count"])
})

test("same-named tables and indexes in different databases keep their complete wire identity", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "object", []))
  const first = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "11", datname: "one", schemaname: "public", relid: "42", relname: "orders" }, {},
    { type_id: "1013005", ordinal: "1", timestamp: "2000000" },
  ), storedLayout)
  const second = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "12", datname: "two", schemaname: "public", relid: "42", relname: "orders" }, {},
    { type_id: "1013005", ordinal: "2", timestamp: "2000000" },
  ), storedLayout)
  assert.equal(relation.relationRowKey(first), JSON.stringify(["pg_stat_user_tables", "object", "11", "one", "public", "42", "orders"]))
  assert.notEqual(relation.relationRowKey(first), relation.relationRowKey(second))

  const indexLayout = layout(layoutRecord("pg_stat_user_indexes", "object", []))
  const indexes = [first, second].map((table, ordinal) => parseRow(relationRecord(
    "pg_stat_user_indexes", "object",
    { ...table.values, indexrelid: "43", indexrelname: "orders_pkey" }, {},
    { type_id: "1014003", ordinal: String(ordinal), timestamp: "2000000" },
  ), indexLayout))
  assert.equal(relation.relationRowKey(indexes[0]), JSON.stringify(["pg_stat_user_indexes", "object", "11", "one", "public", "42", "orders", "43", "orders_pkey"]))
  assert.notEqual(relation.relationRowKey(indexes[0]), relation.relationRowKey(indexes[1]))
})

test("object source locators are exact and index definitions stay lazy and object-only", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_indexes", "object", [
    column("idx_scan", "number", "per_second"),
  ]))
  const row = parseRow(relationRecord(
    "pg_stat_user_indexes",
    "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    { idx_scan: 0 },
    { type_id: "1014004", ordinal: "17", timestamp: "2000000" },
  ), storedLayout)
  const target = relation.relationDetailTarget(row)
  assert.equal(target.at, 2_000_000)
  assert.equal(target.request.typeId, "1014004")
  assert.deepEqual(target.request.fields, ["indexdef"])
  assert.deepEqual(target.options, { typeId: "1014004", rowOrdinal: "17", fullText: true })

  assert.throws(() => relation.parseRelationLayout(layoutRecord(
    "pg_stat_user_indexes", "schema", [column("indexdef", "text", "none")],
  )), /aggregate index definition/)
  assert.throws(() => parseRow({ ...relationRecord(
    "pg_stat_user_indexes", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    { idx_scan: 0 },
  ) }, storedLayout), /relation source/)
})

test("table and index navigation uses exact database-scoped table identity", () => {
  const tableLayout = layout(layoutRecord("pg_stat_user_tables", "object", []))
  const table = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "42", datname: "app", schemaname: "sales", relid: "9001", relname: "orders" },
    {},
    { type_id: "1013008", ordinal: "9", timestamp: "2000000" },
  ), tableLayout)
  assert.deepEqual(relation.linkedRelation(table), {
    section: "pg_stat_user_indexes",
    group: "object",
    filters: { datid: "42", relid: "9001" },
    selectedKey: null,
  })

  const indexLayout = layout(layoutRecord("pg_stat_user_indexes", "object", []))
  const index = parseRow(relationRecord(
    "pg_stat_user_indexes", "object",
    { datid: "42", datname: "app", schemaname: "sales", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    {},
    { type_id: "1014004", ordinal: "10", timestamp: "2000000" },
  ), indexLayout)
  const back = relation.linkedRelation(index)
  assert.deepEqual(back?.filters, { datid: "42", relid: "9001" })
  assert.equal(back?.selectedKey, relation.relationRowKey(table))
  const otherDatabase = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "43", datname: "other", schemaname: "sales", relid: "9001", relname: "orders" }, {},
    { type_id: "1013008", ordinal: "11", timestamp: "2000000" },
  ), tableLayout)
  assert.notEqual(back?.selectedKey, relation.relationRowKey(otherDatabase))
})

test("detail requests only the exact index definition", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_indexes", "object", []))
  const row = parseRow(relationRecord(
    "pg_stat_user_indexes", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders", indexrelid: "9002", indexrelname: "orders_pkey" },
    {}, { type_id: "1014004", ordinal: "17", timestamp: "2000000" },
  ), storedLayout)
  assert.equal(row.segmentId, "1709164800000000")
  assert.deepEqual(relation.relationDetailTarget(row).request.fields, ["indexdef"])

  const tableLayout = layout(layoutRecord("pg_stat_user_tables", "object", []))
  const table = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders" },
    {}, { type_id: "1013006", ordinal: "18", timestamp: "2000000" },
  ), tableLayout)
  assert.throws(() => relation.relationDetailTarget(table), /index definition source/)
})

test("table detail lenses render five distinct semantic field matrices", () => {
  const expectedMetrics = {
    access: ["tuple_throughput", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", "last_seq_scan", "last_idx_scan"],
    changes: ["dml_total", "insert_share_pct", "update_share_pct", "delete_share_pct", "hot_pct", "new_page_pct", "dead_pct", "n_mod_since_analyze", "n_ins_since_vacuum"],
    maintenance: ["vacuum_count", "autovacuum_count", "analyze_count", "autoanalyze_count", "last_vacuum", "last_autovacuum", "last_analyze", "last_autoanalyze", "toast_last_autovacuum", "vacuum_mean_ms", "autovacuum_mean_ms", "analyze_mean_ms", "autoanalyze_mean_ms"],
    size_buffers: ["displayed_storage_bytes", "main_fork_bytes", "toast_bytes", "toast_share_pct", "reltuples", "toast_n_live_tup", "toast_n_dead_tup", "toast_dead_pct", "buffer_hit_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct", "heap_blks_read", "heap_blks_hit", "idx_blks_read", "idx_blks_hit", "toast_blks_read", "toast_blks_hit", "tidx_blks_read", "tidx_blks_hit"],
    freeze: ["xid_age", "mxid_age", "n_ins_since_vacuum", "last_vacuum", "last_autovacuum"],
  }
  const matrices = relation.TABLE_LENSES.map((lens) => {
    const fields = view.relationDetailColumns("pg_stat_user_tables", lens, "object", ["seq_scan", "heap_blks_read"]).map(({ field }) => field)
    assert.deepEqual(fields.slice(-expectedMetrics[lens].length), expectedMetrics[lens], lens)
    assert.equal(fields.includes("datid"), false, lens)
    assert.equal(fields.includes("relid"), false, lens)
    return fields.join(",")
  })
  assert.equal(new Set(matrices).size, relation.TABLE_LENSES.length)

  const size = view.relationDetailColumns("pg_stat_user_tables", "size_buffers", "object", ["heap_blks_read"])
  assert.equal(size.find(({ field }) => field === "heap_blks_read")?.rate, true)
  assert.equal(size.find(({ field }) => field === "heap_blks_hit")?.rate, false)

  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "object", [
    column("heap_blks_read", "number", "per_second"),
    column("heap_blks_hit", "number", "per_second"),
  ]))
  const row = parseRow(relationRecord(
    "pg_stat_user_tables", "object",
    { datid: "42", datname: "app", schemaname: "public", relid: "9001", relname: "orders" },
    { heap_blks_read: null, heap_blks_hit: 0 },
    { type_id: "1013006", ordinal: "18", timestamp: "2000000" },
  ), storedLayout)
  assert.equal(row.values.heap_blks_read, null)
  assert.equal(row.values.heap_blks_hit, 0)
})

test("index detail lenses render four distinct semantic field matrices", () => {
  const matrices = relation.INDEX_LENSES.map((lens) => view.relationDetailColumns("pg_stat_user_indexes", lens, "object").map(({ field }) => field))
  assert.equal(new Set(matrices.map((fields) => fields.join(","))).size, relation.INDEX_LENSES.length)
  for (const fields of matrices.slice(0, 3)) {
    assert.equal(fields.includes("tablespace"), false)
    assert.equal(fields.includes("amname"), false)
  }
  assert.deepEqual(matrices[3], ["tablespace", "amname", "indisvalid", "indisready", "indisunique", "indisprimary", "indisexclusion"])
  assert.equal(matrices.flat().includes("indexdef"), false)
})

test("object history is reset-safe, layout-safe, and keeps missing values unavailable", () => {
  const row = (timestamp, typeId, value) => ({
    segmentId: "segment-a",
    logicalName: "pg_stat_user_indexes",
    typeId,
    ordinal: String(timestamp),
    timestamp,
    values: { idx_scan: value, main_fork_bytes: value },
  })
  const rows = [
    row(1_000_000, "1014003", 10),
    row(3_000_000, "1014003", 14),
    row(5_000_000, "1014003", 3),
    row(7_000_000, "1014004", 7),
    row(9_000_000, "1014004", null),
    row(11_000_000, "1014004", 9),
  ]
  assert.deepEqual(relation.relationHistory(rows, "idx_scan").map(({ value }) => value), [null, 2, null, null, null, null])
  assert.deepEqual(relation.relationHistory(rows.slice(0, 3), "main_fork_bytes").map(({ value }) => value), [10, 14, 3])
  assert.deepEqual(relation.relationHistory([{ ...rows[0], logicalName: "pg_stat_user_tables", values: { reltuples: -1 } }], "reltuples").map(({ value }) => value), [null])
})

test("object DBA histories recompute exact rates and ratios without crossing identity or resets", () => {
  const table = (timestamp, values, identity = { datid: "42", relid: "9001" }) => ({
    segmentId: "segment-a", logicalName: "pg_stat_user_tables", typeId: "1013008",
    ordinal: String(timestamp), timestamp, values: { ...identity, ...values },
  })
  const rows = [
    table(1_000_000, { seq_scan: 10, idx_scan: 30, seq_tup_read: 100, idx_tup_fetch: 200, n_tup_ins: 4, n_tup_upd: 6, n_tup_del: 0, n_live_tup: 90, n_dead_tup: 10, main_fork_bytes: 800, toast_bytes: null }),
    table(3_000_000, { seq_scan: 14, idx_scan: 36, seq_tup_read: 140, idx_tup_fetch: 260, n_tup_ins: 8, n_tup_upd: 8, n_tup_del: 2, n_live_tup: 80, n_dead_tup: 20, main_fork_bytes: 900, toast_bytes: 100 }),
    table(5_000_000, { seq_scan: 1, idx_scan: 2, seq_tup_read: 2, idx_tup_fetch: 3, n_tup_ins: 1, n_tup_upd: 1, n_tup_del: 0, n_live_tup: 0, n_dead_tup: 0, main_fork_bytes: 1_000, toast_bytes: null }),
    table(7_000_000, { seq_scan: 3, idx_scan: 4, seq_tup_read: 8, idx_tup_fetch: 9, n_tup_ins: 3, n_tup_upd: 2, n_tup_del: 1, n_live_tup: 50, n_dead_tup: 50, main_fork_bytes: 2_000, toast_bytes: null }, { datid: "42", relid: "9002" }),
  ]
  const values = (field) => relation.relationHistory(rows, field).map((point) => point.value)
  assert.deepEqual(values("tuple_throughput"), [null, 50, null, null])
  assert.deepEqual(values("sequential_share_pct"), [null, 40, null, null])
  assert.deepEqual(values("seq_tuples_per_scan"), [null, 10, null, null])
  assert.deepEqual(values("dml_total"), [null, 4, null, null])
  assert.deepEqual(values("insert_share_pct"), [null, 50, null, null])
  assert.deepEqual(values("dead_pct"), [10, 20, null, 50])
  assert.deepEqual(values("displayed_storage_bytes"), [800, 1_000, 1_000, 2_000])

  const index = rows.slice(0, 2).map((row, index) => ({
    ...row, logicalName: "pg_stat_user_indexes", typeId: "1014004", values: {
      datid: "42", indexrelid: "9003", idx_scan: index === 0 ? 10 : 12,
      idx_tup_read: index === 0 ? 20 : 28, idx_tup_fetch: index === 0 ? 6 : 10,
      idx_blks_read: index === 0 ? 2 : 4, idx_blks_hit: index === 0 ? 8 : 14,
    },
  }))
  assert.deepEqual(relation.relationHistory(index, "tuples_per_scan").map(({ value }) => value), [null, 4])
  assert.deepEqual(relation.relationHistory(index, "fetches_per_scan").map(({ value }) => value), [null, 2])
  assert.deepEqual(relation.relationHistory(index, "buffer_hit_pct").map(({ value }) => value), [null, 75])
})

test("wire validation rejects mismatched keys, values, intervals, and fake aggregate sources", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "database", [column("table_count", "number", "count", false)]))
  const valid = relationRecord("pg_stat_user_tables", "database", { datid: "42", datname: "app" }, { table_count: 2 })
  assert.throws(() => parseRow({ ...valid, key: { datid: "42", datname: "app", schemaname: "public" } }, storedLayout), /relation key/)
  assert.throws(() => parseRow({ ...valid, values: {} }, storedLayout), /relation values/)
  assert.throws(() => parseRow({ ...valid, values: { table_count: null } }, storedLayout), /relation null/)
  assert.throws(() => parseRow({ ...valid, sample_from: "3", sample_to: "2" }, storedLayout), /sample interval/)
  assert.throws(() => parseRow({ ...valid, source: { type_id: "1013005", ordinal: "1", timestamp: "2" } }, storedLayout), /relation source/)
})

test("the relation table exposes complete server-sortable quantitative lenses", () => {
  const changes = view.relationColumns("pg_stat_user_tables", "changes", "object")
  for (const field of ["dml_total", "insert_share_pct", "update_share_pct", "delete_share_pct", "dead_pct", "hot_pct", "new_page_pct"]) {
    assert.equal(changes.find((column) => column.field === field)?.sortable, true, field)
  }
  assert.equal(changes.some(({ field }) => field.startsWith("n_tup_")), false)
  assert.equal(changes.find((column) => column.field === "relname")?.sticky, true)
  assert.notEqual(changes.find((column) => column.field === "relname")?.sortable, true)

  const state = view.relationColumns("pg_stat_user_indexes", "state", "schema")
  for (const field of ["invalid_count", "unready_count", "unique_count", "primary_count", "exclusion_count"]) {
    assert.equal(state.find((column) => column.field === field)?.sortable, true, field)
  }
  assert.equal(state.some((column) => column.field === "state_severity"), false)
  assert.equal(state.some((column) => column.field === "indexdef"), false)
})

test("all table and index levels and lenses keep exact meaning-first display orders", () => {
  const table = {
    access: {
      object: ["tuple_throughput", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", "tablespace", "last_seq_scan", "last_idx_scan"],
      aggregate: ["tuple_throughput", "table_count", "sequential_share_pct", "seq_scan", "idx_scan", "seq_tuples_per_scan", "idx_tuples_per_scan", "last_seq_scan_never_count", "last_idx_scan_never_count", "last_seq_scan_oldest", "last_seq_scan_latest", "last_idx_scan_oldest", "last_idx_scan_latest"],
    },
    changes: {
      object: ["dml_total", "insert_share_pct", "update_share_pct", "delete_share_pct", "hot_pct", "new_page_pct", "dead_pct", "n_mod_since_analyze", "n_ins_since_vacuum", "tablespace"],
      aggregate: ["dml_total", "table_count", "insert_share_pct", "update_share_pct", "delete_share_pct", "hot_pct", "new_page_pct", "dead_pct", "n_mod_since_analyze", "n_ins_since_vacuum"],
    },
    maintenance: {
      object: ["autovacuum_count", "vacuum_count", "autoanalyze_count", "analyze_count", "autovacuum_mean_ms", "vacuum_mean_ms", "autoanalyze_mean_ms", "analyze_mean_ms", "tablespace", "last_autovacuum", "last_vacuum", "last_autoanalyze", "last_analyze", "toast_last_autovacuum"],
      aggregate: ["autovacuum_count", "table_count", "vacuum_count", "autoanalyze_count", "analyze_count", "autovacuum_mean_ms", "vacuum_mean_ms", "autoanalyze_mean_ms", "analyze_mean_ms", "last_autovacuum_never_count", "last_vacuum_never_count", "last_autoanalyze_never_count", "last_analyze_never_count", "toast_last_autovacuum_never_count", "last_autovacuum_oldest", "last_autovacuum_latest", "last_vacuum_oldest", "last_vacuum_latest", "last_autoanalyze_oldest", "last_autoanalyze_latest", "last_analyze_oldest", "last_analyze_latest", "toast_last_autovacuum_oldest", "toast_last_autovacuum_latest"],
    },
    size_buffers: {
      object: ["displayed_storage_bytes", "buffer_hit_pct", "main_fork_bytes", "toast_share_pct", "reltuples", "toast_dead_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct", "tablespace"],
      aggregate: ["displayed_storage_bytes", "table_count", "buffer_hit_pct", "main_fork_bytes", "toast_share_pct", "reltuples", "toast_dead_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct"],
    },
    freeze: {
      object: ["xid_age", "mxid_age", "n_ins_since_vacuum", "tablespace", "last_autovacuum", "last_vacuum"],
      aggregate: ["xid_age", "table_count", "mxid_age", "n_ins_since_vacuum", "last_autovacuum_never_count", "last_vacuum_never_count", "last_autovacuum_oldest", "last_autovacuum_latest", "last_vacuum_oldest", "last_vacuum_latest"],
    },
  }
  const index = {
    usage: {
      object: ["idx_scan", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", "amname", "tablespace", "last_idx_scan"],
      aggregate: ["idx_scan", "index_count", "idx_tup_read", "idx_tup_fetch", "tuples_per_scan", "fetches_per_scan", "last_idx_scan_never_count", "last_idx_scan_oldest", "last_idx_scan_latest"],
    },
    low_activity: {
      object: ["main_fork_bytes", "idx_scan", "amname", "tablespace", "no_scans", "last_idx_scan"],
      aggregate: ["main_fork_bytes", "index_count", "no_scan_count", "known_scan_count", "idx_scan", "last_idx_scan_never_count", "last_idx_scan_oldest", "last_idx_scan_latest"],
    },
    size_buffers: {
      object: ["main_fork_bytes", "buffer_hit_pct", "amname", "tablespace"],
      aggregate: ["main_fork_bytes", "index_count", "buffer_hit_pct"],
    },
    state: {
      object: ["amname", "tablespace", "indisvalid", "indisready", "indisprimary", "indisunique", "indisexclusion"],
      aggregate: ["invalid_count", "index_count", "unready_count", "primary_count", "unique_count", "exclusion_count"],
    },
  }
  const assertOrders = (section, lenses, objectPrefix, count) => {
    for (const [lens, suffixes] of Object.entries(lenses)) {
      assert.deepEqual(view.relationColumns(section, lens, "object").map(({ field }) => field), [...objectPrefix, ...suffixes.object], `${section}/${lens}/object`)
      assert.deepEqual(view.relationColumns(section, lens, "schema").map(({ field }) => field), ["schemaname", "datname", ...suffixes.aggregate], `${section}/${lens}/schema`)
      assert.deepEqual(view.relationColumns(section, lens, "database").map(({ field }) => field), ["datname", ...suffixes.aggregate], `${section}/${lens}/database`)
      assert.deepEqual(view.relationColumns(section, lens, "tablespace").map(({ field }) => field), ["tablespace", count, ...suffixes.aggregate.filter((field) => field !== count)], `${section}/${lens}/tablespace`)
      assert.equal(view.relationColumns(section, lens, "object").some(({ field }) => ["datid", "relid", "indexrelid", "tablespace_oid"].includes(field)), false)
      assert.equal(view.relationColumns(section, lens, "tablespace").some(({ field }) => field === "tablespace_oid"), false)
      assert.ok(view.relationColumns(section, lens, "database").some(({ field }) => field === count))
    }
  }
  assertOrders("pg_stat_user_tables", table, ["relname", "datname", "schemaname"], "table_count")
  assertOrders("pg_stat_user_indexes", index, ["indexrelname", "datname", "schemaname", "relname"], "index_count")
})

test("meaning-first relation columns keep raw buffer operands in detail", () => {
  const table = view.relationColumns("pg_stat_user_tables", "size_buffers", "object")
  assert.deepEqual(table.map(({ field }) => field), [
    "relname", "datname", "schemaname", "displayed_storage_bytes", "buffer_hit_pct", "main_fork_bytes", "toast_share_pct",
    "reltuples", "toast_dead_pct", "heap_buffer_hit_pct", "index_buffer_hit_pct", "toast_buffer_hit_pct", "tidx_buffer_hit_pct", "tablespace",
  ])
  assert.equal(table.some(({ field }) => field.endsWith("_blks_read") || field.endsWith("_blks_hit")), false)

  const detail = view.relationDetailColumns("pg_stat_user_tables", "size_buffers", "object")
  for (const field of ["toast_bytes", "toast_n_live_tup", "toast_n_dead_tup", "toast_dead_pct", "heap_blks_read", "tidx_blks_hit"]) {
    assert.equal(detail.some((column) => column.field === field), true, field)
  }
  assert.equal(table.find(({ field }) => field === "main_fork_bytes")?.label, "pg.field.table_data_bytes.label")
  assert.equal(view.relationColumns("pg_stat_user_indexes", "size_buffers", "object").find(({ field }) => field === "main_fork_bytes")?.label, "pg.field.main_fork_bytes.label")
})

test("relation copy covers every projected label and help key", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  const usedHelp = new Set()
  const obvious = new Set(["datname", "schemaname", "relname", "indexrelname", "tablespace", "amname", "table_count", "index_count"])
  for (const section of relation.RELATION_SECTIONS) {
    const lenses = section === "pg_stat_user_tables" ? relation.TABLE_LENSES : relation.INDEX_LENSES
    for (const lens of lenses) for (const group of relation.RELATION_GROUPS) {
      const columns = [...view.relationColumns(section, lens, group), ...view.relationDetailColumns(section, lens, group)]
      for (const column of columns) {
        assert.equal(Object.hasOwn(english, column.label), true, column.label)
        if (column.help !== undefined) {
          usedHelp.add(column.help)
          assert.equal(Object.hasOwn(english, column.help), true, column.help)
          assert.equal(Object.hasOwn(russian, column.help), true, column.help)
        }
      }
      for (const column of view.relationColumns(section, lens, group)) {
        assert.equal(column.help === undefined, obvious.has(column.field), `${section}/${lens}/${group}/${column.field}`)
      }
    }
  }
  const relationHelp = Object.keys(english).filter((key) => key.startsWith("pg.help.relation.")).sort()
  assert.deepEqual([...usedHelp].sort(), relationHelp)
})

test("object scan timestamps distinguish never from unavailable", () => {
  const t = (key) => ({ "common.never": "Never", "common.unavailable": "—" })[key] ?? key
  const scan = view.relationColumns("pg_stat_user_tables", "access", "object", [], t).find(({ field }) => field === "last_seq_scan")
  assert.equal(scan.renderNull({ values: { last_seq_scan_never: true } }), "Never")
  assert.equal(scan.renderNull({ values: { last_seq_scan_never: null } }), "—")
})

test("the relation view consumes only rows with the authoritative discriminator", () => {
  const storedLayout = layout(layoutRecord("pg_stat_user_tables", "database", [column("table_count")]))
  const relationRow = parseRow(relationRecord(
    "pg_stat_user_tables", "database", { datid: "42", datname: "app" }, { table_count: 2 },
  ), storedLayout)
  const physical = { ...relationRow, relation: undefined }
  assert.deepEqual(view.relationDataRows([relationRow, physical], "pg_stat_user_tables", "database"), [relationRow])
  assert.deepEqual(view.relationDataRows([relationRow], "pg_stat_user_tables", "schema"), [])
})
