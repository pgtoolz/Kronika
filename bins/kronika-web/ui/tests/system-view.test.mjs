import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import { gunzipSync } from "node:zlib"

import { importModule, registryPlugin } from "./import-module.mjs"
import { parseDictionary, validateDictionaries } from "../scripts/i18n.mjs"

const helpers = await importModule(
  'export { cgroupDevicePresentations, dockGroupMetrics, effectiveCpuCapacity, cgroupSnapshotPlan, chartableEntityColumns, currentValue, entityHistoryRequest, fallbackMetric, hasMetric, metricChartUnit, metricChartValue, metricHistoryPoints, metricHistoryRequest, metricPoints, metricRequestKey, mountPairSeries, recordedEnvironment, resourceBreakdownSeries, sharedCgroupPath, storageTopologyEntries, systemEntityRows, CGROUP_SNAPSHOT_REQUESTS, SYSTEM_ENTITIES, SYSTEM_METRICS, SYSTEM_REQUESTS } from "../src/system-view.tsx"; export { bundledFixtureHour } from "../src/fixture.ts"',
  { plugins: [registryPlugin([
    { typeId: "1108001", logicalName: "os_diskstats", identity: ["major", "minor"], columns: ["ts", "major", "minor", "device", "io_in_progress"] },
    { typeId: "1112002", logicalName: "os_mountinfo", identity: ["major", "minor", "mount_point"], columns: ["ts", "major", "minor", "mount_point", "root", "fstype", "source", "is_k8s_infra", "total_bytes", "free_bytes", "total_inodes", "available_inodes", "scope"] },
  ])] },
)

const rateHelpers = await importModule(
  'export { metricChartUnit, metricHistoryPoints } from "../src/system-view.tsx"',
  { plugins: [registryPlugin([{ typeId: "1108001", logicalName: "os_diskstats", identity: ["major", "minor"], columns: ["ts", "major", "minor", "reads"], columnMetadata: [
    { name: "ts", type: "timestamp_us", class: "timestamp", unit: null },
    { name: "major", type: "i32", class: "label", unit: null },
    { name: "minor", type: "i32", class: "label", unit: null },
    { name: "reads", type: "u64", class: "cumulative", unit: "count" },
  ] }])] },
)

const data = {
  points: [
    { segmentId: "a", series: "test", timestamp: 100, value: 5 },
    { segmentId: "a", series: "test", timestamp: 200, value: null },
    { segmentId: "a", series: "test", timestamp: 300, value: 0 },
  ],
}
const spec = { group: "cpu", help: "x.help", id: "x", label: "x.label", series: "test", unit: "" }

test("system current values use the stored observation at or before the cursor", () => {
  assert.equal(helpers.currentValue(data, spec, 150, "en"), "5")
  assert.equal(helpers.currentValue(data, spec, 200, "en"), "—")
  assert.equal(helpers.currentValue(data, spec, 300, "en"), "0")
  assert.equal(helpers.hasMetric({ points: [{ segmentId: "a", series: "test", timestamp: 1, value: null }] }, spec), false)
  assert.equal(helpers.hasMetric(data, spec), true)
  assert.equal(helpers.fallbackMetric("os_mountinfo"), null)
  assert.equal(helpers.fallbackMetric("os_diskstats"), null)
  const semantic = { points: [{ segmentId: "a", series: "test", timestamp: 100, value: 0.099 }] }
  assert.equal(helpers.currentValue(semantic, { ...spec, unit: "%" }, 100, "en"), "<0.1%")
  assert.equal(helpers.metricPoints(semantic, spec)[0].value, 0.099)
  const core = { points: [{ segmentId: "a", series: "test", timestamp: 100, value: 0.00399 }] }
  assert.equal(helpers.currentValue(core, { ...spec, unit: " cores" }, 100, "en"), "0.004")
  assert.equal(helpers.currentValue(core, { ...spec, unit: " cores" }, 100, "ru"), "0,004")
  assert.equal(helpers.metricChartValue(0.00399, "ru", " cores"), "0,004")
  assert.equal(helpers.metricPoints(core, spec)[0].value, 0.00399)
})

test("system histories omit rows whose layout does not own the selected field", () => {
  const direct = {
    points: [],
    sections: {
      os_vmstat: [
        { segmentId: "a", timestamp: 1, values: { oom_kill: 1 } },
        { segmentId: "a", timestamp: 2, values: { other: null } },
        { segmentId: "b", timestamp: 3, values: { oom_kill: null } },
      ],
    },
  }
  assert.deepEqual(helpers.metricPoints(direct, { ...spec, field: "oom_kill", section: "os_vmstat", series: "missing" }).map((point) => [point.timestamp, point.value]), [[1, 1], [3, null]])
})

test("CPU topology is a static table without selection history", () => {
  const topology = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_topology")
  assert.ok(topology)
  assert.equal(topology.columns.every(({ chartable }) => chartable === false), true)
  assert.deepEqual(helpers.chartableEntityColumns(topology.columns), [])
})

test("CPU frequency stays policy-scoped and keeps actual separate from scaling", () => {
  const row = (policy, timestamp, actual, scaling, online) => ({
    logicalName: "os_cpufreq", ordinal: `${policy}:${timestamp}`, segmentId: "a", timestamp, typeId: "1122001",
    values: { policy_id: policy, actual_source: policy === 0 ? "cpuinfo_avg_freq" : "cpuinfo_cur_freq", actual_frequency_hz: actual, scaling_cur_freq_hz: scaling, online_cpus: online },
  })
  const sections = { os_cpufreq: [
    row(0, 1, 2_000_000_000, 1_800_000_000, 2), row(4, 1, 3_000_000_000, 2_600_000_000, 1),
    row(0, 2, null, 1_900_000_000, 2), row(4, 2, 3_100_000_000, 2_700_000_000, 1),
  ] }
  const actual = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_actual_frequency")
  const scaling = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_scaling_frequency")
  assert.equal(actual.help, "system.metric.cpu_actual_frequency.help")
  assert.equal(scaling.help, "system.metric.cpu_scaling_frequency.help")
  assert.deepEqual(helpers.metricPoints({ points: [], sections }, actual).map(({ value }) => value), [null, null])
  const scalingValues = helpers.metricPoints({ points: [], sections }, scaling).map(({ value }) => value)
  assert.ok(Math.abs(scalingValues[0] - 6_200 / 3) < Number.EPSILON * 2_100)
  assert.ok(Math.abs(scalingValues[1] - 6_500 / 3) < Number.EPSILON * 2_200)
  const series = helpers.resourceBreakdownSeries(actual.id, sections.os_cpufreq, false, "en", (key, values) => key === "system.topology.policy" ? `Policy ${values.policy}` : key)
  assert.deepEqual(series.map(({ label }) => label), ["Policy 0", "Policy 4"])
  assert.deepEqual(series[0].points.map(({ value }) => value), [2_000, null])
})

test("system cards derive production values when fixture-only series are absent", () => {
  const cpu = (timestamp, user, system, idle) => ({
    logicalName: "os_cpu", ordinal: String(timestamp), segmentId: "a", timestamp, typeId: "1102001",
    values: { cpu_id: -1, idle, iowait: 0, irq: 0, nice: 0, scope: 0, softirq: 0, steal: 0, system, user },
  })
  const production = {
    points: [],
    health: [],
    load: [],
    memory: [{ logicalName: "os_meminfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1104001", values: { anon_pages: 30, buffers: 5, cached: 20, mem_available: 25, mem_free: 10, mem_total: 100, s_reclaimable: 5, s_unreclaim: 3 } }],
    pressure: [],
    sections: {
      os_cpu: [cpu(100, 10, 10, 80), { ...cpu(100, 0, 0, 0), values: { cpu_id: 0, scope: 0 } }, cpu(200, 20, 20, 160), { ...cpu(200, 0, 0, 0), values: { cpu_id: 0, scope: 0 } }],
      os_meminfo: [{ logicalName: "os_meminfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1104001", values: { anon_pages: 30, buffers: 5, cached: 20, mem_available: 25, mem_free: 10, mem_total: 100, s_reclaimable: 5, s_unreclaim: 3 } }],
      os_mountinfo: [
        { logicalName: "os_mountinfo", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1112002", values: { free_bytes: 50, total_bytes: 100 } },
        { logicalName: "os_mountinfo", ordinal: "1", segmentId: "a", timestamp: 200, typeId: "1112002", values: { free_bytes: 20, total_bytes: 100 } },
      ],
      os_diskstats: [
        { logicalName: "os_diskstats", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1110001", values: { io_in_progress: 2 } },
        { logicalName: "os_diskstats", ordinal: "1", segmentId: "a", timestamp: 200, typeId: "1110001", values: { io_in_progress: 1 } },
      ],
      os_netdev: [
        { logicalName: "os_netdev", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1111001", values: { rx_bytes: 10, rx_drop: 0, rx_errs: 1, tx_bytes: 40, tx_drop: 2, tx_errs: 0 } },
        { logicalName: "os_netdev", ordinal: "1", segmentId: "a", timestamp: 200, typeId: "1111001", values: { rx_bytes: 20, rx_drop: 3, rx_errs: 0, tx_bytes: null, tx_drop: 0, tx_errs: 0 } },
      ],
      os_vmstat: [{ logicalName: "os_vmstat", ordinal: "0", segmentId: "a", timestamp: 200, typeId: "1106001", values: { oom_kill: 0 } }],
    },
  }
  const derived = (name) => helpers.metricPoints(production, { ...spec, derive: name, series: "missing" }).map((point) => point.value)
  assert.equal(helpers.currentValue(production, { ...spec, derive: "cpu_used_cores", series: "missing", unit: " cores" }, 200, "en"), "0.2")
  const unknownScope = {
    ...production,
    sections: {
      os_cpu: [
        { ...cpu(100, 10, 10, 80), values: { ...cpu(100, 10, 10, 80).values, scope: null } },
        { ...cpu(200, 20, 20, 160), values: { ...cpu(200, 20, 20, 160).values, scope: null } },
      ],
    },
  }
  assert.deepEqual(helpers.metricPoints(unknownScope, { ...spec, derive: "cpu_used_cores", series: "missing" }), [])
  assert.equal(helpers.currentValue(production, { ...spec, field: "mem_available", section: "os_meminfo", series: "missing", unit: " KiB" }, 200, "en"), "25 KiB")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "mem_file_cache", series: "missing", unit: " KiB" }, 200, "en"), "25 KiB")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "mem_other", series: "missing", unit: " KiB" }, 200, "en"), "27 KiB")
  assert.equal(helpers.currentValue(production, { ...spec, derive: "filesystem_free_min", series: "missing" }, 200, "en"), "20")
  assert.equal(helpers.currentValue(production, { ...spec, field: "oom_kill", section: "os_vmstat", series: "missing" }, 200, "en"), "0")
  assert.deepEqual(derived("device_count"), [2])
  assert.deepEqual(derived("device_active_io"), [3])
  assert.deepEqual(derived("filesystem_count"), [2])
  assert.deepEqual(derived("interface_count"), [2])
  assert.deepEqual(derived("network_rx"), [null])
  assert.deepEqual(derived("network_tx"), [null])
  assert.deepEqual(derived("network_errors"), [null])
  assert.deepEqual(derived("network_drops"), [null])
})

test("host CPU categories form a truthful total and keep host capacity separate", () => {
  const aggregate = (timestamp, values) => ({
    logicalName: "os_cpu", ordinal: String(timestamp), segmentId: "s", timestamp, typeId: "1102001",
    values: { cpu_id: -1, scope: 0, ...values },
  })
  const core = (timestamp, id) => ({ logicalName: "os_cpu", ordinal: `${timestamp}-${id}`, segmentId: "s", timestamp, typeId: "1102001", values: { cpu_id: id, scope: 0 } })
  const baseline = { user: 100, nice: 10, system: 50, idle: 500, iowait: 20, irq: 4, softirq: 6, steal: 10 }
  const next = { user: 120, nice: 15, system: 60, idle: 550, iowait: 25, irq: 6, softirq: 9, steal: 15 }
  const rows = [aggregate(1_000_000, baseline), core(1_000_000, 0), core(1_000_000, 1), aggregate(2_000_000, next), core(2_000_000, 0), core(2_000_000, 1)]
  const point = (derive) => helpers.metricHistoryPoints({ ...spec, derive, series: "missing" }, rows).at(-1).value
  assert.deepEqual([point("cpu_user"), point("cpu_system"), point("cpu_irq"), point("cpu_iowait"), point("cpu_steal"), point("cpu_idle")], [25, 10, 5, 5, 5, 50])
  assert.equal(point("cpu_used_cores"), 0.9)
  assert.deepEqual(helpers.metricHistoryPoints({ ...spec, derive: "cpu_capacity", series: "missing" }, rows).map(({ value }) => value), [2, 2])

  const current = { ...data, rateColumns: { os_cpu: ["user"] }, sections: { os_cpu: [aggregate(3_000_000, { user: 20, nice: 5, system: 10, idle: 50, iowait: 5, irq: 2, softirq: 3, steal: 5 }), core(3_000_000, 0), core(3_000_000, 1)] } }
  assert.equal(helpers.metricPoints(current, { ...spec, derive: "cpu_user", series: "missing" })[0].value, 25)
  const reset = [...rows, aggregate(3_000_000, baseline), core(3_000_000, 0), core(3_000_000, 1)]
  assert.equal(helpers.metricHistoryPoints({ ...spec, derive: "cpu_user", series: "missing" }, reset).at(-1).value, null)
})

test("host memory categories do not overlap and available remains a separate estimate", () => {
  const row = { logicalName: "os_meminfo", ordinal: "1", segmentId: "s", timestamp: 1, typeId: "1104001", values: {
    mem_total: 1000, mem_free: 100, mem_available: 450, anon_pages: 300, cached: 200, buffers: 50, s_reclaimable: 50, s_unreclaim: 25,
  } }
  const source = { ...data, sections: { os_meminfo: [row] }, memory: [row] }
  const reading = (derive) => helpers.metricPoints(source, { ...spec, derive, series: "missing" })[0].value
  assert.equal(reading("mem_file_cache"), 250)
  assert.equal(reading("mem_other"), 275)
  assert.equal(300 + 250 + 50 + 25 + 100 + reading("mem_other"), 1000)
  assert.equal(row.values.mem_available, 450)
  const invalid = { ...row, values: { ...row.values, mem_total: 10 } }
  assert.equal(helpers.metricPoints({ ...source, memory: [invalid], sections: { os_meminfo: [invalid] } }, { ...spec, derive: "mem_other", series: "missing" })[0].value, null)
})

test("CPU and memory histories expose their complete operator breakdowns", () => {
  const cpu = (timestamp, id, values = {}) => ({
    logicalName: "os_cpu", ordinal: `${timestamp}-${id}`, segmentId: "s", timestamp, typeId: "1102001",
    values: { cpu_id: id, scope: 0, ...values },
  })
  const cpuRows = [
    cpu(1_000_000, -1, { user: 10, nice: 0, system: 5, idle: 80, iowait: 2, irq: 1, softirq: 1, steal: 1 }),
    cpu(1_000_000, 0), cpu(1_000_000, 1),
    cpu(2_000_000, -1, { user: 30, nice: 5, system: 15, idle: 130, iowait: 7, irq: 3, softirq: 4, steal: 6 }),
    cpu(2_000_000, 0), cpu(2_000_000, 1),
  ]
  const t = (key) => key
  const cpuSeries = helpers.resourceBreakdownSeries("cpu_user", cpuRows, false, "en", t)
  assert.deepEqual(cpuSeries.map(({ id, unit }) => [id, unit]), [
    ["cpu_used_cores", "cores"], ["cpu_capacity", "cores"], ["cpu_user", "%"], ["cpu_system", "%"],
    ["cpu_irq", "%"], ["cpu_iowait", "%"], ["cpu_steal", "%"], ["cpu_idle", "%"],
  ])
  assert.equal(cpuSeries.find(({ id }) => id === "cpu_capacity").points.at(-1).value, 2)

  const memory = [{ logicalName: "os_meminfo", ordinal: "1", segmentId: "s", timestamp: 1, typeId: "1104001", values: {
    mem_total: 1000, mem_available: 450, mem_free: 100, anon_pages: 300, cached: 200, buffers: 50, s_reclaimable: 50, s_unreclaim: 25,
  } }]
  const memorySeries = helpers.resourceBreakdownSeries("mem_anon", memory, false, "en", t)
  assert.deepEqual(memorySeries.map(({ id }) => id), ["mem_total", "mem_available", "mem_anon", "mem_file_cache", "mem_s_reclaimable", "mem_s_unreclaim", "mem_free", "mem_other"])
  assert.equal(memorySeries.find(({ id }) => id === "mem_other").points[0].value, 275)
})

test("device latency uses exact reset-safe counter operands and a stable major:minor identity", () => {
  const disk = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_diskstats")
  const readLatency = disk.columns.find(({ field }) => field === "read_latency_ms")
  const diskRequest = helpers.SYSTEM_REQUESTS.find(({ section }) => section === "os_diskstats")
  assert.equal(diskRequest.fields.includes("io_weighted_time_ms"), true)
  assert.equal(diskRequest.fields.includes("weighted_time_ms"), false)
  const row = (timestamp, reads, readTime) => ({ logicalName: "os_diskstats", ordinal: String(timestamp), segmentId: "s", timestamp, typeId: "1108001", values: { major: 8, minor: 1, reads, read_time_ms: readTime } })
  const points = readLatency.points([
    row(1_000_000, "9007199254740993", "9007199254741000"),
    row(2_000_000, "9007199254740995", "9007199254741010"),
    row(3_000_000, "9007199254740995", "9007199254741020"),
    row(4_000_000, "1", "2"),
  ])
  assert.deepEqual(points.map(({ value }) => value), [null, 5, null, null])
  assert.deepEqual(helpers.entityHistoryRequest(row(2_000_000, "3", "9"), readLatency), {
    fields: ["reads", "read_time_ms", "major", "minor"],
    key: '["1108001",[["major","8"],["minor","1"]],"read_latency_ms"]',
    section: "os_diskstats", typeId: "1108001", where: { major: "8", minor: "1" },
  })
  assert.equal(
    helpers.entityHistoryRequest({ ...row(2_000_000, "3", "9"), segmentId: "next" }, readLatency).key,
    helpers.entityHistoryRequest(row(2_000_000, "3", "9"), readLatency).key,
  )
  const current = helpers.systemEntityRows({ ...data, sections: { os_diskstats: [{ ...row(5_000_000, 2, 10), values: { ...row(5_000_000, 2, 10).values, device: "sda", read_sectors: 3, write_sectors: 4, writes: 0, write_time_ms: 1, io_time_ms: 200, io_weighted_time_ms: 500, io_in_progress: 2 } }] } }, "os_diskstats", 5_000_000)[0]
  assert.equal(current.values.device_id, "8:1")
  assert.equal(current.values.read_latency_ms, 5)
  assert.equal(current.values.write_latency_ms, null)
  assert.equal(current.values.read_bytes, 1536)
  assert.equal(current.values.device_busy, 20)
  assert.equal(current.values.average_queue, 0.5)
})

test("collector cgroup rows keep leaf settings factual and use effective hierarchy capacities", () => {
  const row = (logicalName, path, values, ordinal = path) => ({ logicalName, ordinal, segmentId: "s", timestamp: 10, typeId: logicalName === "os_cgroup_cpu" ? "1201001" : logicalName === "os_cgroup_memory" ? "1202001" : "1203002", values: { cgroup_path: path, scope: 3, ...values } })
  const context = row("os_cgroup_context", "/ignored", {
    cpu_path: "/mine", memory_path: "/mine", io_path: "/mine", cpuset_cpus: 1,
    effective_cpu_quota_usec: 50_000, effective_cpu_period_usec: 100_000, effective_memory_max: 1500,
  }, "context")
  const source = { ...data, sections: {
    os_cgroup_context: [context],
    os_cgroup_cpu: [row("os_cgroup_cpu", "/other", { usage_usec: 9 }), row("os_cgroup_cpu", "/mine", { usage_usec: 1_500_000, user_usec: 1_000_000, system_usec: 400_000, quota_usec: 200_000, period_usec: 100_000 })],
    os_cgroup_memory: [row("os_cgroup_memory", "/mine", { current: 1000, max: 2000, anon: 400, file: 300, kernel: 200, slab: 50 })],
    os_cgroup_io: [row("os_cgroup_io", "/mine", { major: 8, minor: 0, rbytes: 10, wbytes: 20, rios: 1, wios: 2 })],
  } }
  const cpu = helpers.systemEntityRows(source, "os_cgroup_cpu", 10)
  assert.equal(cpu.length, 2)
  const collector = cpu.find(({ values }) => values.cgroup_path === "/mine")
  assert.equal(collector.values.cgroup_used_cores, 1.5)
  assert.equal(collector.values.cgroup_other_cores, 0.1)
  assert.equal(collector.values.cgroup_quota, 2)
  assert.equal(collector.values.cgroup_capacity, 0.5)
  assert.equal(cpu.find(({ values }) => values.cgroup_path === "/other").values.cgroup_capacity, null)
  const columns = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_cgroup_cpu").columns
  for (const field of ["cgroup_used_cores", "cgroup_user_cores", "cgroup_system_cores", "cgroup_other_cores", "cgroup_capacity", "cgroup_quota"]) {
    assert.equal(columns.find((column) => column.field === field).kind, "cores", field)
  }
  const memory = helpers.systemEntityRows(source, "os_cgroup_memory", 10)[0]
  assert.equal(memory.values.effective_memory_max, 1500)
  assert.equal(memory.values.max, 2000)
  assert.equal(memory.values.kernel_other, 150)
  assert.equal(memory.values.memory_unclassified, 100)
  assert.equal(helpers.systemEntityRows(source, "os_cgroup_io", 10)[0].values.device_id, "8:0")
  assert.equal(helpers.effectiveCpuCapacity(null, null, 2), null)
  assert.equal(helpers.effectiveCpuCapacity(-1, null, 2), 2)
  assert.equal(helpers.effectiveCpuCapacity(-1, 100_000, null), null)
  assert.equal(helpers.effectiveCpuCapacity(50_000, 100_000, null), 0.5)
  assert.equal(helpers.effectiveCpuCapacity(300_000, 100_000, 2), 2)
  assert.equal(helpers.effectiveCpuCapacity(50_000, 0, 2), null)
  const malformedContext = { ...context, values: { ...context.values, effective_memory_max: -1 } }
  const malformedMemory = helpers.systemEntityRows({ ...source, sections: { ...source.sections, os_cgroup_context: [malformedContext] } }, "os_cgroup_memory", 10)[0]
  assert.equal(malformedMemory.values.effective_memory_max, null)
})

test("System loads cgroups only for the recorded container environment", () => {
  for (const section of ["os_cgroup_context", "os_cgroup_cpu", "os_cgroup_memory", "os_cgroup_io", "os_cgroup_pids"]) {
    assert.equal(helpers.SYSTEM_REQUESTS.some((request) => request.section === section), false)
    const request = helpers.CGROUP_SNAPSHOT_REQUESTS.find((candidate) => candidate.section === section)
    assert.ok(request, section)
  }
  const tasks = helpers.CGROUP_SNAPSHOT_REQUESTS.find(({ section }) => section === "os_cgroup_pids")
  assert.equal(tasks.fields.includes("current") && tasks.fields.includes("max"), true)
  assert.equal(tasks.fields.includes("tasks_current") || tasks.fields.includes("tasks_max"), false)
  assert.equal(helpers.SYSTEM_REQUESTS.some(({ section, fields }) => section === "instance_metadata" && fields.includes("environment")), true)

  const metadata = (environment) => ({
    logicalName: "instance_metadata", ordinal: "metadata", segmentId: "segment-a", timestamp: 10, typeId: "1021002",
    values: { environment },
  })
  const machine = { sections: { instance_metadata: [metadata(0)] } }
  assert.equal(helpers.recordedEnvironment(machine, 12), "machine")
  assert.deepEqual(helpers.cgroupSnapshotPlan("segment-a", 12, machine).loads, [])

  const container = { sections: { instance_metadata: [metadata(1)] } }
  assert.equal(helpers.recordedEnvironment(container, 12), "container")
  const plan = helpers.cgroupSnapshotPlan("segment-a", 12, container)
  assert.equal(plan.key, '["segment-a",12,"container"]')
  assert.deepEqual(plan.loads.map(({ filters, request }) => [request.section, filters]), [
    ["os_cgroup_cpu", {}],
    ["os_cgroup_memory", {}],
    ["os_cgroup_io", {}],
    ["os_cgroup_pids", {}],
    ["os_cgroup_context", {}],
  ])
  assert.equal(helpers.recordedEnvironment({ sections: {} }, 12), null)
})

test("System never depends on process rows loaded by another view", async () => {
  assert.equal(helpers.SYSTEM_REQUESTS.some(({ section }) => section === "os_process"), false)
  assert.equal(helpers.SYSTEM_METRICS.some(({ id }) => id.startsWith("process_")), false)

  const grouped = new Map(helpers.SYSTEM_METRICS.map((metric) => [metric.id, metric.group]))
  for (const id of ["device_count", "filesystem_count"]) assert.equal(grouped.get(id), "storage")
  for (const id of ["interface_count", "network_rx", "network_tx", "network_errors", "network_drops"]) assert.equal(grouped.get(id), "network")
  const disk = helpers.SYSTEM_REQUESTS.find(({ section }) => section === "os_diskstats")
  assert.ok(disk.fields.includes("major"))
  assert.ok(disk.fields.includes("minor"))
  const source = await readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8")
  assert.match(source, /rows\.length === 0 && activeContext === null/)
})

test("System entity tables keep exact meaning-first orders and rate presentation", () => {
  const fields = Object.fromEntries(helpers.SYSTEM_ENTITIES.map(({ section, columns }) => [section, columns.map(({ field }) => field)]))
  assert.deepEqual(fields.os_diskstats, ["device", "device_id", "reads", "writes", "read_bytes", "write_bytes", "read_latency_ms", "write_latency_ms", "device_busy", "average_queue", "io_in_progress"])
  assert.deepEqual(fields.os_cgroup_cpu, ["cgroup_path", "cgroup_used_cores", "cgroup_user_cores", "cgroup_system_cores", "cgroup_other_cores", "cgroup_capacity", "cgroup_quota", "cpuset_cpus"])
  assert.deepEqual(fields.os_cgroup_memory, ["cgroup_path", "current", "effective_memory_max", "max", "anon", "file", "slab", "kernel_other", "memory_unclassified"])
  assert.deepEqual(fields.os_cgroup_io, ["cgroup_path", "cgroup_device", "device_id", "rbytes", "wbytes", "rios", "wios", "cgroup_device_chain", "cgroup_mount_associations", "cgroup_lower_layers"])
  assert.deepEqual(fields.os_mountinfo, ["mount_point", "root", "source", "fstype", "device_id", "free_bytes", "filesystem_available_percent", "total_bytes", "available_inodes", "inode_available_percent", "total_inodes", "is_k8s_infra"])
  assert.deepEqual(fields.os_netdev, ["iface", "rx_bytes", "tx_bytes", "rx_packets", "tx_packets", "rx_errs", "tx_errs", "rx_drop", "tx_drop", "speed_mbit", "duplex"])
  assert.deepEqual(fields.os_topology, ["cpu_id", "socket_id", "core_id", "numa_node", "model_name", "mhz_max"])

  const disk = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_diskstats")
  for (const field of ["reads", "writes", "read_bytes", "write_bytes"]) assert.equal(disk.columns.find((column) => column.field === field).rate, true)
  for (const field of ["read_latency_ms", "write_latency_ms", "device_busy", "average_queue"]) assert.notEqual(disk.columns.find((column) => column.field === field).historyFields, undefined)
  for (const [field, help] of [
    ["read_latency_ms", "system.field.read_latency_ms.help"],
    ["write_latency_ms", "system.field.write_latency_ms.help"],
    ["device_busy", "system.field.device_busy.help"],
    ["average_queue", "system.field.average_queue.help"],
  ]) assert.equal(disk.columns.find((column) => column.field === field).help, help)
})

test("hidden mount device IDs remain exact request and history identity", () => {
  const mount = helpers.SYSTEM_ENTITIES.find(({ section }) => section === "os_mountinfo")
  const request = helpers.SYSTEM_REQUESTS.find(({ section }) => section === "os_mountinfo")
  assert.ok(request.fields.includes("major"))
  assert.ok(request.fields.includes("minor"))
  assert.equal(mount.columns.some(({ field }) => field === "major" || field === "minor"), false)
  assert.equal(mount.columns.find(({ field }) => field === "fstype").detailValueRole, "machine")
  assert.notEqual(mount.columns.find(({ field }) => field === "total_bytes").detailValueRole, "machine")

  const row = { logicalName: "os_mountinfo", ordinal: "0", segmentId: "s", timestamp: 12, typeId: "1112002", values: { major: 8, minor: 1, mount_point: "/data", free_bytes: 4 } }
  assert.deepEqual(helpers.entityHistoryRequest(row, mount.columns.find(({ field }) => field === "free_bytes")), {
    fields: ["free_bytes", "major", "minor", "mount_point"],
    key: '["1112002",[["major","8"],["minor","1"],["mount_point","/data"]],"free_bytes"]',
    section: "os_mountinfo",
    typeId: "1112002",
    where: { major: "8", minor: "1", mount_point: "/data" },
  })
})

test("System entity headers have exact EN/RU help without obvious or orphan entries", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  const obvious = new Set(["device", "device_id", "cgroup_path", "mount_point", "root", "fstype", "source", "iface", "cpu_id", "model_name"])
  const usedHelp = new Set()
  for (const { columns, section } of helpers.SYSTEM_ENTITIES) for (const column of columns) {
    assert.equal(column.help === undefined, obvious.has(column.field), `${section}/${column.field}`)
    if (column.help === undefined) continue
    usedHelp.add(column.help)
    assert.equal(Object.hasOwn(english, column.help), true, column.help)
    assert.equal(Object.hasOwn(russian, column.help), true, column.help)
  }
  // The mount pair chart's series carry field help without being columns.
  const viewSource = await readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8")
  for (const match of viewSource.matchAll(/system\.field\.([a-z0-9_]+)\.help/g)) usedHelp.add(`system.field.${match[1]}.help`)
  const dictionaryHelp = Object.keys(english).filter((key) => /^system\.field\.[^.]+\.help$/.test(key)).sort()
  assert.deepEqual([...usedHelp].sort(), dictionaryHelp)
})

test("System history requests are selected-metric keys with exact physical inputs", () => {
  const direct = helpers.metricHistoryRequest({ ...spec, field: "oom_kill", section: "os_vmstat", series: undefined })
  assert.deepEqual(direct, { fields: ["oom_kill"], section: "os_vmstat", where: {} })
  const pressure = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_pressure")
  const pressureRequest = helpers.metricHistoryRequest(pressure)
  assert.deepEqual(pressureRequest.where, { resource: "0" })
  assert.ok(pressureRequest.fields.includes("some_avg10"))
  assert.ok(pressureRequest.fields.includes("resource"))
  const cpu = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_user")
  const cpuRequest = helpers.metricHistoryRequest(cpu)
  for (const field of ["cpu_id", "scope", "user", "idle", "iowait"]) assert.ok(cpuRequest.fields.includes(field))
  const memory = helpers.SYSTEM_METRICS.find(({ id }) => id === "mem_anon")
  const memoryRequest = helpers.metricHistoryRequest(memory)
  for (const field of ["mem_total", "mem_available", "mem_free", "cached", "buffers", "anon_pages", "s_reclaimable", "s_unreclaim"]) assert.ok(memoryRequest.fields.includes(field))
  const network = helpers.SYSTEM_METRICS.find(({ id }) => id === "network_rx")
  const networkRequest = helpers.metricHistoryRequest(network)
  assert.ok(networkRequest.fields.includes("rx_bytes"))
  assert.ok(networkRequest.fields.includes("tx_bytes"))
  assert.equal(helpers.metricRequestKey(100, cpu, cpuRequest), helpers.metricRequestKey(100, cpu, cpuRequest))
  assert.notEqual(helpers.metricRequestKey(100, cpu, cpuRequest), helpers.metricRequestKey(200, cpu, cpuRequest))
  assert.equal(helpers.metricChartUnit({ ...spec, unit: " KiB" }, "en"), "B")
  assert.equal(helpers.metricChartValue(16_777_216, "en", " KiB"), "16 GiB")
  assert.equal(helpers.metricChartValue(256, "ru", " KiB"), "256 KiB")
  assert.equal(helpers.metricChartUnit({ ...spec, unit: " B" }, "en"), "")
  assert.equal(helpers.metricChartUnit({ ...spec, unit: " B" }, "ru"), "")
  assert.equal(helpers.metricChartUnit({ ...spec, id: "network_errors" }, "en"), "1/s")
  assert.equal(helpers.metricChartUnit({ ...spec, id: "network_drops" }, "ru"), "1/с")
})

test("the dock offers one chip for a breakdown instead of a strip that repeats the chart legend", () => {
  const metric = (id, group) => ({ group, help: `${id}.help`, id, label: `${id}.label`, unit: "" })
  const cpu = ["cpu_used_cores", "cpu_capacity", "cpu_user", "cpu_system", "procs_running", "procs_blocked"].map((id) => metric(id, "cpu"))
  const dock = helpers.dockGroupMetrics(cpu, "cpu_used_cores")
  assert.deepEqual(dock.chips.map(({ id }) => id), ["cpu_used_cores", "procs_running", "procs_blocked"])
  assert.equal(dock.chartChip("cpu_user"), "cpu_used_cores")
  assert.equal(dock.chartChip("procs_running"), "procs_running")
  const memory = ["mem_total", "mem_anon", "swap_free"].map((id) => metric(id, "memory"))
  const fallback = helpers.dockGroupMetrics(memory, "mem_available")
  assert.deepEqual(fallback.chips.map(({ id }) => id), ["mem_total", "swap_free"])
  const network = ["network_rx", "network_tx"].map((id) => metric(id, "network"))
  const plain = helpers.dockGroupMetrics(network, undefined)
  assert.deepEqual(plain.chips.map(({ id }) => id), ["network_rx", "network_tx"])
  assert.equal(plain.chartChip("network_tx"), "network_tx")
  const exactUse = { ...metric("disk_busy", "storage"), useOnly: true }
  const detail = metric("device_busy", "storage")
  assert.deepEqual(helpers.dockGroupMetrics([exactUse, detail], "disk_busy").chips.map(({ id }) => id), ["device_busy"])
})

test("System entity charts include numeric measurements and exclude identities and categories", () => {
  const columns = [
    { field: "major", kind: "id" },
    { field: "device", kind: "text" },
    { field: "is_k8s_infra", kind: "boolean" },
    { field: "captured_at", kind: "timestamp" },
    { field: "reads", kind: "number" },
    { field: "read_time_ms", kind: "milliseconds" },
    { field: "total_bytes", kind: "bytes" },
  ]
  assert.deepEqual(helpers.chartableEntityColumns(columns).map(({ field }) => field), ["reads", "read_time_ms", "total_bytes"])
  const row = {
    logicalName: "os_diskstats", ordinal: "4", segmentId: "s", timestamp: 12, typeId: "1108001",
    values: { major: 8, minor: 0, reads: 0 },
  }
  const request = helpers.entityHistoryRequest(row, columns[4])
  assert.deepEqual(request.where, { major: "8", minor: "0" })
  assert.deepEqual(request.fields, ["reads", "major", "minor"])
  assert.equal(request.section, "os_diskstats")
  assert.equal(request.typeId, "1108001")
  assert.equal(helpers.entityHistoryRequest(row, columns[0]), null)
  assert.deepEqual(helpers.metricHistoryPoints({ ...spec, field: "reads", section: "os_diskstats", series: undefined }, [
    { ...row, timestamp: 1, values: { ...row.values, reads: 0 } },
    { ...row, timestamp: 2, values: { major: 8, minor: 0 } },
    { ...row, timestamp: 3, values: { ...row.values, reads: null } },
  ]).map(({ timestamp, value }) => [timestamp, value]), [[1, 0], [3, null]])
})

test("a cumulative metric stays absent until its section announces rate columns", async () => {
  const spec = { field: "reads", group: "storage", help: "x", id: "reads", label: "x", section: "os_diskstats", unit: "" }
  const row = { logicalName: "os_diskstats", ordinal: "0", segmentId: "a", timestamp: 1, typeId: "1108001", values: { major: 8, minor: 0, reads: 10 } }
  const registry = [
    { typeId: "1108001", logicalName: "os_diskstats", identity: ["major", "minor"], columns: ["ts", "major", "minor", "reads"], columnMetadata: [
      { name: "ts", type: "timestamp_us", class: "timestamp", unit: null },
      { name: "major", type: "i32", class: "label", unit: null },
      { name: "minor", type: "i32", class: "label", unit: null },
      { name: "reads", type: "u64", class: "cumulative", unit: "count" },
    ] },
  ]
  const cumulative = await importModule('export { hasMetric } from "../src/system-view.tsx"', { plugins: [registryPlugin(registry)] })
  assert.equal(cumulative.hasMetric({ points: [], sections: { os_diskstats: [row] }, rateColumns: {} }, spec), false)
  assert.equal(cumulative.hasMetric({ points: [], sections: { os_diskstats: [row] }, rateColumns: { os_diskstats: ["reads"] } }, spec), true)
})

test("the storage rollups peak across devices and honor pre-computed rates", () => {
  const row = (timestamp, major, ioTime) => ({ logicalName: "os_diskstats", ordinal: `${major}:${timestamp}`, segmentId: "a", timestamp, typeId: "1108001", values: { major, minor: 0, io_time_ms: ioTime } })
  const busy = helpers.SYSTEM_METRICS.find(({ id }) => id === "device_busy")
  const counter = helpers.metricPoints({ points: [], sections: { os_diskstats: [
    row(1_000_000, 7, 100), row(1_000_000, 8, 300),
    row(2_000_000, 7, 200), row(2_000_000, 8, 700),
  ] }, rateColumns: { os_diskstats: ["io_time_ms"] } }, busy).map(({ value }) => value)
  assert.deepEqual(counter, [30, 70])
  const storedRates = helpers.metricPoints({ points: [], sections: { os_diskstats: [row(1_000_000, 7, 0.2), row(1_000_000, 8, 0.7)] }, rateColumns: { os_diskstats: ["io_time_ms"] } }, busy)
  assert.ok(Math.abs((storedRates[0]?.value ?? 0) - 0.07) < 1e-9)
})

test("the storage dock breaks device busy down to the devices that registered activity", () => {
  const row = (timestamp, major, device, ioTime) => ({
    logicalName: "os_diskstats", ordinal: `${major}:${timestamp}`, segmentId: "a", timestamp, typeId: "1108001",
    values: { major, minor: 0, device, io_time_ms: ioTime, io_weighted_time_ms: ioTime },
  })
  const rows = [
    row(1_000_000, 8, "sda", 100), row(2_000_000, 8, "sda", 400),
    row(1_000_000, 9, "sdb", 50), row(2_000_000, 9, "sdb", 50),
  ]
  const series = helpers.resourceBreakdownSeries("device_busy", rows, false, "en", (key) => key)
  assert.deepEqual(series.map(({ label }) => label), ["sda"])
  assert.deepEqual(series[0].points.map(({ value }) => value), [null, 30])
  assert.equal(series[0].unit, "%")
  const allIdle = helpers.resourceBreakdownSeries("device_busy", [row(1_000_000, 9, "sdb", 50), row(2_000_000, 9, "sdb", 50)], false, "en", (key) => key)
  assert.deepEqual(allIdle.map(({ label }) => label), ["sdb"])
  const rated = helpers.resourceBreakdownSeries("device_busy", [row(1_000_000, 8, "sda", 0.42)], true, "en", (key) => key)
  assert.deepEqual(rated.map(({ label }) => label), ["sda"])
  assert.ok(Math.abs((rated[0].points[0]?.value ?? 0) - 0.042) < 1e-9)
  const queue = helpers.resourceBreakdownSeries("device_average_queue", [row(1_000_000, 8, "sda", 42)], true, "en", (key) => key)
  assert.deepEqual(queue.map(({ label }) => label), ["sda"])
  assert.ok(Math.abs((queue[0].points[0]?.value ?? 0) - 0.042) < 1e-9)
})

test("registry cumulative fields become reset-safe rates across storage segments", () => {
  const spec = { field: "reads", group: "storage", help: "x", id: "reads", label: "x", section: "os_diskstats", unit: "" }
  const row = (segmentId, timestamp, reads) => ({ logicalName: "os_diskstats", ordinal: String(timestamp), segmentId, timestamp, typeId: "1108001", values: { major: 8, minor: 0, reads } })
  assert.deepEqual(rateHelpers.metricHistoryPoints(spec, [
    row("a", 1_000_000, 10), row("a", 2_000_000, 14), row("b", 3_000_000, 20), row("b", 4_000_000, null), row("b", 5_000_000, 1), row("b", 6_000_000, 3),
  ]).map(({ value }) => value), [null, 4, 6, null, null, 2])
  assert.equal(rateHelpers.metricChartUnit(spec, "en"), "1/s")
})

test("a mount history charts exact available against total without inventing used space", () => {
  const row = (timestamp, free, total) => ({
    logicalName: "os_mountinfo", ordinal: String(timestamp), segmentId: "a", timestamp, typeId: "1112002",
    values: { free_bytes: free, total_bytes: total },
  })
  const t = (key) => key
  const series = helpers.mountPairSeries([row(1_000_000, 300, 1000), row(2_000_000, 250, 1000)], t)
  assert.deepEqual(series.map(({ id }) => id), ["available_bytes", "total_bytes"])
  assert.deepEqual(series[0].points.map(({ value }) => value), [300, 250])
  assert.deepEqual(series[1].points.map(({ value }) => value), [1000, 1000])
  // The exact available gauge is preserved even when the source is internally
  // inconsistent; the UI does not manufacture an allocated-space result.
  const broken = helpers.mountPairSeries([row(1_000_000, 1500, 1000)], t)
  assert.deepEqual(broken[0].points.map(({ value }) => value), [1500])
  assert.deepEqual(helpers.mountPairSeries([{ ...row(1_000_000, null, null), values: {} }], t), [])
})

test("a mount inode history charts exact available against total", () => {
  const row = (timestamp, available, total) => ({
    logicalName: "os_mountinfo", ordinal: String(timestamp), segmentId: "a", timestamp, typeId: "1112002",
    values: { available_inodes: available, total_inodes: total },
  })
  const series = helpers.mountPairSeries([row(1_000_000, 300, 1000), row(2_000_000, 250, 1000)], (key) => key, "inodes")
  assert.deepEqual(series.map(({ id }) => id), ["available_inodes", "total_inodes"])
  assert.deepEqual(series[0].points.map(({ value }) => value), [300, 250])
  assert.deepEqual(series[1].points.map(({ value }) => value), [1000, 1000])
})

test("storage topology exposes only recorded exact edges and mount roots", () => {
  const row = (logicalName, values) => ({ logicalName, ordinal: "0", segmentId: "a", timestamp: 1, typeId: "0", values })
  assert.deepEqual(helpers.storageTopologyEntries(
    [],
    [
      row("os_block_topology", { major: 259, minor: 1, parent_major: 259, parent_minor: 0 }),
      row("os_block_topology", { major: 252, minor: 0, parent_major: 259, parent_minor: 1 }),
      row("os_block_topology", { major: 252, minor: 0, parent_major: 8, parent_minor: 16 }),
    ],
    [row("os_mountinfo", { major: 259, minor: 1, mount_point: "/data", root: "/subvol", source: "/dev/nvme0n1p1" })],
  ), [
    { associations: [], id: "252:0", name: "252:0", parents: ["259:1", "8:16"] },
    { associations: [{ infrastructure: false, mountPoint: "/data" }], id: "259:1", name: "259:1", parents: ["259:0"] },
  ])
})

// io.stat charges dm-0 and the disk under it for the same bytes: the table
// keeps the mounted volume and carries the disk's counters in its Inspector.
test("a charged lower layer leaves the cgroup I/O table and rides in the top row", () => {
  const row = (logicalName, ordinal, values) => ({ logicalName, ordinal, segmentId: "a", timestamp: 10, typeId: logicalName === "os_cgroup_io" ? "1203002" : logicalName, values })
  const source = {
    ...helpers.bundledFixtureHour(),
    sections: {
      os_cgroup_io: [
        row("os_cgroup_io", "dm", { cgroup_path: "/", major: 252, minor: 0, rbytes: 100, wbytes: 200, rios: 3, wios: 4, scope: 3 }),
        row("os_cgroup_io", "disk", { cgroup_path: "/", major: 259, minor: 0, rbytes: 100, wbytes: 190, rios: 3, wios: 4, scope: 3 }),
      ],
      os_block_topology: [
        row("os_block_topology", "dm", { major: 252, minor: 0, parent_major: 259, parent_minor: 4, scope: 0 }),
        row("os_block_topology", "p4", { major: 259, minor: 4, parent_major: 259, parent_minor: 0, scope: 0 }),
      ],
      os_diskstats: [
        row("os_diskstats", "dm-0", { major: 252, minor: 0, device: "dm-0", scope: 0 }),
        row("os_diskstats", "nvme0n1", { major: 259, minor: 0, device: "nvme0n1", scope: 0 }),
      ],
      os_mountinfo: [row("os_mountinfo", "data", { major: 252, minor: 0, mount_point: "/var/lib/kronika/data", root: "/volumes/x", source: "/dev/mapper/data-docker", is_k8s_infra: false, scope: 0 })],
    },
  }
  const rows = helpers.systemEntityRows(source, "os_cgroup_io", 10)
  assert.equal(rows.length, 1)
  const [top] = rows
  assert.equal(top.values.device_id, "252:0")
  assert.equal(top.values.cgroup_device, "/var/lib/kronika/data")
  assert.equal(top.values.cgroup_device_secondary, "data-docker · dm-0 → nvme0n1")
  assert.equal(top.values.cgroup_device_chain, "dm-0 252:0 → 259:4 → nvme0n1 259:0")
  assert.deepEqual(top.values.cgroup_lower_layers, { layers: [{ id: "259:0", name: "nvme0n1", rbytes: 100, wbytes: 190, rios: 3, wios: 4 }] })
  const otherDisk = row("os_cgroup_io", "other-disk", { cgroup_path: "/other", major: 259, minor: 0, rbytes: 700, wbytes: 900, rios: 7, wios: 9, scope: 3 })
  for (const ioRows of [[...source.sections.os_cgroup_io, otherDisk], [otherDisk, ...source.sections.os_cgroup_io]]) {
    const mixed = helpers.systemEntityRows({ ...source, sections: { ...source.sections, os_cgroup_io: ioRows } }, "os_cgroup_io", 10)
    assert.equal(mixed.length, 2, "a lower device charged by another cgroup keeps its own row")
    const own = mixed.find(({ values }) => values.cgroup_path === "/")
    const other = mixed.find(({ values }) => values.cgroup_path === "/other")
    assert.deepEqual(own.values.cgroup_lower_layers, top.values.cgroup_lower_layers, "the Inspector receives only its cgroup's lower counters")
    assert.equal(other.values.rbytes, 700)
    assert.equal(other.values.cgroup_lower_layers, undefined)
    assert.equal(other.values.cgroup_device, "/var/lib/kronika/data", "the other cgroup can borrow the uncharged mounted layer")
  }
  // The physical row still exists as data: the same hour without edges shows both.
  const flat = helpers.systemEntityRows({ ...source, sections: { ...source.sections, os_block_topology: [] } }, "os_cgroup_io", 10)
  assert.deepEqual(flat.map(({ values }) => [values.device_id, values.cgroup_device, values.cgroup_device_secondary]), [["252:0", "/var/lib/kronika/data", "data-docker · dm-0"], ["259:0", "nvme0n1", null]])
})

test("a shared cgroup path is displayed once without hiding mixed paths", () => {
  const row = (path) => ({ logicalName: "os_cgroup_io", ordinal: path, segmentId: "a", timestamp: 1, typeId: "1203002", values: { cgroup_path: path } })
  assert.equal(helpers.sharedCgroupPath([row("/"), row("/")]), "/")
  assert.equal(helpers.sharedCgroupPath([row("/"), row("/child")]), null)
  assert.equal(helpers.sharedCgroupPath([]), null)
})

test("the committed hour supplies only honest System metrics with complete histories", async () => {
  const encoded = await readFile(new URL("../fixtures/real-hour.json.gz", import.meta.url))
  const fixture = JSON.parse(gunzipSync(encoded).toString("utf8"))
  Object.assign(globalThis, { __KRONIKA_REAL_HOUR__: fixture })
  const hourStart = Math.floor(Number(fixture.meta.captureFromUs) / 3_600_000_000) * 3_600_000_000
  const hour = helpers.bundledFixtureHour(hourStart)
  assert.notEqual(hour, null)

  const available = helpers.SYSTEM_METRICS.map((metric) => ({ metric, points: helpers.metricPoints(hour, metric) }))
    .filter(({ points }) => points.some((point) => point.value !== null && Number.isFinite(point.value)))
  // The health card left with its duplicate; the fixture's own metrics remain.
  assert.ok(available.length >= 6, `available metrics: ${available.length}`)
  // A resource owns its metrics now: load rides with the CPU, PSI with the
  // resource it presses on.
  assert.deepEqual([...new Set(available.map(({ metric }) => metric.group))].sort(), ["cpu", "memory", "storage"])

  // Health is read from the top bar and the timeline lane; a metric card here
  // would be the same number a third time.
  assert.equal(helpers.SYSTEM_METRICS.some((metric) => metric.id === "health"), false)
  Reflect.deleteProperty(globalThis, "__KRONIKA_REAL_HOUR__")
})

test("System is one ledger: rows expand in place and the chart lives on the page", async () => {
  const [source, styles] = await Promise.all([
    readFile(new URL("../src/system-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
  ])
  // One operator question at a time inside a row: cgroup accounting stays
  // dedicated, and Disk separates device I/O, filesystems and topology.
  assert.match(source, /disk: \["io", "filesystems", "topology"\]/)
  // The container rail is the collector cgroup's own lanes, never a filter that leaves host lanes behind.
  assert.match(source, /<Timeline cursor=\{cursor\} environment=\{environment\}/)
  assert.doesNotMatch(source, /lane\.startsWith\("pg_"\)/)
  assert.match(source, /EXACT_TIMELINE_METRIC_LANES[^\n]+\["cpu_busy", "cpu_stall", "memory"\]/)
  assert.doesNotMatch(source, /function groupLane/)
  assert.doesNotMatch(source, /function timelineLane/)
  assert.match(source, /if \(section === "storage"\)[\s\S]*mode === "filesystems"[\s\S]*\["os_mountinfo"\]/)
  // The container scope is four real USE rows: each discloses its own cgroup
  // table, CPU and I/O carry the cgroup activity ledgers, and nothing is
  // force-opened or drawn as a card strip above the ledger.
  assert.match(source, /isContainerResource\(key\) \? \[sectionName\] : sectionEntities\(sectionName, mode\)/)
  assert.match(source, /key === "cgroup_cpu" && data\.availableSections\.includes\("os_cgroup_cpu"\) && <CgroupActivity/)
  assert.match(source, /key === "cgroup_io" && data\.availableSections\.includes\("os_cgroup_io"\) && <CgroupActivity/)
  assert.match(source, /onOpenRow=\{openRow\}/)
  assert.doesNotMatch(source, /ContainerCgroupOverview|cgroup-overview|openedContainer|afterCgroups/)
  // The ledger is the page: expansion is disclosure, the group chart renders
  // inline, and no machinery force-opens an Inspector to fake content.
  assert.match(source, /renderExpansion=\{renderExpansion\}/)
  assert.match(source, /<SystemGroupChart /)
  assert.doesNotMatch(source, /dismissedOverview|autoMetric|SystemDock|metric-choice|metric-grid/)
  assert.match(source, /SYSTEM_METRICS\.find\(\(spec\) => spec\.id === metric\)/)
  // Entity panels say loading while their snapshot catches up; only a section
  // the hour does not carry at all stays absent.
  assert.match(source, /rows\.length === 0 && activeContext === null && requestPhase === "ready"/)
  assert.equal((source.match(/<TableRequestPlaceholder/g) ?? []).length, 3)
  assert.match(source, /<CpuTopologyReference[^>]*requestPhase=\{requestPhase\}/)
  assert.match(source, /<StorageTopologyReference[\s\S]*?requestPhase=\{requestPhase\}[\s\S]*?\/>/)
  assert.match(source, /empty=\{t\("system\.no_metrics"\)\} phase=\{requestPhase\}/)
  assert.match(source, /empty=\{t\("status\.no_data"\)\} phase=\{requestPhase\}/)
  assert.doesNotMatch(source, /metric-history|system-console|system-layout/)
  assert.doesNotMatch(styles, /\.metric-history|\.system-console|\.system-layout/)
})

test("CPU usage is the hour's own reading: no section rows, no request, and the CPU row opens on it", () => {
  const busy = helpers.SYSTEM_METRICS.find(({ id }) => id === "cpu_busy")
  assert.equal(busy.group, "cpu")
  assert.equal(busy.unit, "%")
  // The reading the resource ledger leads with. It arrives with the hour, so
  // it reads even when the browser holds no os_cpu rows for the window — the
  // case that used to leave the CPU chart offering load averages only.
  // Its chart still asks for the per-CPU history: the usage line carries a
  // share breakdown, fetched with the same inputs the share metrics use.
  const busyRequest = helpers.metricHistoryRequest(busy)
  assert.equal(busyRequest.section, "os_cpu")
  for (const field of ["cpu_id", "scope", "user", "system", "idle", "iowait"]) assert.ok(busyRequest.fields.includes(field), field)
  const lanes = { lanePoints: [
    { lane: "cpu_busy", segmentId: "a", timestamp: 100, value: 12.5 },
    { lane: "cpu_stall", segmentId: "a", timestamp: 100, value: 3 },
    { lane: "cpu_busy", segmentId: "a", timestamp: 200, value: null },
  ], points: [], sections: {} }
  assert.deepEqual(helpers.metricPoints(lanes, busy), [
    { segmentId: "a", timestamp: 100, value: 12.5 },
    { segmentId: "a", timestamp: 200, value: null },
  ])
  // It leads the CPU group, and the breakdown still collapses into one chip.
  const ids = helpers.SYSTEM_METRICS.filter(({ group }) => group === "cpu").map(({ id }) => id)
  assert.equal(ids[0], "cpu_busy")
  const chips = helpers.dockGroupMetrics(helpers.SYSTEM_METRICS.filter(({ group }) => group === "cpu"), "cpu_busy").chips.map(({ id }) => id)
  assert.equal(chips[0], "cpu_busy")
  assert.ok(chips.includes("cpu_used_cores"))
  for (const member of ["cpu_user", "cpu_system", "cpu_idle", "cpu_iowait"]) assert.equal(chips.includes(member), false)
})

test("the usage chart draws the recorded share components under its own line", () => {
  // Two aggregate readings 10 s apart: 25% user, 12.5% system of a 2-CPU host.
  const tick = (ordinal, ts, user, system) => ({ segmentId: "a", typeId: "1102001", ordinal: String(ordinal), timestamp: ts, values: { cpu_id: -1, scope: 0, user, nice: 0, system, idle: 0, iowait: 0, irq: 0, softirq: 0, steal: 0 } })
  const core = (ordinal, ts, id) => ({ segmentId: "a", typeId: "1102001", ordinal: String(ordinal), timestamp: ts, values: { cpu_id: id, scope: 0, user: 1, nice: 0, system: 1, idle: 1, iowait: 0, irq: 0, softirq: 0, steal: 0 } })
  const rows = [
    tick(0, 0, 1000, 1000), core(1, 0, 0), core(2, 0, 1),
    tick(3, 10_000_000, 1500, 1250), core(4, 10_000_000, 0), core(5, 10_000_000, 1),
  ]
  const series = helpers.resourceBreakdownSeries("cpu_busy", rows, false, "en", (key) => key)
  assert.deepEqual(series.map(({ id }) => id), ["cpu_user", "cpu_system", "cpu_irq", "cpu_iowait", "cpu_steal", "cpu_idle"])
  for (const one of series) assert.equal(one.scale, "percent")
  // Colours step past the usage line the chart prepends, so no pair collides.
  assert.equal(new Set(series.map(({ color }) => color)).size, series.length)
})
