import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { canonicalSearch, parseSearch, rowMatchesSearch, SEARCH_MAX_CLAUSES, SEARCH_MAX_EXPRESSION, SEARCH_MAX_GROUP_DEPTH, SEARCH_MAX_VALUE, searchFields, withoutSearchClause } = await importModule('export * from "../src/search.ts"')

const row = (logicalName, typeId, values) => ({
  logicalName, ordinal: "1", segmentId: "7", timestamp: 10, typeId, values,
})

test("structured search canonicalizes aliases, AND casing, quotes, and escapes", () => {
  const parsed = parseSearch('query_id:-912345 and db:"Sales \\"East\\""', "pg_stat_statements")
  assert.equal(parsed.ok, true)
  if (!parsed.ok) return
  assert.equal(parsed.query.canonical, 'query_id:-912345 AND database:"Sales \\"East\\""')
  assert.deepEqual(parsed.query.clauses.map(({ key, value }) => [key, value]), [
    ["query_id", "-912345"], ["database", 'Sales "East"'],
  ])
  assert.equal(withoutSearchClause(parsed.query, 0), 'database:"Sales \\"East\\""')
})

test("ordinary free text stays unambiguous and explicit text mixes with selectors", () => {
  const plain = parseSearch("select orders*", "pg_stat_statements")
  assert.equal(plain.ok && plain.query.freeText, "select orders*")
  const mixed = parseSearch('text:"select orders*" AND database:app', "pg_stat_statements")
  assert.equal(mixed.ok && mixed.query.canonical, 'text:"select orders*" AND database:app')
  assert.equal(parseSearch("select AND orders", "pg_stat_statements").ok, false)
})

test("invalid selectors identify the exact offending span and never become text", () => {
  const unknown = parseSearch("query_id:42 AND taname:foo", "pg_stat_statements")
  assert.equal(unknown.ok, false)
  if (unknown.ok) return
  assert.equal(unknown.error.code, "unknown_field")
  assert.equal("query_id:42 AND taname:foo".slice(unknown.error.start, unknown.error.end), "taname")
  for (const expression of ["query_id:*", "query_id:01", 'database:"open', 'database:"bad\\n"']) {
    assert.equal(parseSearch(expression, "pg_stat_statements").ok, false, expression)
  }
  assert.equal(parseSearch('database:""', "pg_stat_statements").ok, false)
  assert.equal(parseSearch("queryid:42", "pg_stat_statements").ok, false)
  assert.equal(parseSearch("planid:42", "pg_store_plans").ok, false)
})

test("parser limits expression, clauses, values, and exact signed bigint text", () => {
  assert.equal(parseSearch("x".repeat(SEARCH_MAX_EXPRESSION + 1), "os_process").ok, false)
  const unicodeValue = parseSearch("😀".repeat(SEARCH_MAX_VALUE + 1), "os_process")
  assert.equal(unicodeValue.ok, false)
  assert.equal(unicodeValue.error.code, "value_too_long")
  const unicodeExpression = parseSearch("😀".repeat(SEARCH_MAX_EXPRESSION + 1), "os_process")
  assert.equal(unicodeExpression.ok, false)
  assert.equal(unicodeExpression.error.code, "expression_too_long")
  assert.equal(parseSearch(`text:${"x".repeat(SEARCH_MAX_VALUE + 1)}`, "os_process").ok, false)
  assert.equal(parseSearch(Array.from({ length: SEARCH_MAX_CLAUSES + 1 }, () => "state:active").join(" AND "), "pg_stat_activity").ok, false)
  assert.equal(parseSearch("query_id:-9223372036854775808", "pg_stat_statements").ok, true)
  assert.equal(parseSearch("query_id:9223372036854775808", "pg_stat_statements").ok, false)
  assert.equal(parseSearch("pid:18446744073709551615", "os_process").ok, true)
  assert.equal(parseSearch("pid:18446744073709551616", "os_process").ok, false)
})

test("each surface exposes only its useful canonical public fields", () => {
  assert.deepEqual(searchFields("pg_stat_user_tables").map(({ key }) => key), [
    "text", "database", "schema", "table_name", "tablespace", "size", "table_count",
    "buffer_hit", "seq_scan_rate", "change_rate", "autovacuum_rate", "autovacuum_mean", "xid_age",
  ])
  const sharedPg = [
    "call_rate", "exec_time_rate", "mean_exec", "row_rate", "rows_per_call", "planning_time_rate", "planning_share",
    "shared_buffer_hit_rate", "shared_buffer_read_rate", "shared_buffer_dirty_rate", "shared_buffer_write_rate",
    "local_buffer_hit_rate", "local_buffer_read_rate", "local_buffer_dirty_rate", "local_buffer_write_rate", "temp_buffer_read_rate", "temp_buffer_write_rate",
    "shared_read_time_rate", "shared_write_time_rate", "local_read_time_rate", "local_write_time_rate", "temp_read_time_rate", "temp_write_time_rate",
    "buffer_hit", "buffer_per_call", "exec_cv", "min_exec_since_reset", "max_exec_since_reset", "mean_exec_since_reset", "stddev_exec_since_reset",
  ]
  assert.deepEqual(searchFields("pg_store_plans").map(({ key }) => key), ["text", "query_id", "plan_id", "database", "role", "calls", ...sharedPg, "slow_call_rate"])
  assert.deepEqual(searchFields("pg_stat_statements").map(({ key }) => key), ["text", "query_id", "database", "role", ...sharedPg, "plan_rate", "wal_rate", "wal_per_call"])
  assert.equal(parseSearch("plan_id:42", "pg_stat_statements").ok, false)
  assert.equal(parseSearch("table_name:orders", "pg_stat_user_tables").ok, true)
  assert.equal(searchFields("pg_store_plans").some((field) => field.key.includes("queryid_stat")), false)
  assert.deepEqual(searchFields("os_process").map(({ key }) => key), [
    "text", "user", "effective_user", "user_id", "effective_user_id", "pid", "parent_pid", "command", "state",
    "rss", "vsz", "swap", "threads", "cpu_cores", "user_cpu_cores", "system_cpu_cores",
    "disk_read_rate", "disk_write_rate", "logical_read_rate", "logical_write_rate", "read_syscall_rate", "write_syscall_rate",
    "major_fault_rate", "minor_fault_rate", "context_switch_rate", "voluntary_context_switch_rate", "involuntary_context_switch_rate", "run_delay", "block_io_delay",
  ])
  const process = parseSearch("username:postgres AND euser:postgres* AND uid:26 AND euid:27", "os_process")
  assert.equal(process.ok && process.query.canonical, "user:postgres AND effective_user:postgres* AND user_id:26 AND effective_user_id:27")
  assert.equal(parseSearch("resident_memory>2MiB", "os_process").query?.canonical, "rss>2MiB")
  assert.equal(parseSearch("virtual_memory>1GiB", "os_process").query?.canonical, "vsz>1GiB")
  assert.equal(parseSearch("majflt_rate>1/s", "os_process").query?.canonical, "major_fault_rate>1/s")
})

test("strict comparisons canonicalize exact quantities without Number conversion", () => {
  const parsed = parseSearch(" schema:public and size > 100.000MB AND seq_scan_rate<0.5/s ", "pg_stat_user_tables")
  assert.equal(parsed.ok, true)
  if (!parsed.ok) return
  assert.equal(parsed.query.canonical, "schema:public AND size>100MB AND seq_scan_rate<0.5/s")
  assert.equal(parsed.query.expr.kind, "and")
  assert.deepEqual(
    parsed.query.clauses.slice(1).map(({ operator, quantity }) => [
      operator, quantity.number, quantity.unit, quantity.numerator, quantity.denominator,
    ]),
    [[">", "100", "MB", 100_000_000n, 1n], ["<", "0.5", "/s", 1n, 2n]],
  )
  assert.equal(canonicalSearch([{ key: "size", operator: ">", value: "100.000MB" }], "pg_stat_user_indexes"), "size>100MB")
})

test("SI, IEC, duration, percentage, and count units retain exact boundaries", () => {
  const quantities = [
    ["size>100MB", 100_000_000n, 1n],
    ["size>100MiB", 104_857_600n, 1n],
    ["size>0.5KiB", 512n, 1n],
    ["autovacuum_mean<250000us", 250n, 1n],
    ["buffer_hit>99.95%", 1_999n, 20n],
    ["table_count<18446744073709551615", 18_446_744_073_709_551_615n, 1n],
  ]
  for (const [expression, numerator, denominator] of quantities) {
    const parsed = parseSearch(expression, "pg_stat_user_tables")
    assert.equal(parsed.ok, true, expression)
    if (!parsed.ok) continue
    assert.equal(parsed.query.clauses[0].quantity.numerator, numerator, expression)
    assert.equal(parsed.query.clauses[0].quantity.denominator, denominator, expression)
  }
  for (const expression of [
    "size>0.1B", "size>100", "size>100mb", "size>100 MB", 'size>"100MB"',
    "buffer_hit>100.1%", "table_count>1.5", "size>-1MB", "size>1e3MB", "size>01MB", "size>1.MB",
    "size>1,000MB", "size>1_MB", "size>NaN", "size>Infinity",
  ]) assert.equal(parseSearch(expression, "pg_stat_user_tables").ok, false, expression)
})

test("process and PostgreSQL quantities preserve scalar, rate, load, and per-call units", () => {
  for (const [surface, expression, numerator, denominator, unit] of [
    ["os_process", "cpu_cores>0.1", 1n, 10n, ""],
    ["os_process", "rss>2MiB", 2_097_152n, 1n, "MiB"],
    ["os_process", "disk_read_rate>1.5MiB/s", 1_572_864n, 1n, "MiB/s"],
    ["os_process", "run_delay<250us/s", 1n, 4n, "us/s"],
    ["pg_stat_statements", "exec_time_rate>0.5s/s", 500n, 1n, "s/s"],
    ["pg_stat_statements", "wal_per_call>0.5KiB", 512n, 1n, "KiB"],
    ["pg_store_plans", "calls>9007199254740993", 9_007_199_254_740_993n, 1n, ""],
    ["pg_store_plans", "rows_per_call<0.125", 1n, 8n, ""],
  ]) {
    const parsed = parseSearch(expression, surface)
    assert.equal(parsed.ok, true, expression)
    if (!parsed.ok) continue
    assert.deepEqual(
      [parsed.query.clauses[0].quantity.numerator, parsed.query.clauses[0].quantity.denominator, parsed.query.clauses[0].quantity.unit],
      [numerator, denominator, unit],
      expression,
    )
  }
  for (const [surface, expression] of [
    ["os_process", "cpu_cores>1core"], ["os_process", "rss>2MiB/s"], ["os_process", "disk_read_rate>1MiB"],
    ["pg_stat_statements", "exec_time_rate>1ms"], ["pg_stat_statements", "wal_per_call>1MiB/s"],
    ["pg_store_plans", "rows_per_call>1/s"], ["pg_stat_statements", "exec_time_rate>1min/s"],
  ]) assert.equal(parseSearch(expression, surface).ok, false, expression)
})

test("non-v1 comparison operators are atomic and NOT stays reserved", () => {
  for (const [expression, code, token] of [
    ["size>=100MB", "unsupported_operator", ">="],
    ["size<=100MB", "unsupported_operator", "<="],
    ["size==100MB", "unsupported_operator", "=="],
    ["size!=100MB", "unsupported_operator", "!="],
    ["size=100MB", "unsupported_operator", "="],
    ["size=>100MB", "malformed_operator", "=>"],
    ["size<>100MB", "malformed_operator", "<>"],
    ["size:100MB", "operator_not_allowed", ":"],
    ["schema>public", "operator_not_allowed", ">"],
    ["NOT size>100MB", "unsupported_syntax", "NOT"],
    ["NOT latency", "unsupported_syntax", "NOT"],
  ]) {
    const parsed = parseSearch(expression, "pg_stat_user_tables")
    assert.equal(parsed.ok, false, expression)
    if (parsed.ok) continue
    assert.equal(parsed.error.code, code, expression)
    assert.equal(expression.slice(parsed.error.start, parsed.error.end), token, expression)
  }
  assert.equal(parseSearch('text:"size>100MB OR (later)"', "pg_stat_user_tables").ok, true)
})

test("OR and parentheses have canonical precedence and stable clause removal", () => {
  const precedence = parseSearch("schema:public or schema:audit and table_name:orders", "pg_stat_user_tables")
  assert.equal(precedence.ok, true)
  if (!precedence.ok) return
  assert.equal(precedence.query.canonical, "schema:public OR schema:audit AND table_name:orders")
  assert.equal(precedence.query.expr.kind, "or")
  assert.equal(precedence.query.expr.right.kind, "and")

  const grouped = parseSearch("((schema:public OR schema:audit)) AND (size > 100.000MB OR buffer_hit<90%)", "pg_stat_user_tables")
  assert.equal(grouped.ok, true)
  if (!grouped.ok) return
  assert.equal(grouped.query.canonical, "(schema:public OR schema:audit) AND (size>100MB OR buffer_hit<90%)")
  assert.equal(withoutSearchClause(grouped.query, 0), "schema:audit AND (size>100MB OR buffer_hit<90%)")
  assert.equal(withoutSearchClause(grouped.query, 3), "(schema:public OR schema:audit) AND size>100MB")
})

test("boolean diagnostics mark empty, unbalanced, missing, doubled, and trailing syntax exactly", () => {
  for (const [expression, code, token] of [
    ["()", "empty_group", "()"],
    ["(schema:public", "unbalanced_parenthesis", "("],
    ["schema:public)", "unbalanced_parenthesis", ")"],
    ["schema:public AND", "missing_operand", "AND"],
    ["schema:public OR OR schema:audit", "missing_operand", "OR"],
    ["AND schema:public", "missing_operand", "AND"],
    ["schema:public table_name:orders", "expected_boolean_operator", "table_name:orders"],
  ]) {
    const parsed = parseSearch(expression, "pg_stat_user_tables")
    assert.equal(parsed.ok, false, expression)
    if (parsed.ok) continue
    assert.equal(parsed.error.code, code, expression)
    assert.equal(expression.slice(parsed.error.start, parsed.error.end), token, expression)
  }
})

test("boolean bounds reject excessive nesting and token counts", () => {
  const nested = `${"(".repeat(SEARCH_MAX_GROUP_DEPTH + 1)}schema:public${")".repeat(SEARCH_MAX_GROUP_DEPTH + 1)}`
  const deep = parseSearch(nested, "pg_stat_user_tables")
  assert.equal(deep.ok, false)
  assert.equal(deep.error.code, "group_too_deep")
  const clauses = Array.from({ length: SEARCH_MAX_CLAUSES }, () => "(schema:public)").join(" OR ")
  assert.equal(parseSearch(clauses, "pg_stat_user_tables").ok, true)
  const tokens = parseSearch(`(${clauses})`, "pg_stat_user_tables")
  assert.equal(tokens.ok, false)
  assert.equal(tokens.error.code, "too_many_tokens")
})

test("grouped relation OR stays wholly before or after reduction", () => {
  const grouped = { grouped: true }
  for (const expression of [
    "(schema:public OR schema:audit) AND size>100MB",
    "schema:public AND (size>100MB OR buffer_hit<90%)",
  ]) assert.equal(parseSearch(expression, "pg_stat_user_tables", grouped).ok, true, expression)

  for (const expression of [
    "schema:public OR size>100MB",
    "(schema:public AND size>100MB) OR schema:audit",
  ]) {
    const parsed = parseSearch(expression, "pg_stat_user_tables", grouped)
    assert.equal(parsed.ok, false, expression)
    if (parsed.ok) continue
    assert.equal(parsed.error.code, "mixed_phase_or", expression)
    assert.equal(expression.slice(parsed.error.start, parsed.error.end), "OR", expression)
  }
  assert.equal(parseSearch("schema:public OR size>100MB", "pg_stat_user_tables").ok, true)
})

test("client surfaces evaluate OR groups without losing fork transparency", () => {
  const statement = row("pg_stat_statements", "1002002", { datname: "app", query: "select orders", queryid: "-912345", usename: "reader" })
  const match = parseSearch("database:other OR (database:app AND query_id:-912345)", "pg_stat_statements")
  assert.equal(match.ok && rowMatchesSearch(statement, match.query, "pg_stat_statements"), true)
  const miss = parseSearch("database:other OR query_id:7", "pg_stat_statements")
  assert.equal(miss.ok && rowMatchesSearch(statement, miss.query, "pg_stat_statements"), false)
})

test("comparison fields are surface-wide and never expose reducer dependencies", () => {
  const tableFields = searchFields("pg_stat_user_tables")
  const indexFields = searchFields("pg_stat_user_indexes")
  assert.equal(tableFields.find(({ key }) => key === "size")?.columns.length, 0)
  assert.equal(indexFields.find(({ key }) => key === "size")?.columns.length, 0)
  for (const expression of ["size>100MB", "xid_age<1000", "autovacuum_mean>250ms"]) {
    assert.equal(parseSearch(expression, "pg_stat_user_tables").ok, true, expression)
  }
  for (const internal of ["displayed_storage_bytes>1B", "main_fork_bytes>1B", "buffer_hit_pct>90%"]) {
    assert.equal(parseSearch(internal, "pg_stat_user_tables").ok, false, internal)
  }
})

test("client matching is conjunctive, globbed only for strings, and fork-transparent", () => {
  const statement = row("pg_stat_statements", "1002002", { datname: "app", query: "select orders", queryid: "-912345", usename: "reader" })
  const parsed = parseSearch("database:app AND query_id:-912345", "pg_stat_statements")
  assert.equal(parsed.ok && rowMatchesSearch(statement, parsed.query, "pg_stat_statements"), true)
  const vadv = row("pg_store_plans", "1004001", { datname: "app", plan: "Index Scan", queryid: "0", queryid_stat_statements: "-912345", usename: "reader" })
  const planSearch = parseSearch("query_id:-912345", "pg_store_plans")
  assert.equal(planSearch.ok && rowMatchesSearch(vadv, planSearch.query, "pg_store_plans"), true)
  const zero = parseSearch("query_id:0", "pg_store_plans")
  assert.equal(zero.ok && rowMatchesSearch(vadv, zero.query, "pg_store_plans"), false)
  assert.equal(canonicalSearch([{ key: "database", value: "Sales Data" }, { key: "query_id", value: "-912345" }], "pg_store_plans"), 'database:"Sales Data" AND query_id:-912345')
})
