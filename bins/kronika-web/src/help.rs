//! Web parameter reference.

pub(crate) const HELP: &str = r"kronika-web - browse a Kronika recording and serve its HTTP API and MCP tools

Usage: kronika-web
       kronika-web --help | -h | --version

Runs in the foreground. Configuration is environment-only. Required variables:
KRONIKA_STORAGE_DIR and KRONIKA_WEB_SOURCES.
Leave both KRONIKA_WEB_USER and KRONIKA_WEB_PASSWORD unset for unauthenticated
access, or set both to nonempty values to enable authentication.
The default address is 127.0.0.1:8080 and accepts local connections only.

EXAMPLES
  Serve an existing recording on the server's network interfaces:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_LISTEN=0.0.0.0:8080 kronika-web

  With both credentials unset, open http://SERVER_IP:8080/ without signing in.
  Anyone who can reach this listener can access the recording.

  Enable authentication by setting both credentials:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_LISTEN=0.0.0.0:8080 KRONIKA_WEB_USER=kronika \
    KRONIKA_WEB_PASSWORD='replace-with-a-random-password' kronika-web

  Sign in at http://SERVER_IP:8080/. The process needs read/write access to the
  recording directory. SERVER_IP is the server's address, not 0.0.0.0.

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
  KRONIKA_PG_DSN on kronika-collector enables PostgreSQL metric collection.

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      No default. One collector's recording directory; use a separate web
      process and listen address for each server's directory. Contains active.wal
      and YYYY/MM/DD/<segment-id>.zms. An individual ZMS file or a flat directory
      of segment files is not accepted. The directory must exist. Web needs
      write access to save search indexes (.idx files) and locks that prevent
      two processes from building the same index at once.
  KRONIKA_WEB_SOURCES
      No default. Accepted values: 0, 1, 2, 3; meanings above.

OPTIONAL ENVIRONMENT
  KRONIKA_WEB_LISTEN   default 127.0.0.1:8080
      IP address and port, e.g. 127.0.0.1:8080, 0.0.0.0:8080, or [::1]:8080.
      Hostnames are not accepted. The default accepts local connections only.
      The listener serves plain HTTP.
  KRONIKA_WEB_USER and KRONIKA_WEB_PASSWORD
      Both unset: browser, API, and MCP access is unauthenticated.
      Both nonempty: browser sessions and HTTP Basic authentication are enabled.
      Setting only one or an explicitly empty value is a startup error.
  KRONIKA_WEB_DEMO     unset by default; the only set value is synthetic
      Tells API catalog clients that the recording contains generated demo data.
  TMPDIR              default the system temporary directory (normally /tmp)
      Temporary ZMS and HTML files during browser exports. Requires write
      access and capacity for both files. Files are removed when closed.

LOGIN, API, AND MCP
  Browser: http://SERVER_IP:8080/ opens directly when both credentials are
  unset. When both are configured, the sign-in form creates a browser session.
  API and MCP clients use the same account via HTTP Basic authentication;
  a browser session is also accepted for API requests.

  MCP uses http://SERVER_IP:8080/mcp. Omit Authorization when both credentials
  are unset; otherwise use the configured HTTP Basic credentials.
  For a local-only listener, use http://127.0.0.1:8080/.

LOGS AND STOPPING
  Readiness (ready IP:PORT) goes to stdout; request/connection/export errors and
  export timings go to stderr. There is no web log-level environment setting.
  Ctrl+C or SIGTERM terminates web; the stored recording remains available on
  restart. Invalid configuration or listener failure exits nonzero.
";
