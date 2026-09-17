//! Examples and operational notes appended to generated parameter help.

pub(crate) const EXAMPLES: &str = r"Examples:
  Whole recording:
    kronika-report incident.zms incident.html

  Exactly 2026-09-05 19:00-20:00 UTC:
    kronika-report incident.zms incident.html \
      --from 1788634800000000 --to-exclusive 1788638400000000

Visible interval:
  Supply both bounds; 0 < from < to-exclusive <= 9007199254740991.
  Units are Unix microseconds, not seconds or RFC3339. The interval is
  [from, to-exclusive): its start is included and its end is excluded.
  Without bounds, it spans the first recorded microsecond through one
  microsecond after the last. Nearby stored samples remain available for
  interval calculations; a first rate without an earlier sample stays null.

Input and output:
  Use a finished collector segment or a kronika-dump slice, with any basename.
  HTML contains the interface, data, fonts and WebAssembly query engine.
  It opens as a local file; queries run on the browser main thread.
  Temporary HTML is written beside OUTPUT.html and replaces the destination
  only after conversion completes. Capacity must cover old and new HTML.
  No separate search index (.idx file) is created.
  No environment variables configure reports; KRONIKA_STORAGE_DIR and web
  credentials are not used. TMPDIR does not change temporary HTML placement.
  Success exits 0 with empty stdout; errors go to stderr and exit nonzero.
  Ctrl-C interrupts a running conversion.
";
