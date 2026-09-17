# Libraries

[Русская версия](README.ru.md)

The workspace libraries own acquisition, storage and recorded-data operations.
The packages in [`bins`](../bins) choose when to run those operations and how to
present their results. Libraries do not depend on application packages.

| Libraries | Responsibility |
| --- | --- |
| `kronika-source-os`, `kronika-source-pg`, `kronika-source-log` | Read sources, decode observations and convert supported rows. PostgreSQL acquisition also owns database/extension discovery and connection caches. |
| `kronika-format`, `kronika-registry`, `kronika-derive` | Define the recording format, section schemas and codecs. |
| `kronika-layout`, `kronika-store`, `kronika-reader`, `kronika-writer` | Locate storage, read captured data, intern strings, append journals and write finished segments. |
| `kronika-index`, `kronika-query`, `kronika-api` | Build derived indexes, execute queries and parse recorded-data requests. The API error status is shared by HTTP and embedded reports. |
| [`kronika-slice`](kronika-slice) | Extract a selected interval into one standalone ZMS. Used by dump and web export. |
| [`kronika-report`](kronika-report), `kronika-report-wasm` | Compose the report query engine, generate HTML and expose its browser adapter. Used by report CLI and web export. |
| `kronika-bdd` | Run end-to-end scenarios against the executables. |

## Source acquisition and persistence

Collector owns schedules, signals, retention, segment state and WAL admission.
It passes source-specific inputs to the acquisition libraries. PostgreSQL emits
bounded batches through a callback and waits for admission before reading more
rows. OS row conversion accepts an interning callback; the segment dictionary
and buffer remain owned by collector. Source libraries do not depend on writer
or collector configuration.

## Native and embedded queries

Web owns HTTP authentication, headers, streaming and MCP transport. Query
execution is shared with the embedded report. Portable builds disable the
native features of query/report dependencies; HTML generation is the report
library's `generator` feature. Slice extraction requires native storage and is
not part of the WASM graph.

The Cargo package for the report executable is `kronika-report-cli`; its binary
name remains `kronika-report`. The package `kronika-report` is the library.

[`check-query-boundary.sh`](../scripts/check-query-boundary.sh) checks the portable
dependency graph and prevents library dependencies on application packages.
Unit tests live under the owning crate's `src/tests`; integration tests stay
under `tests`.
