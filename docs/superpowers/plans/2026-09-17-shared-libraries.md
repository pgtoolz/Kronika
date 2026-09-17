# Shared libraries implementation plan

> For agentic workers: use subagent-driven-development; each extraction needs a focused review and existing contract tests before integration.

**Goal:** Move reusable report, slice, PostgreSQL and OS operations out of application packages while preserving their behavior.

**Architecture:** Applications own CLI configuration, scheduling, transport and persistence policy. Source libraries own acquisition and row conversion. Report and slice libraries have existing consumers and become independent workspace packages.

**Tech stack:** Rust 1.96, Cargo workspace, Tokio/PostgreSQL, Arrow/Parquet, native Linux and wasm32.

**Spec:** The architecture audit accepted in the conversation on 2026-09-17: extract report/slice and PostgreSQL acquisition first, then OS acquisition; consolidate existing duplicates. MCP extraction and native query adapter relocation are deferred.

## Constraints

- Work on the existing `refactoring` branch; retain CLI names, flags, environment precedence and output contracts.
- Preserve section layouts, encoded data, query responses, nullable fields, scopes and row/dictionary ordering.
- Keep CLI, Hyper, Tokio and native filesystem dependencies out of portable report/query builds.
- Source libraries must not depend on collector configuration, scheduler, writer or logfmt output.
- Preserve bounded batches and synchronous admission before reading the next batch, including connection cleanup after rejection.
- Preserve connection generation invalidation, extension fallback order, timeouts and credential-safe diagnostics.
- Preserve caller-owned scratch/output, publication without overwrite and active-data rollover handling.
- Move unit tests with their implementation into the owning crate's `src/tests`; retain integration tests at the consumer boundary.
- Do not add a general-purpose common/runtime crate or change metric collection intervals.

## Task 1: Report and slice libraries

Files: `bins/kronika-report/{Cargo.toml,src,tests}`, `bins/kronika-dump/{Cargo.toml,src,tests}`, new `crates/kronika-report` and `crates/kronika-slice`, their consumers, workspace manifest and report build scripts.

- [x] Move `ReportEngine`, `ReportInput`, errors and HTML generation into a library package; keep the report executable's name and command behavior.
- [x] Move `SliceRange`, `UtcSecond`, errors, selection/staging and `slice_to_zms` into a native library; keep dump inspection and destination publication in its binary.
- [x] Update web/WASM/example imports, package selection in build scripts and portable boundary checks. Keep report generator features separate from its portable core.
- [x] Move library tests and fixture ownership; verify report, slice, dump and web tests and native/WASM response parity.

## Task 2: PostgreSQL acquisition

Files: `crates/kronika-source-pg/src`, `bins/kronika-collector/src/pg_sources*`, `log_sources/settings.rs`, and their owning tests.

- [x] Move database/extension discovery, probe, caches, bounded collection, batch types and observations into source-pg. Accept source-specific selection and connection inputs; translate collector `DueSet` at one boundary.
- [x] Keep writer conversion/admission, schedules and diagnostics in collector. Reuse the existing batch callback contract rather than adding a writer dependency.
- [x] Share credential-safe endpoint labels and compatible connection setup. Move PostgreSQL/PgBouncer log fact queries into source-pg; preserve their distinct protocols and validation rules.
- [x] Verify generation/cache transitions, source fallback, decode errors, cancellation, permission failures, settings retention and credential redaction using the existing tests.

## Task 3: OS acquisition and conversion

Files: `crates/kronika-source-os/src`, `bins/kronika-collector/src/os_sources*`, `cgroup_discovery/buffering.rs`, and their owning tests.

- [x] Move topology/NUMA enrichment, interface facts, mount filtering/device resolution and CPUFreq/cgroup row conversion into source-os.
- [x] Use existing row types and explicit interning callbacks; keep segment interner, user-name durability, buffering, cadence and statvfs child lifecycle in collector.
- [x] Preserve partial failures, missing values, namespace scope, device attribution and cgroup identity checks.
- [x] Move tests with domain behavior and verify OS source tests plus collector OS/cgroup/segment integration tests.

## Task 4: Integration and verification

Files: `crates/kronika-api`, `crates/kronika-report-wasm`, web error mapping, README/build documentation, dependency boundary scripts, lockfile.

- [x] Share the existing query-error status classification through portable API code; retain web headers and WASM response envelopes.
- [x] Remove stale package/path references; document the new library responsibilities and examples without changing launch commands.
- [x] Review each extraction independently, then review the combined dependency graph and application adapters.
- [x] Run formatting, strict workspace Clippy, affected and workspace tests, portable feature checks, canonical report asset reproducibility and native/WASM parity.
- [ ] Commit and push to `refactoring`, update PR #9, and wait for native Linux tests, BDD/demo, custom lints and release checks.

## Evidence

The baseline is commit `4e4ed91`, with all 30 PR checks passing. Existing fixture/protocol tests are the primary behavior oracle; new tests are needed only where consolidation exposes a previously untested contract. Native CI is authoritative for PSI and CPU/RSS budgets unavailable under local Docker Desktop emulation.

Local verification after extraction: 1,895 workspace tests outside collector/BDD,
175 collector release tests and 10 BDD unit tests passed. One provisioned TLS
integration test remains explicitly ignored. All 2,072 pre-existing Rust test
functions retain an owner; ten acquisition boundary tests were added. Formatting,
strict workspace Clippy, dependency boundaries, canonical report reproducibility,
and all 104 native/WASM parity cases passed. Native PR checks run after push.
