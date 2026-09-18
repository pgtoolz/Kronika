# kronika-slice

Native extraction of a bounded time range from Kronika storage into one
standalone ZMS. Both `kronika-dump slice` and web report export use this library.

```rust
use kronika_slice::{SliceRange, UtcSecond, slice_to_zms};

let range = SliceRange::new(
    UtcSecond::from_unix_seconds(from_seconds)?,
    UtcSecond::from_unix_seconds(to_seconds)?,
)?;
let summary = slice_to_zms(&reader, range, &mut scratch_file, &mut output)?;
```

Both endpoints are inclusive whole UTC seconds. Selection retains bounded
neighboring observations needed for rates and counters, preserves dictionaries
and section layouts, and retries once when active-data rollover invalidates
the captured source.

Callers create and own the scratch file and output sink. Scratch contents are
discarded during preparation; output is untouched until selection, decoding
and section construction succeed. Callers handle cleanup after output failure
and publish the finished recording according to their destination policy.

The library owns `UtcSecond`, `SliceRange`, `SliceSummary`, typed range and
slice errors, and the selection/staging tests. CLI arguments, inspection,
temporary-file placement and publication without overwrite remain in
`bins/kronika-dump`. This native crate is outside the portable report and
WebAssembly dependency graphs.
