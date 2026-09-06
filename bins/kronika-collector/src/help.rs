//! Collector parameter reference.

pub(crate) const HELP: &str = r"kronika-collector - record Linux metrics, PostgreSQL metrics, and local logs

Usage: kronika-collector
       kronika-collector --help | -h | --version

Runs in the foreground. Configure it with environment variables; there are no
collection flags or public subcommands. Only KRONIKA_STORAGE_DIR is required.

EXAMPLES
  Linux recording:
    sudo env KRONIKA_STORAGE_DIR=/path/to/recording kronika-collector

  Local PostgreSQL sharing the collector's CPU limits, existing connection:
    sudo env KRONIKA_STORAGE_DIR=/path/to/recording \
      KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
      kronika-collector

  Remote PostgreSQL with 4 CPUs, using an existing connection:
    sudo env KRONIKA_STORAGE_DIR=/path/to/recording \
      KRONIKA_PG_DSNS='host=pg.example.net port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
      KRONIKA_POSTGRES_EFFECTIVE_CPUS=4 kronika-collector

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      Directory where the collector saves recordings. No default. It contains
      the current journal (active.wal), finished files under
      YYYY/MM/DD/<segment-id>.zms, and a lock to prevent two writers. Use a real
      directory, not a ZMS filename or symlink; web uses this same directory.

OPTIONAL POSTGRESQL AND LOG ENVIRONMENT (all unset by default)
  KRONIKA_PG_DSNS
      Semicolon-separated connection strings (DSNs): keyword/value pairs or
      PostgreSQL connection URLs. The first
      enables metrics from that server's connectable databases; all entries
      discover PostgreSQL log paths and formats. Additional DSNs are for log
      discovery, not additional metric sources. Leave unset for Linux only.
      Use an existing monitoring connection, directly or through PgBouncer
      session pooling. Transaction/statement pooling and TLS are unsupported.
  KRONIKA_POSTGRES_EFFECTIVE_CPUS
      Explicit target PostgreSQL CPU capacity: whole number 1..4294967295.
      Requires KRONIKA_PG_DSNS. Set for remote PostgreSQL or a different cgroup
      (a group of processes with shared resource limits), even on the same host.
      This value overrides automatic capacity. Leave unset when local PostgreSQL
      shares the collector's VM/container CPU limits. For each PostgreSQL sample,
      use the latest CPU information recorded at or before it: the VM CPU count
      or the container quota divided by its period, capped by its allowed CPU
      set (cpuset). Fractions are preserved. Unlimited quota uses a positive
      cpuset count; unknown capacity gives null Health. The connection address
      alone cannot establish which CPU limits apply. Health and active-backend
      marks compare the active count with twice this capacity.
  KRONIKA_PG_LOGS
      Optional local PostgreSQL log paths or globs, separated by semicolons.
      When unset, each KRONIKA_PG_DSNS entry still discovers its current log
      through pg_current_logfile(). Explicit entries add to those sources.
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
  KRONIKA_OS_CGROUP_INTERVAL_S        default 30; container cgroup controllers
  KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S default 30; process-to-cgroup mappings
  KRONIKA_LOG_INTERVAL_S              default 10; configured PostgreSQL/PgBouncer logs
  KRONIKA_PG_INTERVAL_S               default 30; PostgreSQL metrics and settings
  KRONIKA_PG_RELATIONS_INTERVAL_S     default 300; relations and database/extension discovery

OPTIONAL LOGGING AND MOUNT PATHS
  KRONIKA_LOG_LEVEL   default info; error, warn (or warning), info, debug, trace
      Case-insensitive. Structured logs go to stderr; readiness and written
      segment paths go to stdout. Segment-write logs include peak rss_kib.
  KRONIKA_PROC_ROOT   default /proc; procfs mount to read
      Setting this limits container detection to that root's cgroup file.
  KRONIKA_SYS_ROOT    default /sys; sysfs mount to read

STOPPING AND ERRORS
  SIGINT (Ctrl+C) and SIGTERM stop collection and retain active.wal. Restart
  with the same directory to recover it. SIGUSR2 forces a collection cycle;
  the accumulated segment is saved when the cycle appended data and left
  a nonempty segment. Invalid configuration and unrecoverable storage failures
  exit nonzero; individual source errors are logged and retried.
";
