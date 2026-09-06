//! Report parameter reference.

pub(crate) const HELP: &str = r"kronika-report - turn a recording into one interactive HTML file

Usage:
  kronika-report <INPUT.zms> <OUTPUT.html>
  kronika-report --from <MICROSECONDS> --to-exclusive <MICROSECONDS> \
                 <INPUT.zms> <OUTPUT.html>
  kronika-report -h | --help
  kronika-report --version

Example:
  kronika-report incident.zms incident.html

The HTML contains the interface, recorded data, fonts and a WebAssembly (WASM)
engine that reads the data.
It opens as a local file; queries run on the browser main thread. The visible
recording is fixed, with local tables, search, charts and heatmaps.

Input and output:
  INPUT.zms          One finished, valid standalone ZMS file, with any basename.
                     Use a finished collector segment or kronika-dump slice.
                     A storage directory or active.wal is not a report input.
  OUTPUT.html        Exact output path; the .html suffix is required. Its parent
                     directory must exist and be writable. The completed HTML
                     REPLACES an existing output file in one operation, after
                     the new HTML is complete.

Options:
  --from MICROSECONDS          Inclusive beginning of the visible report window.
  --to-exclusive MICROSECONDS  Exclusive end of the visible report window.
                               Supply BOTH, in this order, before the two paths.
                               Units: Unix MICROSECONDS, not seconds or RFC3339.
                               Integers must satisfy:
                                 0 < from < to-exclusive <= 9007199254740991
                               The interval is [from, to-exclusive): its start
                               is included and its end is excluded.
  -h, --help                   Parameter reference.
  --version                    Program version.

By default the visible window spans the whole input, from its first recorded
microsecond through one microsecond after its last. Explicit bounds restrict
navigation and the displayed interval; nearby stored samples remain available
for interval calculations. A first rate without an earlier sample stays null.

Example: show exactly 2026-09-05 19:00-20:00 UTC:
  kronika-report --from 1788634800000000 --to-exclusive 1788638400000000 \
    incident.zms incident.html

Environment and completion:
  No environment variables are required or read as report configuration.
  KRONIKA_STORAGE_DIR and web credentials are not used by kronika-report.
  Temporary HTML is written beside OUTPUT.html; TMPDIR does not change this.
  No separate search index (.idx file) is created. Required capacity includes old and new HTML.
  Success exits 0 without writing to stdout; errors go to stderr and exit
  nonzero. Ctrl-C interrupts a running conversion.
";
