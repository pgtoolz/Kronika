# Process user-reference measurements

[Русская версия](process-user-measurements.ru.md)

The `os_user` section maps user IDs (UIDs) to names; `dict.strings` stores the name strings. These measurements show their disk space and recording cost on Linux 6.17.10-100.fc41.x86_64, AMD Ryzen 9 8945HS, using an optimized build and a 100 Hz process CPU-time counter. The measurement date, source revision, compiler version, repeat count and result variation were not recorded.

Current reproduction, using the repository toolchain:

```bash
CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu cargo test --release -p kronika-collector process_user_references_report_production_storage_and_resource_costs -- --nocapture --test-threads=1
```

The test runs the three cases sequentially in one isolated child test process.
Artifacts contain user references and their string dictionary; process rows are
absent. Source: [`user_cost_artifact` and the measurement test](../bins/kronika-collector/src/tests/zms.rs).

| Case | UID observations | Mapping rows | Raw `os_user` body | Raw dictionary body | Raw WAL | Finished `os_user` | Finished dictionary | Marginal finished bytes | Whole ZMS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 processes sharing one UID for 120 ticks | 120,000 | 1 | 1,171 B | 463 B | 1,794 B | 517 B | 245 B | 826 B | 870 B |
| Several ordinary UIDs | 16 | 16 | 1,343 B | 648 B | 2,151 B | 694 B | 428 B | 1,186 B | 1,230 B |
| Maximum distinct observed UIDs and names | 4,096 | 4,096 | 55,368 B | 43,684 B | 99,212 B | 42,414 B | 43,219 B | 85,697 B | 85,741 B |

Additional finished bytes equal the finished user-reference body plus the finished string dictionary plus two catalog entries: `Marginal finished bytes = finished os_user body + finished dictionary body + 2 × catalog_entry_bytes`. One catalog entry is `catalog_entry_bytes = 32 B`. For the shared UID, `517 + 245 + 2 × 32 = 826 B`; the whole ZMS is `870 B`. The 120,000 UID observations produce one mapping row. WAL and whole-ZMS sizes also include file headers and metadata.

| Measurement | Shared UID | 16 UIDs | 4,096 UIDs |
|---|---:|---:|---:|
| Capture elapsed, µs | 131 | 5 | 1,240 |
| Writer elapsed, µs | 9,248 | 7,017 | 10,630 |
| Test-process peak RSS, KiB | 15,832 | 17,048 | 20,496 |
| Capture increase in peak RSS, KiB | 128 | 0 | 0 |

Capture elapsed is the sum of `Instant::elapsed().as_micros()` around
`prepare_rows` for each sample. Writer elapsed sums encoding and WAL append,
plus final segment writing. Reader/dictionary validation runs after this timed
writer block. The 100 Hz clock describes the separate process CPU-time counter;
it does not set the resolution of `Instant` elapsed measurements.

RSS is the process memory resident in physical RAM. The peak here belongs to the whole test process, including the test harness, memory kept by the allocator after earlier cases, and the Parquet writer. The design budget of 25,600 KiB is a target for collector RSS, not an enforced runtime memory limit.

The test asserts one row per recorded UID, no row for an unresolved UID, and
rejection of an oversized passwd source. A malformed passwd line can coexist
with a retained valid line. Separate recovery and forced-rollover tests check
segment-local mappings and dictionary resolution. `/etc/passwd` is the recorded
name source; identities available only through NSS, LDAP, or SSSD remain numeric.
A UID mapping first recorded in a segment stays fixed for that segment.
