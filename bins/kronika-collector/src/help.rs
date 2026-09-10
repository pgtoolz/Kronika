//! Collector parameter reference.

pub(crate) const HELP: &str = r"kronika-collector - record Linux metrics, PostgreSQL metrics, and local logs

Usage: kronika-collector
       kronika-collector --help | -h | --version

Runs in the foreground. Configure it with environment variables; there are no
collection flags or public subcommands. Local mode requires KRONIKA_STORAGE_DIR;
postgresql mode also requires KRONIKA_PG_DSNS.

EXAMPLES
  Linux recording:
    sudo env KRONIKA_STORAGE_DIR=/path/to/recording kronika-collector

  PostgreSQL on the same Linux machine/VM, existing connection:
    sudo env KRONIKA_STORAGE_DIR=/path/to/recording \
      KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=disable' \
      kronika-collector

  PostgreSQL only, local or remote; no sudo:
    KRONIKA_COLLECTOR_MODE=postgresql KRONIKA_STORAGE_DIR=$HOME/kronika-data \
      KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres sslmode=require' \
      kronika-collector

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      Directory where the collector saves recordings. No default. It contains
      the current journal (active.wal), finished files under
      YYYY/MM/DD/<segment-id>.zms, and a lock to prevent two writers. Use a real
      directory, not a ZMS filename or symlink; web uses this same directory.

OPTIONAL COLLECTION MODE
  KRONIKA_COLLECTOR_MODE   default local; local or postgresql
      local: Linux metrics and optional PostgreSQL on the recorded machine.
      postgresql: PostgreSQL only, local or remote; requires KRONIKA_PG_DSNS.
      No local Linux, process, cgroup, or host identity reads. No root required.
      PgBouncer log settings are not accepted in postgresql mode.

OPTIONAL POSTGRESQL AND LOG ENVIRONMENT (all unset by default)
  KRONIKA_PG_DSNS
      Semicolon-separated connection strings: keyword/value pairs or PostgreSQL
      URLs. The first enables metrics from that server's connectable databases.
      In local mode, all entries also discover local PostgreSQL logs. Additional
      DSNs are not additional metric sources. Leave unset for Linux only.
      Direct PostgreSQL and PgBouncer session pooling are supported.
      DSN sslmode: disable (plaintext), prefer (default), require (TLS required).
      TLS validates the CA and server hostname, including query cancellation.
      Transaction/statement pooling and sslmode=verify-full are unsupported.
  KRONIKA_PG_SSL_ROOT_CERT
      Optional PEM CA bundle for PostgreSQL TLS. When set, replaces the included
      public CA roots. Certificate and hostname validation remain enabled.
  KRONIKA_POSTGRES_EFFECTIVE_CPUS
      Optional target PostgreSQL CPU capacity: whole number 1..4294967295.
      Requires KRONIKA_PG_DSNS. Overrides automatic capacity. In local mode on
      a shared machine/VM, unset uses the latest recorded machine CPU count at
      or before each PostgreSQL sample. Remote and container capacity is unknown
      without this setting; SQL metrics continue, dependent Health is null.
      Health and active-backend marks compare active count with twice capacity.
  KRONIKA_PG_LOGS
      Optional local PostgreSQL log paths or globs, separated by semicolons.
      In local mode, each KRONIKA_PG_DSNS entry discovers its current log through
      pg_current_logfile(), even when this list is unset; explicit paths add to
      those sources. In postgresql mode, only explicit paths are opened.
      Files must be readable on the collector host. Example:
      '/var/log/postgresql/*.csv;/srv/pg-logs/*.json'. Only the last path
      component supports * and ?. Filename .csv selects csvlog, .json selects
      jsonlog, otherwise stderr. Paths found only through this list have no
      discovered server ID or stderr prefix; severity, message and continuations
      are parsed. Missing input timestamps use collection time.
  KRONIKA_PGBOUNCER_DSNS
      Semicolon-separated PgBouncer admin-console DSNs (dbname=pgbouncer), for
      SHOW CONFIG/logfile discovery. The account needs stats_users membership.
      This enables log discovery, not PostgreSQL metric collection.
  KRONIKA_PGBOUNCER_LOGS
      Semicolon-separated local PgBouncer log paths or final-component globs.

  Blank lists add no explicit entries; blank entries between semicolons are errors.
  Log paths and patterns refer to files on the collector host. Paths reached
  twice are followed once.
  Discovery retries every five minutes; an unavailable source logs a warning
  while other collection continues. The file read buffer is 64 KiB; a batch
  consumes up to 4 MiB of raw input, and one collection reads at most 256 MiB
  per file across batches.

OPTIONAL STORAGE ENVIRONMENT (sizes are nonnegative whole numbers of bytes)
  KRONIKA_SEGMENT_MAX_BYTES       default 67108864 (64 MiB), greater than 0
      Write a finished segment once the journal reaches this many raw bytes.
  KRONIKA_SEGMENT_MAX_AGE_S       default 900 seconds
      Write the open segment at this age; 0 makes it eligible immediately.
  KRONIKA_JOURNAL_MAX_BYTES       default 1073741824 (1 GiB), range 36..1073741824
      Hard active.wal size cap; reaching it writes the segment early. A segment
      threshold larger than this cap logs a warning and the journal cap wins.
  KRONIKA_RETENTION               default 2147483648 (2 GiB)
      Storage target: byte count, auto (= auto:80), or auto:P (P=1..99).
      A fixed budget must be at least twice KRONIKA_SEGMENT_MAX_BYTES and counts
      the journal, segments, indexes, and recognized temporaries. For example,
      10737418240 sets 10 GiB. auto:P targets used space on the whole filesystem.
      Rotation removes old finished segments/indexes, preserving active.wal and
      the newest finished segment. It checks after a segment is saved and every minute;
      a running collection can delay the check and exceed the target.

OPTIONAL COLLECTION INTERVALS (nonnegative whole numbers of seconds)
  KRONIKA_INTERVAL_S                  default 5; maximum timer sleep
      0 disables timed collection; SIGUSR2 still collects. Positive per-source
      intervals can wake the timer earlier. A per-source 0 reads every timer
      cycle; it does not disable that source.
  KRONIKA_OS_CORE_INTERVAL_S          default 10; CPU, memory, disks, network, PSI
  KRONIKA_OS_MOUNTTOPO_INTERVAL_S     default 60; mounts, capacity, device topology
  KRONIKA_OS_PROCESS_INTERVAL_S       default 5; process counters
  KRONIKA_OS_PROCESS_STATUS_INTERVAL_S default 30; process status details
  KRONIKA_OS_CGROUP_INTERVAL_S        default 30; all visible accessible cgroup v2 groups
  KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S default 30; process-to-cgroup v2 mappings
  KRONIKA_LOG_INTERVAL_S              default 10; configured PostgreSQL/PgBouncer logs
  KRONIKA_PG_INTERVAL_S               default 30; PostgreSQL metrics and settings
  KRONIKA_PG_RELATIONS_INTERVAL_S     default 300; tables and indexes

OPTIONAL LOGGING AND MOUNT PATHS
  KRONIKA_LOG_LEVEL   default info; error, warn (or warning), info, debug, trace
      Case-insensitive. Structured logs go to stderr; readiness and written
      segment paths go to stdout. In local mode, segment-write logs include peak rss_kib.
  KRONIKA_PROC_ROOT   default /proc; procfs mount to read
      Used only in local mode. Container detection uses that root's cgroup file.
  KRONIKA_SYS_ROOT    default /sys; sysfs mount to read in local mode

STOPPING AND ERRORS
  SIGINT (Ctrl+C) and SIGTERM stop collection and retain active.wal. Restart
  with the same directory to recover it. SIGUSR2 forces a collection cycle;
  the accumulated segment is saved when the cycle appended data and left
  a nonempty segment. Invalid configuration and unrecoverable storage failures
  exit nonzero; individual source errors are logged and retried.
";
