import assert from "node:assert/strict"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const iconPlugin = {
  name: "icons",
  setup(context) {
    context.onResolve({ filter: /^lucide-react$/ }, () => ({ namespace: "icons", path: "icons" }))
    context.onLoad({ filter: /.*/, namespace: "icons" }, () => ({ contents: "export const Copy=()=>null" }))
  },
}
const plans = await importModule(
  'export * from "../src/plan-text.ts"; export * from "../src/plan-view.tsx"; export * from "../src/statement-navigation.ts"',
  { plugins: [iconPlugin] },
)
const t = (key) => key

const nativePlan = [
  "",
  "  Merge Join  (cost=0.85..81.42 rows=12 width=64)",
  "    Merge Cond: (orders.customer_id = customers.id)",
  "    ->  Seq Scan on orders  (cost=0.00..20.00 rows=400 width=32)",
  "    ->  Index Scan using customers_pkey on customers  (cost=0.42..8.44 rows=1 width=32)",
].join("\n")

test("native text plans use their first non-empty line as the dense summary", () => {
  assert.equal(plans.planTextSummary(nativePlan), "Merge Join  (cost=0.85..81.42 rows=12 width=64)")
  assert.equal(plans.planTextSummary(`  ${"x".repeat(100)}`), `${"x".repeat(83)}…`)
})

test("the plan detail presents the exact server text once", () => {
  const markup = renderToStaticMarkup(createElement(plans.PlanView, { raw: nativePlan, t }))
  assert.match(markup, /data-testid="pg-text-plan"/)
  assert.match(markup, /Merge Join  \(cost=0\.85\.\.81\.42 rows=12 width=64\)/)
  assert.match(markup, /-&gt;  Seq Scan on orders/)
  assert.equal((markup.match(/data-testid="pg-text-plan"/g) ?? []).length, 1)
  assert.doesNotMatch(markup, /<details/)
})

test("null and blank plans have an honest unavailable state", () => {
  for (const raw of [null, "", " \n\t "]) {
    assert.equal(plans.planTextSummary(raw), null)
    const summary = renderToStaticMarkup(createElement(plans.PlanSummary, { raw }))
    const detail = renderToStaticMarkup(createElement(plans.PlanView, { raw, t }))
    assert.match(summary, />—<\/span>/)
    assert.match(detail, /data-testid="pg-plan-unavailable"/)
    assert.doesNotMatch(detail, /data-testid="pg-text-plan"|<button/)
  }
})

const row = (typeId, values) => ({ logicalName: "pg_store_plans", ordinal: "0", segmentId: "s", timestamp: 10, typeId, values })

test("plan navigation uses public shared IDs for OSSC and Datasentinel and the statement ID for vadv", () => {
  const identity = { datname: "app db", dbid: 7, planid: "22", usename: "reader", userid: 8 }
  for (const typeId of ["1003001", "1018001"]) {
    const shared = plans.statementsForPlan(row(typeId, { ...identity, queryid: "11" }))
    assert.deepEqual(shared, { expression: 'database:"app db" AND role:reader AND query_id:11', queryId: "11", section: "statements" })
  }
  const last = plans.statementsForPlan(row("1004001", { ...identity, queryid: 0, queryid_stat_statements: "33" }))
  assert.deepEqual(last, { expression: 'database:"app db" AND role:reader AND query_id:33', queryId: "33", section: "statements" })
  assert.deepEqual(plans.statementTextForPlan(row("1003001", { queryid: "11" })), { queryId: "11" })
  assert.deepEqual(plans.statementTextForPlan(row("1018001", { queryid: "22" })), { queryId: "22" })
  assert.deepEqual(plans.statementTextForPlan(row("1004001", { queryid: "999", queryid_stat_statements: "33" })), { queryId: "33" })
  assert.equal(plans.statementTextForPlan(row("1004001", { queryid: "999", queryid_stat_statements: 0 })), null)
  assert.equal(plans.statementTextForPlan(row("1003001", { queryid: 0 })), null)
  assert.equal(plans.statementTextForPlan(row("1003001", {})), null)
  assert.equal(plans.statementsForPlan(row("1004001", { ...identity, queryid: 0, queryid_stat_statements: 0 })), null)
  assert.equal(plans.statementsForPlan(row("1003001", { ...identity, queryid: 0 })), null)
  assert.equal(plans.statementsForPlan(row("1003001", { ...identity, dbid: 0, queryid: 11 })), null)
  assert.equal(plans.statementsForPlan(row("1003001", { ...identity, queryid: 11, userid: null })), null)
  assert.deepEqual(plans.plansForPlanId(row("1004001", identity)), { expression: "plan_id:22", planId: "22", section: "plans" })
})

test("recorded plan query text keeps exact bytes", () => {
  const text = "  SELECT *\n  FROM jobs\n"
  const markup = renderToStaticMarkup(createElement(plans.QueryView, {
    retry() {}, status: "ready", text, t,
  }))
  assert.equal((markup.match(/data-testid="pg-plan-query-text"/g) ?? []).length, 1)
  assert.match(markup, /  SELECT \*/)
  assert.doesNotMatch(markup, /app|reader|1\/2|pg\.query\.plan\.identical/)
  assert.equal((markup.match(/pg\.plan\.copy/g) ?? []).length, 1)
  assert.match(markup, /max-h-\[min\(320px,35vh\)\] overflow-auto/)
})

test("query failure and unavailable states remain separate from the execution plan", () => {
  for (const status of ["unavailable", "no_bridge", "error"]) {
    const query = renderToStaticMarkup(createElement(plans.QueryView, { retry() {}, status, text: null, t }))
    assert.match(query, new RegExp(`data-query-status="${status}"`))
    assert.match(query, new RegExp(`pg\\.query\\.plan\\.${status}`))
  }
  const plan = renderToStaticMarkup(createElement(plans.PlanView, { raw: nativePlan, t }))
  assert.match(plan, /data-testid="pg-text-plan"/)
})

test("Activity navigation shows every related statement candidate for database and query ID", () => {
  const target = plans.statementsForActivity({ ...row("1001004", { datid: 16384, datname: "app", query_id: "-42" }), logicalName: "pg_stat_activity" })
  assert.deepEqual(target, { expression: "database:app AND query_id:-42", queryId: "-42", section: "statements" })
  assert.equal(plans.statementsForActivity({ ...row("1001004", { datid: 16384, datname: "app", query_id: 0 }), logicalName: "pg_stat_activity" }), null)
  assert.equal(plans.statementsForActivity({ ...row("1001004", { datid: null, datname: "app", query_id: 42 }), logicalName: "pg_stat_activity" }), null)
})

test("Statement navigation opens every matching plan without selecting one", () => {
  const statement = { ...row("1002002", { datname: "app", dbid: 7, queryid: "-42", usename: "reader", userid: 8 }), logicalName: "pg_stat_statements" }
  assert.deepEqual(plans.plansForStatement(statement), {
    expression: "database:app AND role:reader AND query_id:-42", queryId: "-42", section: "plans",
  })
})
