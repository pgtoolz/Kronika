# kronika-report

Shared report queries and HTML generation for the report CLI, web export and
WebAssembly adapter. The executable remains `kronika-report`; its Cargo package
is `kronika-report-cli` in `bins/kronika-report`.

The portable core binds owned ZMS and canonical IDX bytes to an explicit
`SegmentId`, then executes the existing `kronika-query` requests:

```rust
use kronika_report::{ReportEngine, ReportInput};

let engine = ReportEngine::new(ReportInput {
    segment_id,
    zms,
    idx,
    configured_sources,
    max_zms_bytes,
})?;
engine.execute(request, &mut sink)?;
```

Portable consumers set `default-features = false`. The `generator` feature,
enabled by default, adds native input validation, index construction and
self-contained HTML assembly:

```rust
let summary = kronika_report::write_html_from_file(
    input_file,
    max_zms_bytes,
    &mut output,
)?;
```

Callers own file opening, output publication and cleanup after a sink failure.
The library has no CLI parser or transport runtime. A report's explicit
`ReportTimeRange` uses positive, JavaScript-safe Unix microseconds with an
exclusive upper bound.

The generator embeds the [generated browser assets](assets/README.md). Its
fixtures and query, HTML and collection-mode tests live with this crate; CLI
argument and publication tests remain in `bins/kronika-report`.
