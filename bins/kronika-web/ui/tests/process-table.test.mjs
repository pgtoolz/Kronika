import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { importModule, registryPlugin } from "./import-module.mjs"

const helpers = await importModule('export { LENS_FIELDS, PROCESS_SUMMARY_FIELDS, PROCESS_SUMMARY_METRICS, PROCESS_USER_FIELDS, processRowLabel, processSummaryFormat, processSummaryOutput, processSummaryPoints, processSummaryReducer, processSummaryUnit, processTableDefaultOrder, processUser, processUserSearch } from "../src/process-table.tsx"; export { sticky, stickyOffsets } from "../src/entity-table.tsx"', { plugins: [registryPlugin([])] })
const { LENS_FIELDS } = helpers

test("process lenses keep identity first, lens metrics next, and state last", () => {
  const fields = (lens) => LENS_FIELDS[lens].map(({ id }) => id)
  assert.deepEqual(fields("generic"), [
    "pid", "command", "ppid", "user", "effective_user", "gid", "egid", "num_threads", "tty", "exit_signal", "state",
  ])
  assert.deepEqual(fields("cpu"), [
    "pid", "command", "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw",
    "curcpu", "nice", "prio", "rtprio", "policy", "state",
  ])
  assert.deepEqual(fields("memory"), [
    "pid", "command", "rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt", "state",
  ])
  assert.deepEqual(fields("disk"), [
    "pid", "command", "read_bytes", "write_bytes", "syscr", "syscw", "rchar", "wchar",
    "cancelled_write_bytes", "blkdelay_ticks", "state",
  ])
  assert.deepEqual(fields("tree"), [
    "user", "pid", "cpu_percent", "mem_percent", "vmem_kb", "rmem_kb", "tty", "process_stat", "starttime", "cpu_time_seconds", "command",
  ])
})

test("process users retain exact numeric identity and honest unresolved fallback", () => {
  const [user, effective] = helpers.PROCESS_USER_FIELDS
  const row = (values) => ({ logicalName: "os_process", ordinal: "1", segmentId: "a", timestamp: 1, typeId: "1100001", values })
  assert.equal(helpers.processUser(row({ uid: 26, user: "postgres" }), user), "postgres")
  assert.equal(helpers.processUser(row({ euid: 9999, effective_user: null }), effective), "9999")
  assert.equal(helpers.processUser(row({ euid: 9999, effective_user: "  " }), effective), "9999")
  assert.equal(helpers.processUserSearch(row({ uid: 26, user: "postgres" }), user), "user:postgres")
  assert.equal(helpers.processUserSearch(row({ euid: 27, effective_user: "postgres worker" }), effective), 'effective_user:"postgres worker"')
  assert.equal(helpers.processUserSearch(row({ euid: 9999, effective_user: null }), effective), null)
  assert.deepEqual(helpers.processTableDefaultOrder("generic"), { column: "pid", descending: false })
  assert.deepEqual(helpers.processTableDefaultOrder("disk"), { column: "read_bytes", descending: true })
})

test("process lenses share the measured identity width contract", () => {
  for (const lens of ["generic", "cpu", "memory", "disk", "tree"]) {
    const columns = LENS_FIELDS[lens]
    assert.equal(columns.find(({ id }) => id === "pid").size, 82, `${lens} PID`)
    assert.equal(columns.find(({ id }) => id === "command").size, lens === "tree" ? 620 : 340, `${lens} Command`)
  }
  for (const lens of ["generic", "cpu", "memory", "disk"]) {
    assert.equal(LENS_FIELDS[lens].find(({ id }) => id === "state").size, 50, `${lens} State`)
  }
  assert.equal(LENS_FIELDS.generic.find(({ id }) => id === "user").size, 88)
  assert.equal(LENS_FIELDS.tree.find(({ id }) => id === "user").size, 88)
  assert.equal(LENS_FIELDS.generic.find(({ id }) => id === "effective_user").size, 114)
  assert.equal(LENS_FIELDS.tree.find(({ id }) => id === "process_stat").size, 62)
  assert.equal(LENS_FIELDS.tree.find(({ id }) => id === "starttime").size, 92)
})

test("process and summarized PostgreSQL lens tabs read as one control without a fake splitter", async () => {
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
  assert.match(styles, /\.process-workspace > \.lensbar > \.lens-tabs,\s*\.pg-table-workspace > \.lensbar:has\(> \.process-summary-inline\) > \.lens-tabs \{[^}]*background: var\(--color-s1\)[^}]*border: 1px solid/)
  assert.doesNotMatch(styles, /^\.lensbar > \.lens-tabs \{[^}]*border: 1px/m)
  assert.doesNotMatch(styles, /^\s*\.lensbar > \.process-summary-inline \{[^}]*border-left/m)
})

test("Tree row accessibility names the parent and depth without reading connector glyphs", () => {
  const t = (key, slots = {}) => `${key}:${Object.entries(slots).map(([name, stored]) => `${name}=${stored}`).join(",")}`
  const root = { logicalName: "os_process", ordinal: "1", segmentId: "a", timestamp: 1, typeId: "1100001", values: { cmdline: "/sbin/init", pid: 1, process_tree_depth: 0 } }
  const child = { ...root, ordinal: "2", values: { cmdline: "postgres", pid: 22, process_tree_depth: 2, process_tree_parent_pid: 10 } }
  assert.match(helpers.processRowLabel(root, "tree", t), /^process\.tree\.row\.root:command=\/sbin\/init,pid=1/)
  assert.match(helpers.processRowLabel(child, "tree", t), /^process\.tree\.row\.child:.*command=postgres.*depth=2.*parent=10.*pid=22/)
  assert.match(helpers.processRowLabel(child, "cpu", t), /^table\.activate:pid=22/)
})

test("process sticky headers share live offsets and stacking classes with their cells", async () => {
  const offsets = helpers.stickyOffsets([
    { id: "pid", size: 86, sticky: true },
    { id: "command", size: 344, sticky: true },
    { id: "rmem_kb", size: 142, sticky: false },
  ])
  assert.deepEqual([...offsets], [["pid", 0], ["command", 86]])
  const head = helpers.sticky({ sticky: "sticky-pid" }, true).split(" ")
  const body = helpers.sticky({ numeric: true, sticky: "sticky-command" }, false).split(" ")
  for (const name of ["entity-header-cell", "entity-sticky", "sticky-pid", "sticky", "left-0", "z-40"]) assert.ok(head.includes(name), `${name} in ${head}`)
  for (const name of ["entity-cell", "align-right", "entity-sticky", "sticky-command", "sticky", "left-0", "z-[12]"]) assert.ok(body.includes(name), `${name} in ${body}`)
  const source = await readFile(new URL("../src/entity-table.tsx", import.meta.url), "utf8")
  const styles = await readFile(new URL("../src/styles.css", import.meta.url), "utf8")
  assert.equal(source.match(/left: pinnedLeft\.get\(/g)?.length, 2)
  assert.match(source, /max-\[760px\]:static/)
})

test("all sixteen process aggregate readings use the exact complete-set history projection", async () => {
  assert.deepEqual(helpers.PROCESS_SUMMARY_FIELDS, [
    "processes", "threads", "runnable", "postgresql",
    "user_cores", "system_cores", "run_delay_ms_per_second", "context_switches_per_second",
    "resident_kib", "virtual_kib", "swap_kib", "major_faults_per_second",
    "read_bytes_per_second", "write_bytes_per_second", "read_calls_per_second", "write_calls_per_second",
  ])
  assert.deepEqual(Object.fromEntries(Object.entries(helpers.PROCESS_SUMMARY_METRICS).map(([lens, metrics]) => [lens, metrics.map(({ field }) => field)])), {
    generic: ["processes", "threads", "runnable", "postgresql"],
    cpu: ["user_cores", "system_cores", "run_delay_ms_per_second", "context_switches_per_second"],
    memory: ["resident_kib", "virtual_kib", "swap_kib", "major_faults_per_second"],
    disk: ["read_bytes_per_second", "write_bytes_per_second", "read_calls_per_second", "write_calls_per_second"],
    tree: ["processes", "threads", "runnable", "postgresql"],
  })
  const source = await readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8")
  assert.match(source, /loadSeries\(hour, "os_process_summary", \{\}, PROCESS_SUMMARY_FIELDS, controller\.signal\)/)
  assert.doesNotMatch(source, /summaryMetrics|sum\(rows,|linkedPids.*ProcessSummary/)
})

test("process summary charts preserve absent, null, zero, storage and human units", () => {
  const metric = (field) => Object.values(helpers.PROCESS_SUMMARY_METRICS).flat().find((candidate) => candidate.field === field)
  const row = (segmentId, timestamp, values) => ({ logicalName: "os_process_summary", ordinal: String(timestamp), segmentId, timestamp, typeId: "0", values })
  assert.deepEqual(helpers.processSummaryPoints([
    row("a", 1, {}),
    row("a", 2, { resident_kib: null }),
    row("a", 3, { resident_kib: 0 }),
    row("b", 4, { resident_kib: 2 }),
  ], metric("resident_kib")), [
    { segmentId: "a", timestamp: 2, value: null },
    { segmentId: "a", timestamp: 3, value: 0 },
    { segmentId: "b", timestamp: 4, value: 2048 },
  ])
  const t = (key) => ({ "unit.per_second": "/s", "unit.cores": " cores", "unit.ms_per_second": " ms/s" })[key] ?? key
  assert.equal(helpers.processSummaryOutput(205, metric("processes"), "en", t), "205")
  assert.equal(helpers.processSummaryOutput(1.25, metric("user_cores"), "en", t), "1.25")
  assert.equal(helpers.processSummaryOutput(0.00399, metric("user_cores"), "en", t), "0.004")
  assert.equal(helpers.processSummaryOutput(0, metric("run_delay_ms_per_second"), "en", t), "0 ms/s")
  assert.equal(helpers.processSummaryOutput(1_048_576, metric("read_bytes_per_second"), "en", t), "1 MiB/s")
  assert.equal(helpers.processSummaryOutput(null, metric("threads"), "en", t), "—")
  assert.equal(helpers.processSummaryFormat(metric("resident_kib"), t)(1_048_576, "en"), "1 MiB")
  assert.equal(helpers.processSummaryFormat(metric("read_bytes_per_second"), t)(1_048_576, "en"), "1 MiB/s")
  assert.equal(helpers.processSummaryFormat(metric("processes"), t)(205, "en"), "205")
  assert.equal(helpers.processSummaryFormat(metric("user_cores"), t)(1.25, "en"), "1.25")
  assert.equal(helpers.processSummaryFormat(metric("user_cores"), t)(1.2304, "en"), "1.23")
  assert.equal(helpers.processSummaryFormat(metric("run_delay_ms_per_second"), t)(0, "en"), "0 ms/s")
  assert.equal(helpers.processSummaryUnit(metric("resident_kib"), "en", t), "B")
  assert.equal(helpers.processSummaryUnit(metric("read_bytes_per_second"), "en", t), "B/s")
  assert.equal(helpers.processSummaryUnit(metric("processes"), "en", t), "count")
  assert.equal(helpers.processSummaryUnit(metric("user_cores"), "en", t), "cores")
  assert.equal(helpers.processSummaryUnit(metric("run_delay_ms_per_second"), "en", t), "")
})

test("process summary request states retain rows only within the requested hour", () => {
  const firstHour = 100
  const secondHour = 200
  const rows = [{ logicalName: "os_process_summary", ordinal: "1", segmentId: "a", timestamp: 1, typeId: "0", values: { processes: 719 } }]
  const loading = helpers.processSummaryReducer({ hour: null, history: [], status: "loading" }, { hour: firstHour, type: "loading" })
  const ready = helpers.processSummaryReducer(loading, { hour: firstHour, type: "loaded", rows })
  assert.deepEqual(ready, { hour: firstHour, history: rows, status: "ready" })
  assert.deepEqual(helpers.processSummaryReducer(ready, { hour: firstHour, type: "loading" }), { hour: firstHour, history: rows, status: "loading" })
  assert.deepEqual(helpers.processSummaryReducer(ready, { hour: firstHour, type: "error" }), { hour: firstHour, history: rows, status: "error" })

  const changed = helpers.processSummaryReducer(ready, { hour: secondHour, type: "loading" })
  assert.deepEqual(changed, { hour: secondHour, history: [], status: "loading" })
  assert.deepEqual(helpers.processSummaryReducer(changed, { hour: firstHour, type: "loaded", rows }), changed)
  assert.deepEqual(helpers.processSummaryReducer(changed, { hour: firstHour, type: "error" }), changed)
  const failed = helpers.processSummaryReducer(changed, { hour: secondHour, type: "error" })
  assert.deepEqual(failed, { hour: secondHour, history: [], status: "error" })
  assert.deepEqual(helpers.processSummaryReducer(failed, { hour: secondHour, type: "loaded", rows: [] }), { hour: secondHour, history: [], status: "empty" })
})
