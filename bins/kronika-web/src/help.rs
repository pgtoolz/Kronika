//! Launch examples and operational notes appended to generated parameter help.

pub(crate) const EXAMPLES: &str = r"Examples:
  Linux recording, listening on all IPv4 interfaces:
    sudo kronika-web --storage-dir /var/lib/kronika \
      --listen 0.0.0.0:8080

  PostgreSQL recording, with authentication:
    kronika-web --storage-dir ./recording \
      --user kronika --password 'replace-with-a-random-password'

  Existing environment configuration also works:
    KRONIKA_STORAGE_DIR=./recording kronika-web

  Open http://SERVER_IP:8080/. The default listener accepts local connections only.

Configuration:
  CLI arguments override environment variables, then built-in defaults apply.
  --storage-dir is required unless KRONIKA_STORAGE_DIR is set.
  To clear optional credentials or demo mode, unset the corresponding variables.
  Both credentials unset: browser, API and MCP access is unauthenticated.
  Both nonempty: browser sessions and HTTP Basic authentication are enabled.
  A missing partner or an explicitly empty credential is a startup error.
  --demo synthetic (KRONIKA_WEB_DEMO=synthetic) marks generated demo data.

Configured sources:
  none        (0) Neither source family configured.
  os          (1) Linux OS configured.
  postgresql  (2) PostgreSQL configured.
  all         (3) Linux OS and PostgreSQL configured (default).
  Names and legacy bitsets 0..3 work in both --sources and KRONIKA_WEB_SOURCES.
  Override these catalog flags with --sources or KRONIKA_WEB_SOURCES.
  All recorded data remains available. This setting does not change collection.

Storage and exports:
  Use one collector's recording directory containing active.wal and dated
  YYYY/MM/DD/<segment-id>.zms files. Individual ZMS files and flat directories of
  segments are not accepted. Web needs write access for search indexes and locks.
  TMPDIR selects writable export scratch space (default: system temporary directory).
  Temporary ZMS and HTML files coexist during export and are removed when closed.

Access and process:
  The listener serves plain HTTP. API accepts a browser session or HTTP Basic;
  MCP uses HTTP Basic at http://SERVER_IP:8080/mcp. Without credentials, omit
  Authorization. Readiness goes to stdout; request/connection/export errors and
  export timings go to stderr. There is no web log-level setting.
  Runs in the foreground. Ctrl+C or SIGTERM stops web. Invalid settings fail
  before runtime startup; listener failures exit nonzero.
";
