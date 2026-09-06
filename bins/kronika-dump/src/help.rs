//! Terminal instructions for storage inspection and standalone slices.

pub(crate) const HELP: &str =
    "kronika-dump - inspect recorded metrics or extract a standalone ZMS file

Usage:
  kronika-dump <DIR> [--section <ID> | --index] [--json]
               [--limit <N>] [--from <MICROSECONDS>] [--to <MICROSECONDS>]
  kronika-dump slice --from <RFC3339> --to <RFC3339> --out <FILE.zms>
  kronika-dump -h | --help
  kronika-dump --version

Examples:
  kronika-dump /path/to/recording
  kronika-dump /path/to/recording --json
  kronika-dump /path/to/recording --index
  kronika-dump /path/to/recording --section 1100001 --limit 10

DIR is the real recording directory used by kronika-collector, containing
YYYY/MM/DD/*.zms and, while collecting, active.wal. A standalone .zms file,
a flat directory of arbitrary .zms files, or a symlink is not a storage root.
Both finished segments and the committed current journal are readable while
the collector runs. Inspection requires read access to the storage root
and has no environment configuration.

Inspection options:
  No display option   List each file's time bounds, section IDs, row counts,
                      bytes per section, and bytes used by the file structure.
  --section ID        Print rows from one section, selected by its numeric
                      type_id. Replace stored dictionary IDs with their values.
                      Section IDs are listed in default output.
                      Example: 1100001 is the OS process section.
  --index             Summarize calculated time series and searchable entries.
                      With --json, print individual points and the locations of
                      matching records. Cannot be
                      combined with --section; does not write an .idx file.
  --json              One JSON object per line (NDJSON), instead of text tables.
  --limit N           Rows per segment for --section only. Default: 20.
                      N is a nonnegative integer; 0 prints every row.
  --from MICROSECONDS Inclusive earliest Unix timestamp; default: no lower bound.
  --to MICROSECONDS   Inclusive latest Unix timestamp; default: no upper bound.
                      These signed integer bounds select intersecting segments;
                      they do not trim individual section rows. Units are
                      MICROSECONDS, not seconds or RFC3339. Either may be used.
  -h, --help          Parameter reference (inspection and slice).
  --version           Program version.

Inspection prints data to stdout; text-mode scan warnings and errors use
stderr. With --json, scan warnings are also JSON objects on stdout. It exits
after reading; a closed output pipe is successful. Other failures exit nonzero.
";

pub(crate) const SLICE_HELP: &str = r"kronika-dump slice - extract a time interval into one standalone ZMS file

Usage:
  KRONIKA_STORAGE_DIR=<DIR> kronika-dump slice \
    --from <RFC3339> --to <RFC3339> --out <FILE.zms>
  kronika-dump slice -h | --help

Example: extract 19:00:00 through 19:59:59 UTC, including both whole seconds:
  sudo env KRONIKA_STORAGE_DIR=/path/to/recording kronika-dump slice \
    --from 2026-09-05T19:00:00Z --to 2026-09-05T19:59:59Z \
    --out incident.zms

Required environment:
  KRONIKA_STORAGE_DIR  Existing real collector storage directory, not a .zms
                       file or symlink. No default. Both finished segments and
                       the committed current journal are read. Read permission
                       is required; no database or web connection is used.

Required options (each exactly once, in any order):
  --from RFC3339       Inclusive first whole second.
  --to RFC3339         Inclusive last whole second; must be at or after --from.
                      Include a timezone: Z or an offset such as +03:00.
                      Fractional seconds and Unix numeric timestamps are refused.
                      --from equal to --to selects that entire second.
  --out FILE.zms       Exact new output path with a .zms suffix. Its parent
                      directory must exist and be writable. Existing paths are
                      refused; output is never overwritten.
  -h, --help          Slice parameter reference.

An interval with no recorded rows fails. The file can retain up to 30 seconds
of nearby samples on each side for interval calculations. On success, stdout
reports bytes, rows, sections, and requested/actual bounds in Unix microseconds.
The requested_to_exclusive value is one second after the requested --to.

Work files and temporary output are created beside --out, on its filesystem;
TMPDIR does not move them. Required capacity includes both work files and result.
The completed ZMS is checked before it is saved to the requested path. The command exits when done;
errors go to stderr and return nonzero. Ctrl-C interrupts a running command.
";
