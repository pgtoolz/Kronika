//! Launch examples and operational notes appended to generated parameter help.

pub(crate) const EXAMPLES: &str = r"Examples:
  Linux recording:
    sudo kronika-collector --storage-dir /var/lib/kronika --retention 10GiB

  Local Linux and PostgreSQL, with automatic log discovery:
    sudo kronika-collector --storage-dir /var/lib/kronika \
      --pg-dsn 'host=localhost user=monitor dbname=postgres'
    The collector asks PostgreSQL for its current log file (pg_current_logfile)
    and reads it locally. No --pg-log is needed; the file must be readable.

  PostgreSQL only (no sudo):
    kronika-collector --mode postgresql --storage-dir ./recording \
      --pg-dsn 'host=pg.example.net user=monitor dbname=postgres sslmode=require'

Configuration:
  CLI arguments override environment variables, then built-in defaults apply.
  Repeated CLI log paths and PgBouncer DSNs replace the entire corresponding env
  list. Only env lists use semicolons; a CLI value is never split. To clear an
  inherited optional setting, unset its environment variable before starting.
  KRONIKA_PG_DSNS is the legacy fallback: only its first entry is used. Without
  --pg-dsn, setting both it and KRONIKA_PG_DSN is an error.
  Storage sizes accept numbers with or without a suffix: 4096, 4096.75,
  1.5kb, 1.5Mb, 1.5M, 1.5GiB. Suffixes are case-insensitive.
  G/GB = 1000^3, GiB = 1024^3 (likewise K, M, T). Without a suffix, the unit
  is bytes. Fractional byte counts round to the nearest byte.

Collection:
  Runs in the foreground. local collects Linux and optional PostgreSQL in the
  same VM or pod; postgresql collects PostgreSQL and explicit local log paths.
  In local mode, --pg-dsn also discovers logs automatically; --pg-log adds
  extra paths. In postgresql mode, logs require --pg-log: no paths are discovered
  and remote files are not downloaded through the database connection.
  Mode is never inferred from the DSN. PgBouncer logs require local mode.
  TLS verifies certificates and hostnames; --pg-ssl-root-cert replaces the CA
  roots. Direct PostgreSQL and PgBouncer session pooling are supported.
  Transaction/statement pooling and sslmode=verify-full are unsupported.
  Intervals are whole seconds. A source interval of 0 reads every timer cycle;
  it does not disable that source. Statements/plans always wait at least 300s.
  A nonempty lock-wait result enables the faster activity interval; an empty
  result restores the ordinary interval. Queries run sequentially, so a slow
  query can delay activity. --interval-s 0 keeps all collection signal-driven.

Signals and output:
  SIGUSR2 collects immediately, preserving the statements/plans cooldown, and
  closes a nonempty segment after appending. SIGTERM and SIGINT stop without
  discarding active.wal. Restarting recovers its valid data into a segment.
  Structured diagnostics go to stderr; readiness and written paths to stdout.
  Invalid settings fail before startup. Individual source failures are logged
  and retried. Unrecoverable persistence failures stop the collector.
";
