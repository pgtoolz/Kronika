import assert from "node:assert/strict"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const statementBase = ["ts", "queryid", "calls", "rows", "shared_blks_read", "shared_blks_dirtied", "temp_blks_written"]
const tableBase = ["ts", "relid", "n_tup_ins", "n_tup_upd", "n_tup_del", "seq_tup_read", "heap_blks_read", "n_dead_tup"]
const registry = [
  { typeId: "1002001", logicalName: "pg_stat_statements", identity: ["queryid"], columns: [...statementBase, "total_time"] },
  { typeId: "1002003", logicalName: "pg_stat_statements", identity: ["queryid"], columns: [...statementBase, "total_exec_time", "wal_bytes"] },
  { typeId: "1013005", logicalName: "pg_stat_user_tables", identity: ["relid"], columns: tableBase },
  { typeId: "1013008", logicalName: "pg_stat_user_tables", identity: ["relid"], columns: [...tableBase, "total_autovacuum_time"] },
]
const cuts = await importModule(
  'export * from "../src/activity-cuts.ts"; export { cutsForLayouts } from "../src/postgres-metrics.ts"',
  { plugins: [registryPlugin(registry)] },
)

const ids = (list) => list.map((cut) => cut.id)
const fields = (list, id) => list.find((cut) => cut.id === id)?.fields

test("an older statements layout answers the execution cut under its own column and loses the WAL cut", () => {
  const offered = cuts.cutsForLayouts(cuts.STATEMENT_CUTS, ["1002001"])
  assert.deepEqual(fields(offered, "exec_time"), ["total_time"])
  assert.deepEqual(ids(offered), ["exec_time", "calls", "rows", "shared_read", "shared_dirtied", "temp_written"])
})

test("a current statements layout keeps the declared cuts", () => {
  const offered = cuts.cutsForLayouts(cuts.STATEMENT_CUTS, ["1002003"])
  assert.deepEqual(fields(offered, "exec_time"), ["total_exec_time"])
  assert.deepEqual(ids(offered), ids(cuts.STATEMENT_CUTS))
})

test("an hour recording both layouts requests every recorded name of a renamed fact", () => {
  const offered = cuts.cutsForLayouts(cuts.STATEMENT_CUTS, ["1002001", "1002003"])
  assert.deepEqual(fields(offered, "exec_time"), ["total_exec_time", "total_time"])
  assert.ok(ids(offered).includes("wal_bytes"))
})

test("a table cut whose fact no recorded layout carries is dropped and multi-field cuts stay whole", () => {
  assert.deepEqual(ids(cuts.cutsForLayouts(cuts.TABLE_CUTS, ["1013005"])), ["writes", "seq_read", "heap_read", "dead_tuples"])
  assert.deepEqual(fields(cuts.cutsForLayouts(cuts.TABLE_CUTS, ["1013005"]), "writes"), ["n_tup_ins", "n_tup_upd", "n_tup_del"])
  assert.deepEqual(ids(cuts.cutsForLayouts(cuts.TABLE_CUTS, ["1013008"])), ids(cuts.TABLE_CUTS))
})

test("unknown or missing layouts leave the declared cuts, by identity", () => {
  assert.equal(cuts.cutsForLayouts(cuts.STATEMENT_CUTS, []), cuts.STATEMENT_CUTS)
  assert.equal(cuts.cutsForLayouts(cuts.STATEMENT_CUTS, ["9999999"]), cuts.STATEMENT_CUTS)
})

test("a layout answering no cut at all keeps the declared cuts instead of an empty ledger", () => {
  assert.equal(cuts.cutsForLayouts(cuts.STATEMENT_CUTS, ["1013005"]), cuts.STATEMENT_CUTS)
})
