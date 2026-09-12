import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { gunzipSync } from "node:zlib";
import { resolve } from "node:path";
import { runInThisContext } from "node:vm";

const SEGMENT_ID = "1709164800000000";
const SAMPLE_TO = "1709164801000000";
const SOURCES = 3;
const [glueArgument, wasmArgument, nativeArgument, collectionArgument] = process.argv.slice(2);
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
let session = new bindings.ReportSession(
  SEGMENT_ID,
  zms,
  idx,
  SOURCES,
  BigInt(zms.length),
);
let nativeFixtureDirectory = null;
let cases = 0;
let outputBytes = 0;

function nativeBody(path, query) {
  const result = spawnSync(nativePath, [path, query, ...(nativeFixtureDirectory === null ? [] : [nativeFixtureDirectory])], {
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


if (collectionArgument) {
  for (const name of ["postgresql-unknown", "postgresql-explicit", "selected-cgroup", "separated-controllers", "all-cgroups", "all-cgroups-machine", "legacy-cgroup"]) {
    session.free();
    nativeFixtureDirectory = resolve(collectionArgument, name);
    const [fixtureZms, fixtureIdx, sourceText, html] = await Promise.all([
      readFile(resolve(nativeFixtureDirectory, "recording.zms")),
      readFile(resolve(nativeFixtureDirectory, "recording.idx")),
      readFile(resolve(nativeFixtureDirectory, "sources"), "utf8"),
      readFile(resolve(nativeFixtureDirectory, "report.html"), "utf8"),
    ]);
    const embedded = /const z=b\("([A-Za-z0-9+/=]+)"\),i=b\("([A-Za-z0-9+/=]+)"\)/.exec(html);
    assert.ok(embedded, `${name}: generated report must embed both artifacts`);
    const reportZms = Buffer.from(embedded[1], "base64");
    const reportIdx = Buffer.from(embedded[2], "base64");
    assert.deepEqual(reportZms, fixtureZms);
    assert.deepEqual(reportIdx, fixtureIdx);
    session = new bindings.ReportSession(SEGMENT_ID, reportZms, reportIdx, Number(sourceText), BigInt(reportZms.length));
    const catalog = records(compare(`${name}-catalog`, "/api/catalog", ""));
    const families = catalog.find(row => row.record === "catalog").source_families;
    const pgOnly = name.startsWith("postgresql-");
    assert.equal(families.find(row => row.name === "os").configured, !pgOnly);
    assert.equal(families.find(row => row.name === "postgresql").configured, pgOnly);
    const hour = records(compare(`${name}-hour`, "/api/hour", `from=${SEGMENT_ID}&to=1709164805000000`));
    const lanes = records(compare(`${name}-lanes`, "/api/hour", `from=${SEGMENT_ID}&to=1709164805000000&part=lanes&segments=${SEGMENT_ID}`));
    const context = lanes.find(row => row.record === "lane_context");
    assert.equal(context.os_enabled, !pgOnly);
    assert.equal(context.postgresql_processes_shared, false);
    if (pgOnly) {
      assert.equal(families.find(row => row.name === "os").present, false);
      assert.equal(hour.some(row => row.record === "point" && row.series === "os_health"), false);
      const expected = name === "postgresql-explicit" ? 80 : null;
      for (const series of ["postgres_health", "overall_health"]) {
        assert.deepEqual(hour.filter(row => row.record === "point" && row.series === series).map(row => row.value), [expected]);
      }
      const processes = records(compare(`${name}-no-process-association`, "/api/hour", `from=${SEGMENT_ID}&to=1709164805000000&section=os_process_summary`));
      assert.equal(processes.some(row => row.record === "row"), false);
      const metadata = records(compare(`${name}-metadata`, `/api/segments/${SEGMENT_ID}/sections/instance_metadata/rows`, "field=hostname&field=os_enabled&field=postgresql_processes_shared"));
      const row = metadata.find(row => row.record === "row");
      assert.deepEqual(row.values, [null, false, false]);
    } else {
      if (name === "legacy-cgroup") {
        const path = `/api/segments/${SEGMENT_ID}/snapshot`;
        const query = new URLSearchParams({ at: "1709164805000000", section: "os_cgroup_v2_cpu", page_size: "20" });
        for (const field of ["cgroup_path", "cgroup_identity", "usage_usec", "quota_cores"]) query.append("field", field);
        const selected = records(compare(`${name}-selected-CPU`, path, query.toString())).filter(row => row.record === "row");
        assert.equal(selected.length, 1);
        assert.equal(selected[0].type_id, "1201003");
        assert.deepEqual(selected[0].values, ["/legacy-selected", "directory:selected", 1_000_000, 2]);
        for (const prefix of ["", "text:", "q:", "path:"]) {
          query.set("search", `${prefix}legacy-selected`);
          const matching = records(compare(`${name}-${prefix || "plain"}-selected-search`, path, query.toString())).filter(row => row.record === "row");
          assert.deepEqual(matching, selected, "search aliases retain the same physical legacy row");
          query.set("search", `${prefix}legacy-earlier`);
          const excluded = records(compare(`${name}-${prefix || "plain"}-excluded-selected`, path, query.toString())).filter(row => row.record === "row");
          assert.equal(excluded.length, 0, "search does not revive an earlier legacy observation");
        }
        const contextQuery = new URLSearchParams({ at: "1709164805000000", section: "os_cgroup_context", "where.cpu_path": "/legacy-selected", "where.cpu_identity": "directory:selected", "where.scope": "4" });
        for (const field of ["cpuset_cpus", "cpu_path", "cpu_identity"]) contextQuery.append("field", field);
        const selectedContext = records(compare(`${name}-selected-cpuset`, path, contextQuery.toString())).filter(row => row.record === "row");
        assert.equal(selectedContext.length, 1);
        assert.deepEqual(selectedContext[0].values, ["8", "/legacy-selected", "directory:selected"]);
        continue;
      }
      if (name.startsWith("all-cgroups")) {
        const at = "1709164805000000";
        for (const [resource, field] of [["cpu", "usage_usec"], ["memory", "current"], ["pids", "current"], ["io", "rbytes"]]) {
          const path = `/api/segments/${SEGMENT_ID}/snapshot`;
          const query = new URLSearchParams({ at, section: `os_cgroup_v2_${resource}`, by: field, direction: "desc", page_size: "20" });
          query.append("field", "cgroup_path");
          query.append("field", field);
          query.append("field", "cgroup_identity");
          if (resource === "io") {
            query.append("field", "major");
            query.append("field", "minor");
          } else if (resource === "pids") {
            query.append("field", "events_source");
          }
          const first = records(compare(`${name}-${resource}-first-page`, path, query.toString()));
          const firstRows = first.filter(row => row.record === "row");
          assert.equal(firstRows.length, 20);
          const identity = row => JSON.stringify([row.type_id, row.values[0], ...row.values.slice(2)]);
          const firstIdentities = new Set(firstRows.map(identity));
          assert.equal(firstIdentities.size, 20, `${resource}: no duplicate identities in first page`);
          if (resource === "cpu" || resource === "io") {
            assert.equal(firstRows[0].values[0], "/visible/jobs/worker-258", "rate ordering differs from lifetime-counter ordering");
            assert.equal(Number(firstRows[0].values[1]), resource === "cpu" ? 2_630_000 : 4_688_183_296);
          }
          const cursor = first.find(row => row.record === "snapshot_page")?.next_cursor;
          assert.equal(typeof cursor, "string", `${resource}: populated table has another page`);
          query.set("cursor", cursor);
          const secondRows = records(compare(`${name}-${resource}-next-page`, path, query.toString())).filter(row => row.record === "row");
          assert.equal(secondRows.length, 20);
          const secondIdentities = new Set(secondRows.map(identity));
          assert.equal(secondIdentities.size, 20, `${resource}: no duplicate identities in second page`);
          assert.ok(secondRows.every(row => !firstIdentities.has(identity(row))), `${resource}: pages have disjoint exact identities`);
          query.delete("cursor");
          const searchedPath = "/visible/jobs/worker-010";
          assert.ok(firstRows.every(row => row.values[0] !== searchedPath), `${resource}: search target was not on first page`);
          query.set("search", `path:"${searchedPath}"`);
          const found = records(compare(`${name}-${resource}-path-search`, path, query.toString())).filter(row => row.record === "row");
          assert.equal(found.length, resource === "io" ? 2 : 1);
          assert.ok(found.every(row => row.values[0] === searchedPath));
          assert.equal(new Set(found.map(identity)).size, found.length);
          for (const prefix of ["", "text:", "q:"]) {
            query.set("search", `${prefix}worker-010`);
            const matching = records(compare(`${name}-${resource}-${prefix || "plain"}-search`, path, query.toString())).filter(row => row.record === "row");
            assert.deepEqual(matching, found, `${resource}: plain, text, q and path searches return the same full-dataset rows`);
          }
          if (resource === "io") {
            assert.deepEqual(new Set(found.map(row => JSON.stringify(row.values.slice(3).map(Number)))), new Set(["[8,0]", "[8,16]"]));
          }
        }
        const partial = records(compare(`${name}-partial-observation`, `/api/segments/${SEGMENT_ID}/snapshot`, "at=1709164803000000&section=os_cgroup_v2_cpu&field=cgroup_path"));
        assert.equal(partial.filter(row => row.record === "row").length, 262);
        const selectedHistory = new URLSearchParams({ from: String(SEGMENT_ID), to: at, section: "os_cgroup_v2_cpu", type_id: "1207001", field: "usage_usec", "where.cgroup_path": "/visible/jobs/worker-258", "where.cgroup_identity": "directory:group-262" });
        const historyRows = records(compare(`${name}-selected-history-boundary`, "/api/hour", selectedHistory.toString())).filter(row => row.record === "row");
        assert.equal(historyRows.length, 5);
        assert.equal(historyRows.find(row => row.timestamp === "1709164804000000")?.break_before, true, "missing predecessor is retained through the native/WASM history transport");
        assert.equal(historyRows.find(row => row.timestamp === "1709164805000000")?.break_before, undefined, "the next valid interval remains continuous");
      }
      if (name === "selected-cgroup") {
        for (const section of ["group", "cpu", "memory", "pids", "io"]) {
          const rows = records(compare(`discovered-${section}`, `/api/segments/${SEGMENT_ID}/sections/os_cgroup_v2_${section}/rows`, "")).filter(row => row.record === "row");
          assert.equal(rows.length, 2, `${section}: both recorded observations survive native/WASM/report decoding`);
        }
      }
      if (name === "separated-controllers") {
        const oom = lanes.filter(row => row.record === "lane" && row.lane === "cg_oom");
        assert.deepEqual(oom.map(row => row.value), [null, 0, 0, 0, 1, null], "memory continuity survives a CPU identity change but ends on memory replacement");
      }
      const shares = hour.filter(row => row.record === "lane" && row.lane === "cg_cpu_share");
      if (name === "all-cgroups-machine") {
        assert.equal(context.environment, 0);
        assert.deepEqual(shares, [], "machine fixture does not acquire a selected-container overview");
        const old = records(compare(`${name}-no-primary-companion`, "/api/hour", `from=${SEGMENT_ID}&to=1709164805000000&section=os_cgroup_cpu`));
        assert.equal(old.some(row => row.record === "row"), false);
        continue;
      }
      assert.deepEqual(shares.map(row => row.value), [null, 100, 75, null, null, 50]);
      const history = records(compare(`${name}-counter-history`, "/api/hour", `from=${SEGMENT_ID}&to=1709164805000000&section=os_cgroup_cpu&field=usage_usec`));
      const reset = history.find(row => row.record === "row" && row.timestamp === "1709164804000000");
      assert.ok(reset, "replacement sample is retained");
      assert.equal(reset.values[0], "100000000", "raw replacement counter is retained");
      const before = history.find(row => row.record === "row" && row.timestamp === "1709164803000000");
      const after = history.find(row => row.record === "row" && row.timestamp === "1709164805000000");
      assert.ok(before && after);
      assert.notDeepEqual(reset.identity, before.identity, "history exposes the replacement identity");
      assert.deepEqual(reset.identity, after.identity, "subsequent sample retains the new identity");
    }
  }
}

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
