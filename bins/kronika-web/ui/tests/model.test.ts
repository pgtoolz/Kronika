import assert from "node:assert/strict"
import test from "node:test"

import type { Cell, DataRow } from "../src/api.ts"
import { fittedWidth } from "../src/column-size.ts"
import { activityFor, compact, cores, estimatedRows, humanAge, humanBytes, humanCores, humanDuration, humanDurationAxis, humanPercent, identifier, measure, millisecondsPerSecond, nearestTime, processCommand, processCpuTime, processDefaultSort, processKey, processLens, processTty, rawText, stateText, type Locale } from "../src/model.ts"

function row(timestamp: number): DataRow {
  return { segmentId: "7", logicalName: "os_process", typeId: "1100001", ordinal: "0", timestamp, values: {} }
}

test("nearest stored time chooses the earlier value on a tie", () => {
  assert.equal(nearestTime([row(100), row(300)], 200), 100)
})

test("null is a dash while zero remains data and identifiers are ungrouped", () => {
  assert.equal(measure(null, "en"), "—")
  assert.equal(measure(0, "en"), "0")
  assert.equal(identifier("1234567"), "1234567")
  assert.equal(stateText(82), "R")
})

test("text payloads retain their exact text value", () => {
  const payload: Cell = { representation: "text", stored_text: "SELECT 1", full_len: "8", truncated: true, sha256: "abc" }
  assert.equal(rawText(payload), "SELECT 1")
})

test("integer lists retain exact ungrouped identifiers", () => {
  assert.equal(rawText([1234, 5678]), "[1234,5678]")
})

test("activity linking uses the cursor-nearest PG snapshot and exact PID", () => {
  const process = { ...row(100), values: { pid: 77, comm: "worker", cmdline: null } }
  const activityAt100 = { ...row(100), values: { pid: 77, backend_type: "old" } }
  const activityAt300 = { ...row(300), values: { pid: 77, backend_type: "nearest" } }
  const activityOtherPid = { ...row(300), ordinal: "1", values: { pid: 78, backend_type: "other" } }
  const linked = activityFor(process, [activityAt100, activityAt300, activityOtherPid], 290)
  assert.equal(linked.snapshotTime, 300)
  assert.equal(linked.row?.values.backend_type, "nearest")
  assert.equal(processCommand(process), "worker")
})

test("activity linking filters exact PID before choosing its nearest per-database snapshot", () => {
  const process = { ...row(300), values: { pid: 77, comm: "postgres", cmdline: null } }
  const matching = { ...row(286), values: { pid: 77, datname: "app", query: "select exact_backend" } }
  const globallyCloserOtherDatabase = { ...row(299), ordinal: "1", values: { pid: 78, datname: "postgres", query: "select wrong_backend" } }
  const linked = activityFor(process, [globallyCloserOtherDatabase, matching], 300)
  assert.equal(linked.snapshotTime, 286)
  assert.equal(linked.row?.values.query, "select exact_backend")
  assert.equal(activityFor({ ...process, values: { pid: 79 } }, [globallyCloserOtherDatabase, matching], 300).row, null)
})

test("finding fields select the matching process lens", () => {
  assert.equal(processLens("read_bytes"), "disk")
  assert.equal(processLens("rmem_kb"), "memory")
  assert.equal(processLens("rundelay_ns"), "cpu")
  assert.equal(processLens("state"), "generic")
})

test("process identity is PID-only within the selected hour", () => {
  const process = (pid: number, starttime: string): DataRow => ({
    ...row(100),
    values: { pid, starttime },
  })
  assert.equal(processKey(process(41, "99")), processKey(process(41, "99")))
  assert.equal(processKey(process(41, "99")), processKey(process(41, "100")))
  assert.notEqual(processKey(process(41, "99")), processKey(process(42, "99")))
})

test("process tables start with CPU and disk sorting chooses the first active signal", () => {
  const process = (values: DataRow["values"]): DataRow => ({ ...row(100), values })
  assert.equal(processDefaultSort("generic", []), "pid")
  assert.equal(processDefaultSort("cpu", []), "utime")
  assert.equal(processDefaultSort("memory", []), "rmem_kb")
  assert.equal(processDefaultSort("disk", [process({ read_bytes: 0, write_bytes: 4, syscr: 9 })]), "write_bytes")
  assert.equal(processDefaultSort("disk", [process({ read_bytes: 0, write_bytes: 0, syscr: 9 })]), "syscr")
  assert.equal(processDefaultSort("disk", [process({ read_bytes: 0, write_bytes: 0, syscr: 0, syscw: 0 })]), "read_bytes")
})

test("rates and ratios stay bounded without turning small nonzero values into zero", () => {
  assert.equal(humanBytes(1_572_864, "en"), "1.5 MiB")
  assert.equal(humanBytes(512, "en"), "512 B")
  assert.equal(humanBytes(0.49, "en", "/s"), "0.49 B/s")
  assert.equal(humanBytes(41.729068244136855, "en", "/s"), "41.7 B/s")
  assert.equal(humanBytes(null, "en"), "—")
  assert.equal(cores(161.01, "en", 100), "1.61")
  assert.equal(cores(0.4, "en", 100), "0.004")
  assert.equal(humanCores(0.00399, "en", " cores"), "0.004 cores")
  assert.equal(humanCores(1.23, "en", " cores"), "1.23 cores")
  assert.equal(humanCores(1.23, "ru", " ядра"), "1,23 ядра")
  assert.equal(humanCores(null, "en"), "—")
  assert.equal(cores(161.01, "en", null), "—")
  assert.equal(cores(161.01, "en", 0), "—")
  assert.equal(millisecondsPerSecond(111_622_111.53, "en"), "112")
  assert.equal(millisecondsPerSecond(40_000, "en"), "0.04")
})

test("byte values use locale-aware binary units without compact decimal words", () => {
  const bytes = 19_757_000_000
  assert.equal(humanBytes(bytes, "en"), "18.4 GiB")
  assert.equal(humanBytes(bytes, "ru"), "18,4 GiB")
  assert.doesNotMatch(humanBytes(bytes, "ru"), /млрд Б/)
  assert.equal(identifier(String(bytes)), "19757000000")
})

test("a fitted column takes the widest cell within bounds", () => {
  assert.equal(fittedWidth(200), 212)
  assert.equal(fittedWidth(0), 64)
  assert.equal(fittedWidth(4000), 720)
  assert.equal(fittedWidth(120.2), 133)
})

test("metric numbers use three significant digits and locale-aware compact scales", () => {
  assert.equal(compact(0, "en"), "0")
  assert.equal(compact(0.03, "en"), "0.03")
  assert.equal(compact(9.876, "en"), "9.88")
  assert.equal(compact(999, "en"), "999")
  assert.equal(compact(1407.48, "en"), "1.41K")
  assert.equal(compact(9999, "en"), "10K")
  assert.equal(compact(21_471, "en"), "21.5K")
  assert.equal(compact(3_052_945.27, "en"), "3.05M")
  assert.equal(compact(452_000_000, "en"), "452M")
  assert.equal(compact(-21_471, "en"), "-21.5K")
  assert.equal(compact(4.5e12, "en"), "4.5T")
  assert.equal(compact(Number.MAX_VALUE, "en"), "1.8E308")
  assert.equal(compact(Number.NaN, "en"), "—")
  assert.equal(compact(Number.POSITIVE_INFINITY, "en"), "—")
  assert.equal(compact(1407.48, "ru"), "1,41 тыс.")
  assert.equal(compact(8_117_857, "ru"), "8,12 млн")
  assert.equal(compact(1_360_000_000, "ru"), "1,36 млрд")
  assert.equal(measure(1407.48, "en", " ms/s"), "1.41K ms/s")
  assert.equal(compact(4e-7, "en"), "4E-7")
  assert.equal(humanPercent(41.729068244136855, "en"), "41.7%")
  assert.equal(humanPercent(41.729068244136855, "ru"), "41,7 %")
  assert.equal(humanPercent(0, "en"), "0%")
  assert.equal(humanPercent(0.099, "en"), "<0.1%")
  assert.equal(humanPercent(0.099, "ru"), "<0,1 %")
  assert.equal(humanPercent(0.1, "en"), "0.1%")
  assert.equal(humanPercent(12, "en"), "12%")
  assert.equal(humanPercent(12.04, "en"), "12%")
  assert.equal(humanPercent(12.05, "en"), "12.1%")
  assert.equal(humanPercent(null, "en"), "—")
  assert.equal(humanPercent(4e-7, "en"), "<0.1%")
  assert.equal(humanDuration(999.999, "en"), "1,000 ms")
  assert.equal(humanDuration(1_234, "en"), "1.23 s")
  assert.equal(humanDuration(0.025, "en"), "25 µs")
  assert.equal(humanDuration(16, "en", "microseconds", "/call"), "16 µs/call")
  assert.equal(humanDuration(3_600, "en", "seconds"), "1 h")
  assert.equal(humanDuration(null, "en"), "—")
  // Axis ticks share one unit chosen from the range top.
  assert.equal(humanDurationAxis(1_998_000, 3_996_000, "ru"), "0,555 ч")
  assert.equal(humanDurationAxis(0, 3_996_000, "ru"), "0 ч")
  assert.equal(humanDurationAxis(5_400_000, 7_200_000, "en"), "1.5 h")
  assert.equal(humanDurationAxis(1_500, 45_000, "en"), "1.5 s")
  assert.equal(identifier("9007199254740993"), "9007199254740993")
})

function rowTranslator(locale: Locale) {
  const copy = locale === "ru"
    ? { one: "≈{value} строка", few: "≈{value} строки", many: "≈{value} строк" }
    : { one: "≈{value} row", few: "≈{value} rows", many: "≈{value} rows" }
  return (key: string, slots: Readonly<Record<string, string | number>> = {}) => copy[key.slice(key.lastIndexOf(".") + 1) as keyof typeof copy].replace("{value}", String(slots.value))
}

test("estimated row gauges keep compact and exact bigint labels", () => {
  assert.deepEqual(estimatedRows(713_456, "en", rowTranslator("en")), { primary: "≈713K rows", secondary: "≈713,456 rows" })
  assert.deepEqual(estimatedRows(12_876, "ru", rowTranslator("ru")), { primary: "≈12,9 тыс. строк", secondary: "≈12 876 строк" })
  assert.equal(estimatedRows(999, "en", rowTranslator("en"))?.primary, "≈999 rows")
  assert.equal(estimatedRows(1_000, "en", rowTranslator("en"))?.primary, "≈1K rows")
  assert.equal(estimatedRows(999_499, "en", rowTranslator("en"))?.primary, "≈999K rows")
  assert.equal(estimatedRows(999_500, "en", rowTranslator("en"))?.primary, "≈1M rows")
  assert.equal(estimatedRows("9994999999999999", "en", rowTranslator("en"))?.primary, "≈9.99E15 rows")
  assert.equal(estimatedRows("9007199254740993", "en", rowTranslator("en"))?.secondary, "≈9,007,199,254,740,993 rows")
  assert.equal(estimatedRows(null, "ru", rowTranslator("ru")), null)
})

test("estimated row exact labels use bigint-safe EN and RU plurals", () => {
  for (const [value, suffix] of [[0, "rows"], [1, "row"], [2, "rows"]] as const) {
    assert.equal(estimatedRows(value, "en", rowTranslator("en"))?.secondary, `≈${value} ${suffix}`)
  }
  for (const [value, suffix] of [[11, "строк"], [12, "строк"], [14, "строк"], [21, "строка"], [22, "строки"], [25, "строк"]] as const) {
    assert.equal(estimatedRows(value, "ru", rowTranslator("ru"))?.secondary, `≈${value} ${suffix}`)
  }
})

test("elapsed wall time uses the shared adaptive duration scale", () => {
  assert.equal(humanAge(0, "ru"), "0 с")
  assert.equal(humanAge(0.94, "ru"), "0 с")
  assert.equal(humanAge(12.7, "ru"), "12 с")
  assert.equal(humanAge(12.7, "en"), "12 s")
  assert.equal(humanAge(95, "ru"), "1,58 мин")
})

test("the TIME column follows the ps ladder from MM:SS to DD-HH:MM:SS", () => {
  assert.equal(processCpuTime(0), "0:00")
  assert.equal(processCpuTime(276.18), "4:36")
  assert.equal(processCpuTime(59.9), "0:59")
  assert.equal(processCpuTime(3_600), "1:00:00")
  assert.equal(processCpuTime(44_819), "12:26:59")
  assert.equal(processCpuTime(97_230), "1-03:00:30")
  assert.equal(processCpuTime(null), "—")
  assert.equal(processCpuTime(-1), "—")
})

test("the TTY column names the recorded device the way ps does", () => {
  assert.equal(processTty(0), "?")
  assert.equal(processTty(null), "?")
  assert.equal(processTty(34_816), "pts/0")
  assert.equal(processTty(34_819), "pts/3")
  assert.equal(processTty(1_025), "tty1")
  assert.equal(processTty(1_088), "ttyS0")
  assert.equal(processTty(2_305), "9:1")
})
