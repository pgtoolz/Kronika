#!/usr/bin/env bash
set -euo pipefail

PG_BIN=/usr/lib/postgresql/15/bin
MONITOR_USER=kronika_monitor
WEB_USER=${KRONIKA_WEB_USER:-demo}
WEB_PASSWORD=${KRONIKA_WEB_PASSWORD:-forensics}

"$PG_BIN/pg_isready" --quiet --host=127.0.0.1 --port=5432 --username="$MONITOR_USER" --dbname=postgres
"$PG_BIN/psql" --no-psqlrc --set=ON_ERROR_STOP=1 \
    --host=127.0.0.1 --port=6432 --username="$MONITOR_USER" --dbname=pgbouncer \
    --command='SHOW VERSION' >/dev/null
catalog=$(curl --compressed --fail --silent --show-error --max-time 8 \
    --user "$WEB_USER:$WEB_PASSWORD" http://127.0.0.1:8080/api/catalog)
missing=()
for section in os_process pg_stat_activity pg_stat_statements pg_store_plans pg_locks pg_stat_progress_vacuum pgbouncer_events; do
    if ! grep -Fq "\"logical_name\":\"$section\"" <<<"$catalog"; then
        missing+=("$section")
    fi
done
if (( ${#missing[@]} )); then
    echo "demo-healthcheck: catalog is missing required sections: ${missing[*]}" >&2
    exit 1
fi
