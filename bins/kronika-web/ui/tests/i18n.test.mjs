import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

import { parseDictionary, validateDictionaries } from "../scripts/i18n.mjs"

test("flat dictionaries reject unstable keys, duplicates and empty values", () => {
  assert.throws(() => parseDictionary('справка.процесс: "x"', "sample"), /invalid stable key/)
  assert.throws(() => parseDictionary('app.title: "A"\napp.title: "B"', "sample"), /duplicate key/)
  assert.throws(() => parseDictionary('app.title: "  "', "sample"), /nonempty/)
})

test("locale validation checks key and placeholder parity", () => {
  assert.throws(
    () => validateDictionaries({ "app.title": "A" }, { "app.name": "A" }),
    /key mismatch/,
  )
  assert.throws(
    () => validateDictionaries({ "app.title": "At {time}" }, { "app.title": "В {date}" }),
    /placeholder mismatch/,
  )
  assert.deepEqual(
    validateDictionaries({ "app.title": "At {time}" }, { "app.title": "В {time}" }),
    ["app.title"],
  )
})

test("transient export admission is concise in both locales", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  assert.equal(english["export.error.export_busy"], "The server is already preparing another export, try again shortly")
  assert.equal(russian["export.error.export_busy"], "Сервер уже готовит другой экспорт, повторите чуть позже")
})

test("PostgreSQL buffer and block metric labels stay canonical English in RU", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  for (const key of [...Object.keys(english), ...Object.keys(russian)]) assert.doesNotMatch(key, /[А-Яа-яЁё]/u, key)
  const keys = Object.keys(english).filter((key) => !key.startsWith("filter.field.") && key.endsWith(".label") && /(?:blks|blocks|buffer_hit)/.test(key))
  assert.ok(keys.length >= 29)
  for (const key of keys) assert.equal(russian[key], english[key], key)
  assert.equal(english["pg.field.shared_blks_read.label"], "Shared buffer read bytes")
  assert.equal(english["pg.field.shared_blks_hit.label"], "Shared buffer hit bytes")
  assert.equal(english["pg.field.temp_blks_written.label"], "Temp buffer written bytes")
  assert.match(russian["pg.field.shared_blks_read.help"], /[А-Яа-яЁё]/u)
  assert.equal(russian["filter.field.buffer_hit.label"], english["filter.field.buffer_hit.label"])
})

test("quantitative search labels and unit tokens stay canonical English in RU", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  const quantitative = [
    "call_rate", "exec_time_rate", "mean_exec", "row_rate", "rows_per_call", "plan_rate", "planning_time_rate", "planning_share",
    "shared_buffer_hit_rate", "shared_buffer_read_rate", "shared_buffer_dirty_rate", "shared_buffer_write_rate", "local_buffer_hit_rate", "local_buffer_read_rate",
    "local_buffer_dirty_rate", "local_buffer_write_rate", "temp_buffer_read_rate", "temp_buffer_write_rate", "shared_read_time_rate", "shared_write_time_rate",
    "local_read_time_rate", "local_write_time_rate", "temp_read_time_rate", "temp_write_time_rate", "wal_rate", "wal_per_call", "buffer_per_call", "slow_call_rate",
    "exec_cv", "min_exec_since_reset", "max_exec_since_reset", "mean_exec_since_reset", "stddev_exec_since_reset", "rss", "vsz", "swap", "threads",
    "cpu_cores", "user_cpu_cores", "system_cpu_cores", "disk_read_rate", "disk_write_rate", "logical_read_rate", "logical_write_rate",
    "read_syscall_rate", "write_syscall_rate", "major_fault_rate", "minor_fault_rate", "context_switch_rate",
    "voluntary_context_switch_rate", "involuntary_context_switch_rate", "run_delay", "block_io_delay",
  ]
  for (const field of quantitative) assert.equal(russian[`filter.field.${field}.label`], english[`filter.field.${field}.label`], field)
  for (const token of ["/s", "MiB", "ms/s", "CPU", "RSS", "WAL"]) {
    assert.ok(Object.values(russian).some((value) => value.includes(token)), token)
  }
})

test("RU keeps technical labels in English and localizes help", async () => {
  const [englishSource, russianSource, activitySource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
    readFile(new URL("../src/activity.tsx", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)

  const canonical = [
    "nav.sources", "inspector.timeline", "system.history", "pg.sections", "pg.section.activity", "pg.section.statements",
    "lens.tree", "pg.section.vacuum", "detail.pg_snapshot.label", "process.summary.postgresql", "process.summary.major_faults",
    "col.cpu_percent.label", "col.mem_percent.label", "col.stat.label", "col.starttime.label", "col.cpu_time.label", "col.minflt.label", "col.majflt.label",
    "system.metric.mem_s_reclaimable.label", "system.metric.mem_s_unreclaim.label", "system.metric.swap_free.label", "system.metric.swap_total.label",
    "system.metric.cpu_pressure.label", "system.metric.memory_pressure.label", "system.metric.io_pressure.label", "system.metric.filesystem_free_min.label",
    "system.metric.network.label", "system.field.cpu_id.label", "filter.field.size.label", "filter.field.table_count.label", "filter.field.index_count.label",
    "filter.field.buffer_hit.label", "filter.field.seq_scan_rate.label", "filter.field.change_rate.label", "filter.field.autovacuum_rate.label",
    "filter.field.autovacuum_mean.label", "filter.field.xid_age.label", "filter.field.scan_rate.label", "pg.pid.label", "pg.datid.label",
    "pg.vacuum.at_sample", "pg.vacuum.load.read.label", "pg.vacuum.load.write.label", "pg.vacuum.load.block_wait.label",
    "lane.cpu_stall.label", "lane.io_stall.label", "events.source.locks", "events.source.archiver", "events.source.cgroup_memory",
    "events.metric.data_corruption", "pg.field.checksum_failures.label", "pg.field.min_mxid_age.label", "pg.field.sessions_fatal.label",
    "pg.field.sessions_killed.label", "pg.field.failed_count.label",
    "use.lane.mem_oom", "use.lane.cg_oom", "use.lane.cg_cpu_throttle",
    "pg.lens.label", "pg.lens.load", "pg.lens.per_call", "pg.lens.io", "pg.lens.resources", "pg.lens.stability",
    "pg.value.legend", "pg.value.good", "pg.value.warning", "pg.value.critical",
    "activity.title", "activity.retry", "activity.cut_label", "activity.cut.exec_time", "activity.cut.calls", "activity.cut.rows",
    "activity.cut.shared_read", "activity.cut.shared_dirtied", "activity.cut.temp_written", "activity.cut.wal_bytes", "activity.top",
    "activity.totals", "activity.others", "activity.scale_label", "activity.scale.global", "activity.scale.row", "activity.maximize",
    "activity.restore", "activity.nested", "activity.others_label", "activity.top_label",
    ...[
      "query", "datname", "usename", "queryid", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call",
      "rows_per_second", "rows_per_call", "blocks_per_call", "hit_pct", "shared_blks_read", "shared_blks_hit",
      "shared_blks_dirtied", "shared_blks_written", "local_blks_read", "temp_blks_read", "temp_blks_written", "wal_bytes",
      "wal_per_call", "planning_ms_per_second", "plan_time_pct", "cv", "min_exec_time_ms", "max_exec_time_ms",
      "mean_exec_time_ms", "stddev_exec_time_ms",
    ].map((field) => `pg.field.${field}.label`),
  ]
  for (const key of canonical) assert.equal(russian[key], english[key], key)
  for (const [key, value] of Object.entries({
    "use.scope.container": "Контейнер",
    "use.scope.namespace": "Сетевое пространство",
    "use.scope.host": "Хост",
    "use.utilisation": "Использование",
    "use.saturation": "Насыщение",
    "use.errors": "Ошибки",
    "use.resource.memory": "Память",
    "use.resource.disk": "Хранилище",
    "use.resource.network": "Сеть",
    "use.resource.namespace_network": "Сеть",
    "use.lane.cpu_busy": "Занято",
    "use.lane.memory": "Использовано",
    "use.lane.mem_swap": "Подкачка",
    "use.lane.disk_busy": "Занятость устройств",
    "use.lane.disk_queue": "Глубина очереди",
    "use.lane.net_drop": "Потери",
    "use.lane.net_errors": "Ошибки",
  })) assert.equal(russian[key], value, key)
  for (const key of Object.keys(english).filter((candidate) => candidate.startsWith("pg.vacuum.") && candidate.endsWith(".label"))) {
    assert.equal(russian[key], english[key], key)
  }
  assert.equal(english["filter.field.seq_scan_rate.label"], "Seq scan rate")
  assert.equal(russian["events.field.holding_pids"], english["events.field.holding_pids"])
  for (const key of Object.keys(english).filter((candidate) =>
    candidate.startsWith("pg.vitals.band.") || (candidate.startsWith("pg.vitals.row.") && !candidate.endsWith(".help")))) {
    assert.equal(russian[key], english[key], key)
  }

  const localizedActivity = new Set(["activity.loading", "activity.error", "activity.empty", "activity.average", "activity.at_cursor"])
  assert.equal(russian["activity.average"], "Среднее")
  assert.equal(russian["activity.at_cursor"], "На курсоре")
  for (const key of Object.keys(english).filter((candidate) => candidate.startsWith("activity.") && !candidate.endsWith(".help") && !localizedActivity.has(candidate))) {
    assert.equal(russian[key], english[key], key)
  }
  for (const [key, label] of Object.entries({
    "activity.cut.writes": "Rows changed",
    "activity.cut.seq_read": "Rows read by seq scans",
    "activity.cut.heap_read": "Heap buffer read bytes",
    "activity.cut.idx_tup_read": "Index tuples read",
    "activity.cut.idx_blks_read": "Index buffer read bytes",
    "activity.cut.rss": "RSS",
    "activity.tables.members": "{count} tables",
    "activity.indexes.members": "{count} indexes",
  })) assert.equal(english[key], label, key)

  for (const [key, label] of [["col.user.label", "User"], ["col.effective_user.label", "Effective user"]]) {
    assert.equal(english[key], label, key)
    assert.equal(russian[key], label, key)
  }
  for (const key of [
    "filter.field.user.help", "filter.field.effective_user.help", "filter.field.user_id.help", "filter.field.effective_user_id.help",
    "col.user.help", "col.effective_user.help",
  ]) assert.match(russian[key], /[А-Яа-яЁё]/u, key)

  for (const key of Object.keys(english).filter((candidate) => candidate.startsWith("activity.") && candidate.endsWith(".help"))) {
    assert.match(russian[key], /[А-Яа-яЁё]/u, key)
  }
  for (const key of ["activity.loading", "activity.error", "activity.empty"]) assert.match(russian[key], /[А-Яа-яЁё]/u, key)
  for (const text of Object.entries(english).filter(([key]) => key.startsWith("activity.")).map(([, value]) => value)) {
    assert.doesNotMatch(text, /chatty|heavy query|hint|drives|pressures|too small|loads replication|argues/i)
  }
  for (const text of Object.entries(russian).filter(([key]) => key.startsWith("activity.")).map(([, value]) => value)) {
    assert.doesNotMatch(text, /болтлив|тяж[её]л|намека|давит|нагружает|ему мал|довод/i)
  }
  const technicalScope = Object.entries(russian)
    .filter(([key]) => /^(?:pg\.(?:field|vacuum|vitals)|system\.(?:field|metric)|events|filter\.field)\./.test(key))
    .map(([, value]) => value)
    .join("\n")
  assert.doesNotMatch(technicalScope, /бэкенд|кортеж|вакуум|очистк|контрольн\S*\s+точ|PSI CPU|PSI I\/O|последовательн\S*\s+скан|сканирован\S*\s+индекс/iu)
  assert.equal(russian["pg.detail.query"], english["pg.detail.query"])
  assert.equal(russian["pg.detail.plan"], english["pg.detail.plan"])
  assert.match(activitySource, /t\("pg\.detail\.query", \{ id: queryId \?\? "—" \}\)/)
  assert.match(activitySource, /t\("pg\.detail\.plan", \{ id: planId \?\? "—" \}\)/)
  assert.doesNotMatch(activitySource, /`queryid /)
})

test("hour empty states are provisional only while the selected hour is open", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)

  assert.equal(english["status.no_data_current"], "No data yet. Refresh may show new data.")
  assert.equal(english["status.no_data_completed"], "No data was recorded in this hour.")
  assert.equal(russian["status.no_data_current"], "Данных пока нет. После обновления они могут появиться.")
  assert.equal(russian["status.no_data_completed"], "За этот час данные не записаны.")
})

test("project dictionaries cover the active UI keys", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)

  const roots = new Set()
  const literalKey = /["']((?:app|nav|section|status|hour|locale|help|common|export|lens|process|system|lane|locator|table|col|detail|pg|events)\.[a-z0-9_.]+)["']/g
  const sourceFiles = [
    "app.tsx",
    "detail.tsx",
    "events-view.tsx",
    "export-dialog.tsx",
    "help.tsx",
    "hour-picker.tsx",
    "postgres-view.tsx",
    "plan-view.tsx",
    "process-table.tsx",
    "system-view.tsx",
    "timeline.tsx",
  ]
  for (const file of sourceFiles) {
    const source = await readFile(new URL(`../src/${file}`, import.meta.url), "utf8")
    for (const match of source.matchAll(literalKey)) roots.add(match[1])
    const dynamicPrefix = file === "system-view.tsx" ? "system.field" : file === "postgres-view.tsx" ? "pg.field" : null
    if (dynamicPrefix !== null) {
      for (const match of source.matchAll(/(?:text|number|id|bytes|milliseconds|timestamp|boolean|rateNumber|rateBytes|rateMilliseconds)\("([a-z0-9_]+)"/g)) {
        roots.add(`${dynamicPrefix}.${match[1]}`)
      }
    }
  }

  const required = new Set([
    ...["host", "processes", "postgresql", "events"].map((name) => `nav.${name}`),
    ...["overview", "cpu", "memory", "storage", "network"].map((name) => `section.${name}`),
    ...["generic", "cpu", "memory", "disk", "tree"].map((name) => `lens.${name}`),
    ...["event", "known_bad", "spike"].map((name) => `locator.${name}`),
    ...["overview", "activity", "statements", "plans", "locks", "databases", "vacuum"].map((name) => `pg.section.${name}`),
  ])
  for (const root of roots) {
    if (Object.hasOwn(english, root)) {
      required.add(root)
      continue
    }
    required.add(`${root}.label`)
    if (Object.hasOwn(english, `${root}.help`)) required.add(`${root}.help`)
  }

  const missing = [...required].filter((key) => !Object.hasOwn(english, key)).sort()
  assert.deepEqual(missing, [])
  assert.equal(Object.keys(english).some((key) => key.startsWith("system_table.")), false)
  assert.equal(Object.hasOwn(english, "detail.os_layout.label"), false)
  assert.equal(Object.hasOwn(english, "detail.pg_layout.label"), false)
})

test("plan copy identifies unavailable values and vadv attribution", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")

  assert.equal(english["common.unavailable"], "—")
  assert.equal(russian["common.unavailable"], "—")
  assert.equal(english["pg.section.plans"], "Plans")
  assert.equal(russian["pg.section.plans"], "Plans")
  for (const key of ["nav.host", "nav.processes", "nav.postgresql", "nav.events"]) {
    assert.equal(russian[key], english[key])
  }
  assert.equal(russian["pg.activity.idle"], "Idle")
  assert.equal(russian["pg.field.query_duration_ms.label"], "Query time")
  assert.equal(russian["pg.field.transaction_duration_ms.label"], "Xact time")
  assert.equal(english["pg.vitals.row.wal_size"], "pg_wal size")
  assert.equal(russian["pg.vitals.row.wal_size"], "pg_wal size")
  assert.match(russian["pg.vitals.row.wal_size.help"], /[А-Яа-яЁё]/u)
  assert.match(english["pg.field.queryid_stat_statements.help"], /vadv fork of pg_store_plans/)
  assert.match(russian["pg.field.queryid_stat_statements.help"], /форком pg_store_plans от vadv/)
  for (const text of [...Object.values(english), ...Object.values(russian)]) {
    assert.doesNotMatch(text, /not an exact join key|не точн(?:ый|ым) ключ соединения/i)
  }
})

test("dense-table help is factual, concise, and complete in both locales", async () => {
  const [englishSource, russianSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  validateDictionaries(english, russian)
  const pgFields = new Set([
    "pid", "backend_age_ms", "query_duration_ms", "transaction_duration_ms", "state_duration_ms", "queryid", "planid", "toplevel", "datname", "usename", "query", "plan",
    "calls", "calls_per_second", "execution_ms_per_second", "mean_exec_ms_per_call", "rows_per_call", "blocks_per_call", "hit_pct", "wal_per_call",
    "plan_time_pct", "cv", "min_exec_time_ms", "max_exec_time_ms", "mean_exec_ms", "stddev_exec_time_ms", "first_call", "last_call",
    "rows_per_second", "planning_ms_per_second", "shared_blks_hit", "shared_blks_read", "shared_blks_written", "shared_blks_dirtied",
    "local_blks_read", "temp_blks_read", "temp_blks_written", "wal_bytes", "queryid_stat_statements", "cmd_type", "numbackends",
    "xact_commit", "xact_rollback", "blks_hit", "blks_read", "tup_returned", "tup_fetched", "tup_inserted", "tup_updated", "tup_deleted",
    "deadlocks", "conflicts", "temp_files", "temp_bytes", "blk_read_time", "blk_write_time", "sessions", "frozen_xid_age",
    "application_name", "state", "wait_event_type", "wait_event", "blocked_by", "lock_locktype", "lock_mode", "lock_target", "lock_relname", "waitstart",
  ])
  const exact = new Set([
    "pg.datname.help", "pg.usename.help", "pg.application_name.help", "pg.client_addr.help", "pg.backend_type.help", "pg.state.help",
    "pg.wait_event_type.help", "pg.wait_event.help", "pg.query.help", "pg.field.statement_database.help", "pg.field.plan_database.help",
    "pg.leader_pid.help", "pg.query_id.help", "pg.backend_xid_age.help", "pg.backend_xmin_age.help", "pg.backend_start.help",
    "pg.xact_start.help", "pg.query_start.help", "pg.state_change.help",
  ])
  const audited = (key) => key.endsWith(".help") && (
    key.startsWith("col.") || key.startsWith("system.field.") || key.startsWith("pg.help.relation.") || key.startsWith("pg.vacuum.")
    || exact.has(key) || key.startsWith("pg.field.") && pgFields.has(key.slice("pg.field.".length, -".help".length))
  )
  const dictionaries = [["en", english], ["ru", russian]]

  for (const [locale, dictionary] of dictionaries) {
    const helpEntries = Object.entries(dictionary).filter(([key]) => audited(key))
    assert.ok(helpEntries.length > 100)
    for (const [key, value] of helpEntries) {
      assert.doesNotMatch(value, locale === "en"
        ? /\b(?:compare|inspect|check|open|recommend|threshold|diagnos|server sort|nulls? last|raw backend)\b/i
        : /\b(?:сопоставьте|проверьте|смотрите|откройте|рекоменду|порог|диагноз|сортировк\S* на сервере|raw backend)\b/iu, `${locale} ${key}`)
      const sentences = value.match(/[.!?](?:\s|$)/g)?.length ?? 0
      assert.ok(sentences >= 1 && sentences <= 2, `${locale} ${key} must contain one or two sentences`)
      assert.ok(value.length <= 360, `${locale} ${key} is too long`)
    }
  }

  assert.match(english["pg.field.calls.help"], /exact cumulative counter.*separate from Calls\/s/i)
  assert.equal(english["pg.field.calls_per_second.help"], "Statement or plan executions per second in the calculation interval.")
  assert.equal(english["pg.field.execution_ms_per_second.help"], "Accumulated execution time per wall-clock second in the calculation interval. Concurrent executions add together, so the value can exceed 1000 ms/s.")
  assert.equal(english["pg.field.mean_exec_ms_per_call.help"], "Mean execution time per call in the calculation interval; unavailable without calls.")
  assert.equal(english["pg.field.rows_per_second.help"], "Rows returned or affected per second in the calculation interval.")
  assert.equal(english["pg.field.statement_database.help"], "Database associated with this physical statement entry.")
  assert.match(russian["pg.field.calls.help"], /точный накопительный счётчик отделён от Calls\/s/i)
  assert.equal(russian["pg.field.calls_per_second.help"], "Число выполнений запроса или плана в секунду за расчётный интервал.")
})

test("obsolete status and internal collection copy stay out of the UI", async () => {
  const [englishSource, russianSource, appSource, eventsSource, helpSource, processSource, detailSource] = await Promise.all([
    readFile(new URL("../i18n/en.yaml", import.meta.url), "utf8"),
    readFile(new URL("../i18n/ru.yaml", import.meta.url), "utf8"),
    readFile(new URL("../src/app.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/events-view.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/help.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/process-table.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/detail.tsx", import.meta.url), "utf8"),
  ])
  const english = parseDictionary(englishSource, "en.yaml")
  const russian = parseDictionary(russianSource, "ru.yaml")
  const removed = [
    "app.kicker", "app.offline", "help.intro", "col.scope.label", "col.scope.help",
    "pg.field.relid.label", "pg.field.indexrelid.label", "pg.relation.scope.database", "pg.relation.scope.schema", "pg.relation.scope.table", "pg.relation.scope.index", "locator.spike.help",
  ]
  for (const key of removed) {
    assert.equal(Object.hasOwn(english, key), false)
    assert.equal(Object.hasOwn(russian, key), false)
  }
  assert.equal(Object.keys(english).some((key) => key.startsWith("system.metric.process_")), false)
  assert.equal(Object.keys(russian).some((key) => key.startsWith("system.metric.process_")), false)

  assert.equal(english["common.raw"], "Copy exact value")
  assert.equal(russian["common.raw"], "Скопировать точное значение")
  assert.equal(english["lens.generic"], "General")
  assert.equal(russian["lens.generic"], "General")
  assert.equal(english["process.summary.running"], "Runnable")
  assert.equal(russian["process.summary.running"], "Runnable")
  assert.equal(english["locator.spike"], "Sharp rise")
  assert.equal(russian["locator.spike"], "Резкий рост")
  assert.doesNotMatch(englishSource, /Local · offline|Hover over|No source row|collection scope/)
  assert.doesNotMatch(russianSource, /Наведите указатель|исходное значение|исходной строки|Область сбора/)
  assert.doesNotMatch(appSource, /\[\.\.\.HELP_SYSTEM, \.\.\.HELP_PROCESS\]/)
  assert.match(appSource, /label: "system\.metric\.health\.label", help: "lane\.health\.os_health\.help"/)
  assert.doesNotMatch(eventsSource, /lens-tabs|locator\.spike/)
  assert.doesNotMatch(helpSource, /help\.intro|help-intro/)
  assert.doesNotMatch(processSource, /col\.scope|idField\("scope"/)
  assert.doesNotMatch(detailSource, /col\.scope|processField\("scope"/)
  assert.doesNotMatch(detailSource, /col\.starttime/)
})
