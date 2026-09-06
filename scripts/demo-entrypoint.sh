#!/usr/bin/env bash
set -Eeuo pipefail
umask 0027

PG_BIN=/usr/lib/postgresql/15/bin
PG_DATA=/var/lib/kronika/pgdata
PG_PORT=5432
PGB_DIR=/var/lib/kronika/pgbouncer
PGB_PORT=6432
PG_SUPERUSER=postgres
MONITOR_USER=kronika_monitor
WORKLOAD_USER=kronika_demo
WORKLOAD_DATABASE=kronika_demo
DEMO_ROOT=${KRONIKA_DEMO_DIR:-/var/lib/kronika/data}
STORAGE_DIR=${KRONIKA_STORAGE_DIR:-$DEMO_ROOT/segments}

POSTGRES_PID=
PGB_PID=
WEB_PID=
DEMO_PID=
SHUTTING_DOWN=0
STARTED_PID=

run_as() {
    local user=$1
    shift
    setpriv --reuid="$user" --regid="$user" --init-groups -- "$@"
}

start_as() {
    local user=$1
    shift
    setpriv --reuid="$user" --regid="$user" --init-groups -- "$@" &
    STARTED_PID=$!
}

# The collector needs CAP_SETUID/CAP_SETGID to switch fsuid/fsgid for PostgreSQL's
# /proc/PID/io. setpriv requires ambient capabilities in the inheritable set.
start_as_with_caps() {
    local user=$1
    local caps=$2
    shift 2
    setpriv --reuid="$user" --regid="$user" --init-groups \
        --inh-caps "$caps" --ambient-caps "$caps" -- "$@" &
    STARTED_PID=$!
}

empty_directory() {
    local path=$1
    install -d -m 0750 -o "$PG_SUPERUSER" -g "$PG_SUPERUSER" "$path"
    find "$path" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
}

wait_for_postgres() {
    local attempt
    for attempt in $(seq 1 300); do
        if run_as "$PG_SUPERUSER" "$PG_BIN/pg_isready" --quiet \
            --host=127.0.0.1 --port="$PG_PORT" --username="$PG_SUPERUSER"; then
            return 0
        fi
        if ! kill -0 "$POSTGRES_PID" 2>/dev/null; then
            wait "$POSTGRES_PID" || true
            echo "demo-entrypoint: PostgreSQL exited during startup; see $PG_DATA/postgresql-startup.log" >&2
            sed 's/^/postgresql: /' "$PG_DATA/postgresql-startup.log" >&2 || true
            return 1
        fi
        sleep 0.1
    done
    echo "demo-entrypoint: PostgreSQL was not ready within 30 seconds" >&2
    return 1
}

start_postgres() {
    local initdb_log=/tmp/kronika-initdb.log
    empty_directory "$PG_DATA"
    if ! run_as "$PG_SUPERUSER" "$PG_BIN/initdb" \
        --pgdata="$PG_DATA" --auth-host=trust --auth-local=trust \
        --username="$PG_SUPERUSER" --encoding=UTF8 --no-sync \
        >"$initdb_log" 2>&1; then
        echo "demo-entrypoint: initdb failed:" >&2
        cat "$initdb_log" >&2
        return 1
    fi
    install -m 0640 -o "$PG_SUPERUSER" -g "$PG_SUPERUSER" \
        "$initdb_log" "$PG_DATA/initdb.log"
    cat >>"$PG_DATA/postgresql.conf" <<-EOF
	listen_addresses = '127.0.0.1'
	port = $PG_PORT
	unix_socket_directories = ''
	max_connections = 120
	shared_buffers = '64MB'
	work_mem = '2MB'
	temp_file_limit = '32MB'
	shared_preload_libraries = 'pg_stat_statements,pg_store_plans'
	compute_query_id = on
	track_io_timing = on
	pg_stat_statements.max = 2000
	pg_stat_statements.track = all
	pg_store_plans.max = 2000
	pg_store_plans.track = all
	pg_store_plans.plan_format = text
	logging_collector = on
	log_destination = 'stderr'
	log_directory = 'log'
	log_filename = 'postgresql.log'
	log_file_mode = 0640
	log_truncate_on_rotation = on
	log_rotation_age = '10min'
	log_rotation_size = '16MB'
	log_checkpoints = on
	log_lock_waits = on
	deadlock_timeout = '1s'
	log_temp_files = 0
	log_min_duration_statement = 1000
	checkpoint_timeout = '5min'
	max_wal_size = '128MB'
	min_wal_size = '32MB'
	fsync = off
	full_page_writes = off
	synchronous_commit = off
	EOF
    chmod 0750 "$PG_DATA"
    start_as "$PG_SUPERUSER" "$PG_BIN/postgres" -D "$PG_DATA" \
        >"$PG_DATA/postgresql-startup.log" 2>&1
    POSTGRES_PID=$STARTED_PID
    wait_for_postgres
    chmod 0750 "$PG_DATA/log"
    echo "demo-entrypoint: PostgreSQL ready (pid $POSTGRES_PID)"
}

psql_superuser() {
    run_as "$PG_SUPERUSER" "$PG_BIN/psql" --no-psqlrc --set=ON_ERROR_STOP=1 \
        --host=127.0.0.1 --port="$PG_PORT" --username="$PG_SUPERUSER" "$@"
}

bootstrap_postgres() {
    run_as "$PG_SUPERUSER" "$PG_BIN/createuser" --host=127.0.0.1 --port="$PG_PORT" \
        --login "$MONITOR_USER"
    run_as "$PG_SUPERUSER" "$PG_BIN/createuser" --host=127.0.0.1 --port="$PG_PORT" \
        --login "$WORKLOAD_USER"
    psql_superuser --dbname=postgres <<-SQL
	GRANT pg_monitor TO $MONITOR_USER;
	GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO $MONITOR_USER;
	SQL
    psql_superuser --dbname=postgres <<-SQL
	CREATE EXTENSION pg_stat_statements;
	CREATE EXTENSION pg_store_plans;
	SQL
    run_as "$PG_SUPERUSER" "$PG_BIN/createdb" --host=127.0.0.1 --port="$PG_PORT" \
        --owner="$WORKLOAD_USER" "$WORKLOAD_DATABASE"
    psql_superuser --dbname="$WORKLOAD_DATABASE" <<-SQL
	CREATE EXTENSION pg_stat_statements;
	CREATE EXTENSION pg_store_plans;
	ALTER ROLE $WORKLOAD_USER SET statement_timeout = '10s';
	ALTER ROLE $WORKLOAD_USER SET idle_in_transaction_session_timeout = '15s';
	SQL
    echo "demo-entrypoint: PostgreSQL demo roles, database, and extensions ready"
}

wait_for_pgbouncer() {
    local attempt
    for attempt in $(seq 1 300); do
        if "$PG_BIN/psql" --no-psqlrc --set=ON_ERROR_STOP=1 \
            --host=127.0.0.1 --port="$PGB_PORT" --username="$MONITOR_USER" \
            --dbname=pgbouncer --command='SHOW VERSION' >/dev/null 2>&1; then
            return 0
        fi
        if ! kill -0 "$PGB_PID" 2>/dev/null; then
            wait "$PGB_PID" || true
            echo "demo-entrypoint: PgBouncer exited during startup; see $PGB_DIR/pgbouncer.log" >&2
            return 1
        fi
        sleep 0.1
    done
    echo "demo-entrypoint: PgBouncer was not ready within 30 seconds" >&2
    return 1
}

start_pgbouncer() {
    empty_directory "$PGB_DIR"
    printf '"%s" ""\n"%s" ""\n' "$MONITOR_USER" "$WORKLOAD_USER" >"$PGB_DIR/users.txt"
    cat >"$PGB_DIR/pgbouncer.ini" <<-EOF
	[databases]
	$WORKLOAD_DATABASE = host=127.0.0.1 port=$PG_PORT dbname=$WORKLOAD_DATABASE

	[pgbouncer]
	listen_addr = 127.0.0.1
	listen_port = $PGB_PORT
	unix_socket_dir =
	auth_type = trust
	auth_file = $PGB_DIR/users.txt
	stats_users = $MONITOR_USER
	logfile = $PGB_DIR/pgbouncer.log
	pidfile = $PGB_DIR/pgbouncer.pid
	pool_mode = transaction
	max_client_conn = 80
	default_pool_size = 20
	reserve_pool_size = 5
	server_idle_timeout = 60
	EOF
    chown -R "$PG_SUPERUSER:$PG_SUPERUSER" "$PGB_DIR"
    start_as "$PG_SUPERUSER" /usr/sbin/pgbouncer "$PGB_DIR/pgbouncer.ini"
    PGB_PID=$STARTED_PID
    wait_for_pgbouncer
    echo "demo-entrypoint: PgBouncer ready (pid $PGB_PID, transaction pooling)"
}

start_kronika() {
    install -d -m 0750 -o kronika -g kronika "$DEMO_ROOT"
    install -d -m 0750 -o kronika -g kronika "$STORAGE_DIR"
    export KRONIKA_DEMO_DIR="$DEMO_ROOT"
    export KRONIKA_STORAGE_DIR="$STORAGE_DIR"
    export KRONIKA_PG_DSNS="host=127.0.0.1 port=$PG_PORT user=$MONITOR_USER dbname=postgres application_name=kronika-demo-monitor"
    export KRONIKA_PGBOUNCER_DSNS="host=127.0.0.1 port=$PGB_PORT user=$MONITOR_USER dbname=pgbouncer"
    export KRONIKA_POSTGRES_EFFECTIVE_CPUS="${KRONIKA_POSTGRES_EFFECTIVE_CPUS:-2}"
    export KRONIKA_PG_INTERVAL_S="${KRONIKA_PG_INTERVAL_S:-5}"
    export KRONIKA_RETENTION="${KRONIKA_RETENTION:-536870912}"
    export KRONIKA_SEGMENT_MAX_BYTES="${KRONIKA_SEGMENT_MAX_BYTES:-16777216}"
    export KRONIKA_JOURNAL_MAX_BYTES="${KRONIKA_JOURNAL_MAX_BYTES:-67108864}"
    export KRONIKA_SEGMENT_MAX_AGE_S="${KRONIKA_SEGMENT_MAX_AGE_S:-300}"
    export KRONIKA_WEB_LISTEN="${KRONIKA_WEB_LISTEN:-0.0.0.0:8080}"
    export KRONIKA_WEB_USER="${KRONIKA_WEB_USER:-demo}"
    export KRONIKA_WEB_PASSWORD="${KRONIKA_WEB_PASSWORD:-forensics}"
    export KRONIKA_WEB_SOURCES="${KRONIKA_WEB_SOURCES:-3}"
    export KRONIKA_WEB_DEMO=synthetic
    export KRONIKA_COLLECTOR_BIN=/usr/local/bin/kronika-collector
    export KRONIKA_DEMO_DURATION_S="${KRONIKA_DEMO_DURATION_S:-0}"
    export KRONIKA_DEMO_COLLECTOR_LOG=stderr
    export KRONIKA_DEMO_WORKLOAD_DSN="${KRONIKA_DEMO_WORKLOAD_DSN:-host=127.0.0.1 port=$PGB_PORT user=$WORKLOAD_USER dbname=$WORKLOAD_DATABASE application_name=kronika-demo-workload}"
    export KRONIKA_DEMO_WORKLOAD_DIRECT_DSN="${KRONIKA_DEMO_WORKLOAD_DIRECT_DSN:-host=127.0.0.1 port=$PG_PORT user=$WORKLOAD_USER dbname=$WORKLOAD_DATABASE application_name=kronika-demo-direct}"
    export KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED="${KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED-true}"
    export KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR="${KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR-$DEMO_ROOT/system-activity}"
    export KRONIKA_DEMO_SYSTEM_CPU_PERCENT="${KRONIKA_DEMO_SYSTEM_CPU_PERCENT-12}"
    export KRONIKA_DEMO_SYSTEM_MEMORY_MIB="${KRONIKA_DEMO_SYSTEM_MEMORY_MIB-32}"
    export KRONIKA_DEMO_SYSTEM_FILE_MIB="${KRONIKA_DEMO_SYSTEM_FILE_MIB-8}"
    export KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S="${KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S-32}"
    export KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S="${KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S-32}"
    export KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S="${KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S-5}"

    start_as kronika /usr/local/bin/kronika-web
    WEB_PID=$STARTED_PID
    echo "demo-entrypoint: kronika-web started (pid $WEB_PID)"
    start_as_with_caps kronika +setuid,+setgid /usr/local/bin/kronika-demo
    DEMO_PID=$STARTED_PID
    echo "demo-entrypoint: kronika-demo started (pid $DEMO_PID)"
}

stop_pid() {
    local signal=$1
    local pid=$2
    if [[ -n $pid ]] && kill -0 "$pid" 2>/dev/null; then
        kill "-$signal" "$pid" 2>/dev/null || true
    fi
}

shutdown() {
    local status=${1:-0}
    if (( SHUTTING_DOWN )); then
        return
    fi
    SHUTTING_DOWN=1
    trap - TERM INT
    echo "demo-entrypoint: shutting down"

    stop_pid TERM "$DEMO_PID"
    [[ -z $DEMO_PID ]] || wait "$DEMO_PID" 2>/dev/null || true
    stop_pid TERM "$WEB_PID"
    stop_pid TERM "$PGB_PID"
    stop_pid INT "$POSTGRES_PID"
    [[ -z $WEB_PID ]] || wait "$WEB_PID" 2>/dev/null || true
    [[ -z $PGB_PID ]] || wait "$PGB_PID" 2>/dev/null || true
    [[ -z $POSTGRES_PID ]] || wait "$POSTGRES_PID" 2>/dev/null || true
    echo "demo-entrypoint: shutdown complete"
    exit "$status"
}

unexpected_exit() {
    local name=$1
    local pid=$2
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        echo "demo-entrypoint: $name exited unexpectedly" >&2
        return 0
    fi
    return 1
}

trap 'shutdown 0' TERM INT
start_postgres
bootstrap_postgres
start_pgbouncer
start_kronika

set +e
wait -n "$POSTGRES_PID" "$PGB_PID" "$WEB_PID" "$DEMO_PID"
set -e
unexpected_exit PostgreSQL "$POSTGRES_PID" \
    || unexpected_exit PgBouncer "$PGB_PID" \
    || unexpected_exit kronika-web "$WEB_PID" \
    || unexpected_exit kronika-demo "$DEMO_PID" \
    || echo "demo-entrypoint: a supervised process exited unexpectedly" >&2
shutdown 1
