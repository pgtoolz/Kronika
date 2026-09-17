//! Examples and operational details appended to clap's generated reference.

pub(crate) const INSPECT: &str = "Examples:
  kronika-dump /path/to/recording
  kronika-dump /path/to/recording --json
  kronika-dump /path/to/recording --index
  kronika-dump /path/to/recording --section 1100001 --limit 10
  kronika-dump slice --help

With no display flag, list segment bounds, section IDs, row counts, section
bytes, and file overhead. Inspection reads finished segments and the committed
current journal while the collector runs. DIR must be a real recording root;
standalone .zms files, flat segment directories, and symlinks are not roots.
Inspection has no environment configuration.

Data goes to stdout; text scan warnings and errors go to stderr. With --json,
scan warnings go to stdout. A closed output pipe is successful. Other failures
exit nonzero. --from/--to select intersecting segments, not individual rows.";

pub(crate) const SLICE: &str = "Example: extract both whole seconds and everything between them:
  kronika-dump slice --storage-dir /path/to/recording \\\n    --from 2026-09-05T19:00:00Z --to 2026-09-05T19:59:59Z \\\n    --out incident.zms

--storage-dir overrides KRONIKA_STORAGE_DIR. The storage root must be a real
collector directory, not a .zms file or symlink. Both finished segments and the
committed current journal are read. All slice options are supplied once.

The requested interval is [from, to + 1 second). An interval with no recorded
rows fails. Up to 30 seconds of surrounding samples can be retained for interval
calculations. Stdout reports bytes, rows, sections, and requested/actual bounds
in Unix microseconds; requested_to_exclusive is one second after --to.

Work files are created beside --out on the same filesystem; TMPDIR does not move
them. Its parent must have room for work files and the result. The completed ZMS
is validated before publication, and an existing output is never overwritten.
Errors go to stderr and exit nonzero. Ctrl+C interrupts a running command.";
