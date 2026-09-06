//! Web parameter reference.

pub(crate) const HELP: &str = r"kronika-web - browse a Kronika recording and serve its HTTP API and MCP tools

Usage: kronika-web
       kronika-web --help | -h | --version

Runs in the foreground. Configuration is environment-only. Required variables:
KRONIKA_STORAGE_DIR, KRONIKA_WEB_USER, KRONIKA_WEB_PASSWORD, KRONIKA_WEB_SOURCES.
The default address is 127.0.0.1:8080; sign in at http://127.0.0.1:8080/.

EXAMPLE
  Run over an existing recording with your configured web credentials:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_USER=kronika KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
    kronika-web

  The process needs read/write access to the recording directory.

KRONIKA_WEB_SOURCES (required; no default)
  0  Neither source family declared configured.
  1  Linux OS declared configured (bit 0).
  2  PostgreSQL declared configured (bit 1).
  3  Linux OS and PostgreSQL declared configured.

  This setting tells the API catalog which sources are configured. The browser
  hides the PostgreSQL no-data tooltip when PostgreSQL is marked configured or
  PostgreSQL data has been recorded. The Linux OS flag is only API metadata.
  All recorded data remains available for every value. Health uses instance
  information saved by the collector.
  KRONIKA_PG_DSNS on kronika-collector enables PostgreSQL metric collection.

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      No default. Use the collector's recording directory, containing active.wal
      and YYYY/MM/DD/<segment-id>.zms. An individual ZMS file or a flat directory
      of segment files is not accepted. The directory must exist. Web needs
      write access to save search indexes (.idx files) and locks that prevent
      two processes from building the same index at once.
  KRONIKA_WEB_USER
      No default. Nonempty login name, used by browser login and HTTP Basic auth.
  KRONIKA_WEB_PASSWORD
      No default. Nonempty password for that account.
      Both credentials remain required even with KRONIKA_WEB_AUTH=disabled.
  KRONIKA_WEB_SOURCES
      No default. Accepted values: 0, 1, 2, 3; meanings above.

OPTIONAL ENVIRONMENT
  KRONIKA_WEB_LISTEN   default 127.0.0.1:8080
      IP address and port, e.g. 127.0.0.1:8080, 0.0.0.0:8080, or [::1]:8080.
      Hostnames are not accepted. The default accepts local connections only.
      The listener serves plain HTTP.
  KRONIKA_WEB_AUTH     default required; accepted: required, disabled
      required enforces browser sessions and API/MCP authentication. disabled
      permits unauthenticated access.
      Setting disabled does not remove the required user/password configuration.
  KRONIKA_WEB_DEMO     unset by default; the only set value is synthetic
      Tells API catalog clients that the recording contains generated demo data.
  TMPDIR              default the system temporary directory (normally /tmp)
      Temporary ZMS and HTML files during browser exports. Requires write
      access and capacity for both files. Files are removed when closed.

LOGIN, API, AND MCP
  Browser: http://127.0.0.1:8080/ shows the sign-in form. The configured account
  creates a browser session. API and MCP clients use the same account via HTTP
  Basic authentication; a browser session is also accepted for API requests.

  MCP uses http://127.0.0.1:8080/mcp with the same HTTP Basic credentials.

LOGS AND STOPPING
  Readiness (ready IP:PORT) goes to stdout; request/connection/export errors and
  export timings go to stderr. There is no web log-level environment setting.
  Ctrl+C or SIGTERM terminates web; the stored recording remains available on
  restart. Invalid configuration or listener failure exits nonzero.
";
