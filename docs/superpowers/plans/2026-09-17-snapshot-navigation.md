# Snapshot navigation implementation plan

**Goal:** Move between recorded samples of the open screen without depending on collector intervals or stepping through unrelated OS/PostgreSQL timestamps.

**Design:** Add a small neighbor query to the shared query engine. Web asks for the next or previous timestamp, then loads the existing snapshot. Navigation responses are never stored in the browser cache; existing finished-snapshot caching stays unchanged. Embedded reports execute the same query locally.

**Approved contract:** `GET /api/snapshot/neighbor`, with repeated `section`, required `at` and `direction=next|previous`, and optional inclusive `from`/`to`. Select the nearest recorded timestamp at least one second in the requested direction. Return one NDJSON `snapshot_neighbor` record with paired nullable decimal-string `at` and `segment_id`. No collector configuration, value comparison, or second-by-second probing participates.

**Integration finding:** Overlapping segments can contain newer samples in an older segment. Ordinary screen snapshots must select the latest eligible observation across their captured segments. Expose this as `selection=latest`; retain `selection=anchor` as the default for existing API consumers and exact-row requests. The distinct URL also prevents a previously cached immutable snapshot from masking the corrected result. Keep pagination and validators bound to the selection policy.

## Shared query and route

- [x] Add failing query and parser tests for sparse samples, subsecond neighbors, both directions, bounds, layouts, and overlapping segments.
- [x] Implement `SnapshotNeighborRequest` and portable query execution. Inspect timestamp columns only, prune segments by inventory and time bounds, and retain cancellation.
- [x] Mark all navigation results mutable, including an absent neighbor in a finished segment.
- [x] Run query/API tests and strict Clippy.

## Browser navigation

- [x] Add failing API/state/browser tests with different data at consecutive Activity timestamps.
- [x] Map the open screen to its recorded sections and use the neighbor query for keyboard and mobile steps.
- [x] Respect the supplied navigation domain; do not merge unrelated timeline lanes back into it.
- [x] Cancel obsolete requests, refresh a stale catalog when the result names a new segment, and keep URL/cursor/table transitions coherent.
- [x] Retry an absent neighbor on a later click; preserve live-hour refresh and fixed manually selected cursors.
- [x] Run UI tests, type checking, and production-artifact browser regressions.

## Cache and native/report integration

- [x] Add native HTTP regression coverage for no-store with conditional requests, absent-then-appended neighbors, publication, and unchanged immutable snapshot caching.
- [x] Include neighbor requests in native/WASM parity checks.
- [x] Update time-navigation documentation in English and Russian.
- [x] Rebuild embedded UI/report/WASM artifacts with the canonical toolchains and verify reproducibility.
- [x] Review the combined change and run affected checks.

Publication and CI results are tracked in [PR #9](https://github.com/pgtoolz/Kronika/pull/9).
