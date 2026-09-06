import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { gunzipSync } from "node:zlib";
import { resolve } from "node:path";
import { runInThisContext } from "node:vm";

const SEGMENT_ID = "1709164800000000";
const SAMPLE_TO = "1709164801000000";
const SOURCES = 3;
const [glueArgument, wasmArgument, nativeArgument] = process.argv.slice(2);
assert.ok(glueArgument, "generated WebAssembly glue path is required");
assert.ok(wasmArgument, "compressed WebAssembly path is required");
assert.ok(nativeArgument, "native oracle path is required");

const gluePath = resolve(glueArgument);
const wasmPath = resolve(wasmArgument);
const nativePath = resolve(nativeArgument);
const fixtureRoot = new URL("../../../bins/kronika-report/tests/fixtures/", import.meta.url);
const [glue, wasmGzip, zms, idx] = await Promise.all([
  readFile(gluePath, "utf8"),
  readFile(wasmPath),
  readFile(new URL("standalone.zms", fixtureRoot)),
  readFile(new URL("standalone.idx", fixtureRoot)),
]);
runInThisContext(glue, { filename: gluePath });
const bindings = globalThis.KronikaReportWasm;
assert.ok(bindings, "browser bindings must expose KronikaReportWasm");
const wasmBytes = gunzipSync(wasmGzip);
const module = await WebAssembly.compile(wasmBytes);
const wasm = await bindings.initEmbedded(module);
const memoryBeforeBytes = wasm.memory.buffer.byteLength;
const session = new bindings.ReportSession(
  SEGMENT_ID,
  zms,
  idx,
  SOURCES,
  BigInt(zms.length),
);
let cases = 0;
let outputBytes = 0;

function nativeBody(path, query) {
  const result = spawnSync(nativePath, [path, query], {
    encoding: null,
    maxBuffer: 16 * 1024 * 1024,
  });
  assert.ifError(result.error);
  assert.equal(
    result.status,
    0,
    `native request failed: ${result.stderr?.toString("utf8") ?? "missing stderr"}`,
  );
  return result.stdout;
}

function wasmBody(path, query) {
  const response = session.request(path, query);
  try {
    assert.equal(response.status, 200);
    assert.equal(response.code, undefined);
    assert.equal(response.parameter, undefined);
    assert.equal(response.message, undefined);
    return Buffer.from(response.takeBody());
  } finally {
    response.free();
  }
}

function compare(name, path, query) {
  const native = nativeBody(path, query);
  const browser = wasmBody(path, query);
  assert.deepEqual(browser, native, `${name} bytes differ`);
  cases += 1;
  outputBytes += browser.length;
  return browser;
}

function records(body) {
  return body
    .toString("utf8")
    .trimEnd()
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

compare("catalog", "/api/catalog", "");
compare(
  "index",
  `/api/segments/${SEGMENT_ID}/sections/pg_stat_database/index`,
  "",
);
compare(
  "hour",
  "/api/hour",
  `from=${SEGMENT_ID}&to=${SAMPLE_TO}&part=base`,
);
compare(
  "snapshot-large-text-limit",
  `/api/segments/${SEGMENT_ID}/snapshot`,
  `at=${SAMPLE_TO}&section=os_cpu&field=user&page_size=1&text=5000000000`,
);

const rowsPath = `/api/segments/${SEGMENT_ID}/sections/os_process/rows`;
const firstQuery = "field=comm&field=utime&order=asc&page_size=1";
const firstPage = compare("rows-first-page", rowsPath, firstQuery);
const nextCursor = records(firstPage).find((record) => record.record === "page")
  ?.next_cursor;
assert.equal(typeof nextCursor, "string", "first page must carry a cursor");
compare(
  "rows-next-page",
  rowsPath,
  `${firstQuery}&cursor=${encodeURIComponent(nextCursor)}`,
);

const events = compare(
  "detail-source",
  "/api/events",
  `from=${SEGMENT_ID}&to=1709164801000001&representation=occurrences&limit=5&source=pg_log_errors`,
);
const detailRef = records(events).find(
  (record) => record.record === "event_occurrence",
)?.detail_ref;
assert.equal(typeof detailRef, "string", "event occurrence must carry a detail ref");
compare(
  "row-detail",
  "/api/row-detail",
  `detail_ref=${encodeURIComponent(detailRef)}`,
);

const memoryAfterBytes = wasm.memory.buffer.byteLength;
session.free();
process.stdout.write(`${JSON.stringify({
  cases,
  zmsBytes: zms.length,
  idxBytes: idx.length,
  wasmBytes: wasmBytes.length,
  wasmGzipBytes: wasmGzip.length,
  outputBytes,
  memoryBeforeBytes,
  memoryAfterBytes,
})}\n`);
