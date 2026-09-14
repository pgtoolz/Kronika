//! Web parameter reference.

pub(crate) const HELP: &str = r"kronika-web - browse a Kronika recording and serve its HTTP API and MCP tools

Usage: kronika-web
       kronika-web --help | -h | --version

Runs in the foreground. Configuration is environment-only. Required variables:
KRONIKA_STORAGE_DIR and KRONIKA_WEB_SOURCES.

EXAMPLES
  Without authentication, leave both credentials unset:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_LISTEN=0.0.0.0:8080 kronika-web

  To require a login, set both credentials:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_LISTEN=0.0.0.0:8080 KRONIKA_WEB_USER=kronika \
    KRONIKA_WEB_PASSWORD='replace-with-a-random-password' kronika-web

  Open http://SERVER_IP:8080/, replacing SERVER_IP with the server's address.

KRONIKA_WEB_SOURCES (required, no default)
  0  Neither source family declared configured.
  1  Linux OS declared configured (bit 0).
  2  PostgreSQL declared configured (bit 1).
  3  Linux OS and PostgreSQL declared configured.

  This setting tells the API catalog which sources are configured. The browser
  hides the PostgreSQL no-data tooltip when PostgreSQL is marked configured or
  PostgreSQL data has been recorded. The Linux OS flag is only API metadata.
  All recorded data remains available for every value. Health uses instance
  information saved by the collector.
  KRONIKA_PG_DSN on kronika-collector enables PostgreSQL metric collection.

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      No default. One collector's recording directory. Use a separate web
      process and listen address for each server's directory. Contains active.wal
      and YYYY/MM/DD/<segment-id>.zms. An individual ZMS file or a flat directory
      of segment files is not accepted. The directory must exist. Web needs
      write access to save search indexes (.idx files) and locks that prevent
      two processes from building the same index at once.
  KRONIKA_WEB_SOURCES
      No default. Accepted values: 0, 1, 2, 3. See meanings above.

OPTIONAL ENVIRONMENT
  KRONIKA_WEB_LISTEN   default 127.0.0.1:8080
      IP address and port, e.g. 127.0.0.1:8080, 0.0.0.0:8080, or [::1]:8080.
      Hostnames are not accepted. The default accepts local connections only.
      The listener serves plain HTTP.
  KRONIKA_WEB_USER and KRONIKA_WEB_PASSWORD
      Both unset: browser, API, and MCP access is unauthenticated.
      Both nonempty: browser sessions and HTTP Basic authentication are enabled.
      Setting only one or an explicitly empty value is a startup error.
  KRONIKA_WEB_DEMO     unset by default
      The only accepted value is synthetic. Tells API catalog clients that the
      recording contains generated demo data.
  TMPDIR              default the system temporary directory (normally /tmp)
      Temporary ZMS and HTML files during browser exports. Requires write
      access and capacity for both files. Files are removed when closed.

LOGIN, API, AND MCP
  With credentials configured, browser login creates a session. API requests
  accept that session or HTTP Basic. MCP uses HTTP Basic at
  http://SERVER_IP:8080/mcp. With credentials unset, omit Authorization.

LOGS AND STOPPING
  Readiness (ready IP:PORT) goes to stdout. Request/connection/export errors and
  export timings go to stderr. There is no web log-level environment setting.
  Ctrl+C or SIGTERM terminates web. Invalid configuration or listener failure
  exits nonzero.
";
