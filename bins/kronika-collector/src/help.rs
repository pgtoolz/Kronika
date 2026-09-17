//! Collector parameter reference.

pub(crate) const HELP: &str = r"kronika-collector - record Linux metrics, PostgreSQL metrics, and local logs

Usage: kronika-collector
       kronika-collector --help | -h | --version

Runs in the foreground. Configure it with environment variables.
KRONIKA_STORAGE_DIR is required. PostgreSQL mode also requires KRONIKA_PG_DSN.

EXAMPLES
  Linux recording:
    sudo env KRONIKA_STORAGE_DIR=/path/to/recording kronika-collector

  PostgreSQL and Linux in the same VM or pod:
    sudo env KRONIKA_STORAGE_DIR=/path/to/recording \
      KRONIKA_PG_DSN='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
      kronika-collector

  PostgreSQL only, local or remote (no sudo):
    KRONIKA_COLLECTOR_MODE=postgresql KRONIKA_STORAGE_DIR=$HOME/kronika-data \
      KRONIKA_PG_DSN='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
      kronika-collector

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      Directory where the collector saves recordings. No default. It contains
      the current journal (active.wal), finished files under
      YYYY/MM/DD/<segment-id>.zms, persistent seal.seed, and a writer lock.
      Use a real directory, not a ZMS filename or symlink. Web uses this directory.

OPTIONAL COLLECTION MODE
  KRONIKA_COLLECTOR_MODE   default local, local or postgresql
      local: Linux metrics and optional PostgreSQL in the same VM or pod.
      postgresql: PostgreSQL only. Requires KRONIKA_PG_DSN.
      The mode is not inferred from the DSN address.
      In postgresql mode: no Linux, process, cgroup, or host identity reads.
      No root required in postgresql mode.
      PgBouncer log settings are not accepted in postgresql mode.

OPTIONAL POSTGRESQL AND LOG ENVIRONMENT
  KRONIKA_PG_DSN
      One connection string: keyword/value pairs or a PostgreSQL URL.
      Selects one server and its connectable databases. In local mode, the same
      DSN also discovers local PostgreSQL logs. Leave unset for Linux
      only. Use a separate process and storage directory per server.
      Direct PostgreSQL and PgBouncer session pooling are supported.
      DSN sslmode: disable (plaintext), prefer (default), require (TLS required).
      TLS validates the CA and server hostname, including query cancellation.
      Transaction/statement pooling and sslmode=verify-full are unsupported.
  KRONIKA_PG_SSL_ROOT_CERT
      Optional PEM CA bundle for PostgreSQL TLS. When set, replaces the included
      public CA roots. Certificate and hostname validation remain enabled.
  KRONIKA_POSTGRES_EFFECTIVE_CPUS
      Optional target PostgreSQL CPU capacity: whole number 1..4294967295.
      Requires KRONIKA_PG_DSN. Overrides automatic capacity. In local mode on
      a shared machine/VM, unset uses the latest recorded machine CPU count at
      or before each PostgreSQL sample. Remote and container capacity is unknown
      without this setting. SQL metrics continue, dependent Health is null.
      Health and active-backend marks compare active count with twice capacity.
  KRONIKA_PG_LOGS
      Optional local PostgreSQL log paths or globs, separated by semicolons.
      In local mode, KRONIKA_PG_DSN discovers the selected server's log through
      pg_current_logfile(), even when this list is unset. Explicit paths add to
      those sources. In postgresql mode, only explicit paths are opened.
      Files must be readable on the collector host. Example:
      '/var/log/postgresql/*.csv;/srv/pg-logs/*.json'. Only the last path
      component supports * and ?. Filename .csv selects csvlog, .json selects
      jsonlog, otherwise stderr. With KRONIKA_PG_DSN, log_line_prefix and
      log_timezone come from the server. Missing stderr timestamps use read time.
  KRONIKA_PG_LOG_MAX_LAG_S
      Skip PostgreSQL log records older than this many seconds at read time.
      Positive integer. Default: 900.
  KRONIKA_PGBOUNCER_DSNS
      Semicolon-separated PgBouncer admin-console DSNs (dbname=pgbouncer), for
      SHOW CONFIG/logfile discovery. The account needs stats_users membership.
      This enables log discovery, not PostgreSQL metric collection.
  KRONIKA_PGBOUNCER_LOGS
      Semicolon-separated local PgBouncer log paths or final-component globs.

  Blank lists add no explicit entries. Blank entries between semicolons are errors.
  Log paths and patterns refer to files on the collector host. Paths reached
  twice are followed once.
  Discovery retries every five minutes. An unavailable source logs a warning
  while other collection continues. The file read buffer is 64 KiB. A batch
  consumes up to 4 MiB of raw input, and one collection reads at most 256 MiB
  per file across batches.

OPTIONAL STORAGE ENVIRONMENT (sizes are nonnegative whole numbers of bytes)
  KRONIKA_SEGMENT_MAX_BYTES       default 67108864 (64 MiB), greater than 0
      Write a finished segment once the journal reaches this many raw bytes.
  KRONIKA_SEGMENT_MAX_AGE_S       default 900 seconds
      Nonnegative period for scheduled closing, with a persistent random phase
      per storage directory. The next boundary can make a segment due earlier.
      Later ordinary boundaries keep this period. Closing can be delayed by work
      in progress. 0 makes it eligible immediately. No timed age closing when
      KRONIKA_INTERVAL_S=0.
  KRONIKA_JOURNAL_MAX_BYTES       default 1073741824 (1 GiB), range 36..1073741824
      Hard active.wal size cap. Reaching it writes the segment early. A segment
      threshold larger than this cap logs a warning and the journal cap wins.
  KRONIKA_RETENTION               default 2147483648 (2 GiB)
      Storage target: byte count, auto (= auto:80), or auto:P (P=1..99).
      A fixed budget must be at least twice KRONIKA_SEGMENT_MAX_BYTES and counts
      the journal, segments, indexes, and recognized temporaries. For example,
      10737418240 sets 10 GiB. auto:P targets used space on the whole filesystem.
      Rotation removes old finished segments/indexes, preserving active.wal and
      the newest finished segment. It checks after a segment is saved and every minute.
      A running collection can delay the check and exceed the target.

OPTIONAL COLLECTION INTERVALS (nonnegative whole seconds)
  KRONIKA_INTERVAL_S                  default 5, maximum timer sleep
      0 disables timed collection. SIGUSR2 still collects. Positive per-source
      intervals can wake the timer earlier. A per-source 0 reads every timer
      cycle, except statements/plans, whose interval must be at least 300.
      A per-source 0 does not disable that source.
  KRONIKA_OS_CORE_INTERVAL_S          default 10, CPU, memory, disks, network, PSI
  KRONIKA_OS_MOUNTTOPO_INTERVAL_S     default 60, mounts, capacity, device topology
  KRONIKA_OS_PROCESS_INTERVAL_S       default 5, process counters
  KRONIKA_OS_PROCESS_STATUS_INTERVAL_S default 30, process status details
  KRONIKA_OS_CGROUP_INTERVAL_S        default 30, all visible accessible cgroup v2 groups
  KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S default 30, process-to-cgroup v2 mappings
  KRONIKA_LOG_INTERVAL_S              default 10, configured PostgreSQL/PgBouncer logs
  KRONIKA_PG_INTERVAL_S               default 30, server counters and settings
  KRONIKA_PG_ACTIVITY_INTERVAL_S      default 10, activity, lock waits, VACUUM progress
  KRONIKA_PG_ACTIVITY_BLOCKED_INTERVAL_S default 5, activity during lock waits
      A successful nonempty lock-wait read uses the smaller of the ordinary
      and blocked activity intervals. 0 reads on every regular timer wakeup
      without adding timer wakeups.
      A successful empty read restores the base interval; errors keep it unchanged.
  KRONIKA_PG_STATEMENTS_INTERVAL_S    default 300, statements/plans and their info views
      Must be >= 300. Waits at least this long after the preceding PostgreSQL
      pass containing these sources finishes; SIGUSR2 cannot bypass the limit.
  KRONIKA_PG_RELATIONS_INTERVAL_S     default 300, tables and indexes in each database

  Collection is sequential. Slow queries can delay activity snapshots.
  KRONIKA_INTERVAL_S=0 keeps all collection signal-driven, even during lock waits.

OPTIONAL LOGGING AND MOUNT PATHS
  KRONIKA_LOG_LEVEL   default info, error, warn (or warning), info, debug, trace
      Case-insensitive. Structured logs go to stderr. Readiness and written
      segment paths go to stdout. In local mode, segment-write logs include peak rss_kib.
  KRONIKA_PROC_ROOT   default /proc, procfs mount to read
      Used only in local mode. Container detection uses that root's cgroup file.
  KRONIKA_SYS_ROOT    default /sys, sysfs mount to read in local mode

STOPPING AND ERRORS
  SIGINT (Ctrl+C) and SIGTERM stop collection and retain active.wal without a
  final ZMS close. Restart with the same directory to recover a valid nonempty
  journal immediately. SIGUSR2 forces a collection cycle while preserving the
  minimum interval for statements/plans.
  The accumulated segment is saved when the cycle appended data and left
  a nonempty segment. Invalid configuration and unrecoverable storage failures
  exit nonzero. Individual source errors are logged and retried.
";
