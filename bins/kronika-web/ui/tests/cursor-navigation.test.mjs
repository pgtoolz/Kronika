import assert from "node:assert/strict"
import test from "node:test"
import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { importModule, registryPlugin } from "./import-module.mjs"

const helpers = await importModule('export { snapshotNavigationSections } from "../src/cursor-navigation.tsx"; export { systemNavigationSections, SYSTEM_REQUESTS, SYSTEM_METRICS, metricHistoryRequest } from "../src/system-view.tsx"; export { CursorRow } from "../src/cursor-row.tsx";', { plugins: [registryPlugin([])] })

test("snapshot navigation follows the active source rather than its companion lanes", () => {
  for (const [screen, section] of Object.entries({ activity: "pg_stat_activity", statements: "pg_stat_statements", plans: "pg_store_plans", tables: "pg_stat_user_tables", indexes: "pg_stat_user_indexes", vacuum: "pg_stat_progress_vacuum", locks: "pg_locks", databases: "pg_stat_database" })) {
    assert.deepEqual(helpers.snapshotNavigationSections("postgresql", screen, ["os_cpu"]), [section])
  }
  assert.deepEqual(helpers.snapshotNavigationSections("processes", "activity", []), ["os_process"])
  const overview = helpers.snapshotNavigationSections("postgresql", "overview", [])
  assert.equal(overview.length, 12)
  for (const section of ["pg_stat_activity", "pg_stat_database", "pg_stat_progress_vacuum", "pg_settings", "pg_log_lifecycle"]) assert.ok(overview.includes(section), section)
  for (const [metric, expected] of Object.entries({ cpu_busy: ["os_cpu"], cpu_actual_frequency: ["os_cpufreq"], cpu_pressure: ["os_psi"], memory: ["os_meminfo"], mem_swap: ["os_vmstat"], disk_busy: ["os_diskstats"], filesystem_free_min: ["os_mountinfo"], network_rx: ["os_netdev"], cgroup_cpu: ["os_cgroup_v2_cpu", "os_cgroup_cpu"] })) {
    assert.deepEqual(helpers.snapshotNavigationSections("host", "activity", helpers.systemNavigationSections(metric)), expected)
  }
  const host = helpers.systemNavigationSections(null)
  assert.ok(host.length <= 32)
  assert.equal(new Set(host).size, host.length)
  for (const { section } of helpers.SYSTEM_REQUESTS) assert.ok(host.includes(section), section)
  for (const metric of helpers.SYSTEM_METRICS) {
    const request = helpers.metricHistoryRequest(metric)
    if (request !== null) assert.ok(host.includes(request.section), `${metric.id}: ${request.section}`)
  }
  for (const resource of ["cpu", "memory", "io", "pids"]) {
    for (const prefix of ["os_cgroup_", "os_cgroup_v2_"]) assert.ok(host.includes(prefix + resource), prefix + resource)
  }
})

test("source steps stay available without loaded chart points and after a missing neighbor", () => {
  const row = (pending) => renderToStaticMarkup(createElement(helpers.CursorRow, {
    cursor: 100, cursorTimes: [], onCursor() {}, reading: "", navigation: { pending, step() {} }, t: (key) => key,
  }))
  assert.doesNotMatch(row(false), /disabled/)
  assert.equal((row(true).match(/disabled/g) ?? []).length, 2)
})
