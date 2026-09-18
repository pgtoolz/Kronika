# Review fixes implementation plan

**Goal:** Restore native API features and reliable refresh, and make heatmap boundary values independent of segment packaging.

**Approved scope:** Fix the four regressions reviewed in `0c3540b..2e5417a`. Heatmaps retain their existing column layout and midpoint allocation. A hard 15-minute limit applies to neighboring-sample lookup and consecutive counter sample gaps, regardless of collector configuration. Exactly 15 minutes is allowed; a longer gap contributes no rate. Ranking totals and gauges retain their existing semantics.

## Native API cache metadata

- [x] Reproduce build-suffixed requests failing native route validation.
- [x] Handle cache metadata consistently for export, MCP, instance labels, and shared routes while rejecting invalid request parameters.
- [x] Run route/HTTP regressions and relevant API tests.

## Refresh lifecycle

- [x] Reproduce a progressing refresh being repeatedly cancelled after 30 seconds.
- [x] Preserve periodic polling and stalled-request recovery without cancelling requests that continue receiving data.
- [x] Cover slow success, inactivity, cancellation, and retry through production behavior.

## Heatmap boundaries

- [x] Reproduce segment-packaging and overlapping-baseline failures.
- [x] Enforce the 15-minute bound at both segment and row selection, and select nearest edge samples across overlapping segments.
- [x] Suppress counter intervals longer than 15 minutes, including internal gaps; resume when a valid pair becomes available.
- [x] Test exact and exceeded limits, groups, Total/Other, ranking compatibility, and native/report parity.

## Integration

- [x] Update the English and Russian metric-time reference.
- [x] Run affected Rust/UI suites, strict lint, and focused browser regressions.
- [x] Rebuild and verify embedded UI and WASM assets with the canonical toolchains.
- [x] Review the combined diff, then commit and push to the existing branch.

## Verification

Integrated remote commit `5e2e135` before final verification. Native API handlers now accept build metadata, so the temporary UI exceptions for MCP access and instance labels are unnecessary.

- Affected Rust suites: 691 tests passed; strict Clippy passed.
- UI: 568 tests, typecheck, and the production refresh browser regression passed.
- Native/WASM parity: 111 cases passed, including heatmap edge samples and ranking totals.
- Embedded UI, report shell, and WASM were rebuilt and their reproducibility checks passed.
- Neighbor-only empty heatmaps remain revalidated; old heatmap ETags are invalidated by the representation version.
