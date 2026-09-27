#!/usr/bin/env bash
# Golden exposition check for the collector Prometheus endpoint.
#
# Starts a bounded local PostgreSQL 16 container, runs the collector with
# --prometheus-listen, scrapes /metrics, sanitizes environment-varying
# values, and diffs the result against the committed golden file. Requires
# docker and the repository binaries; leaves nothing behind.
set -euo pipefail
export LC_ALL=C

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
golden="$repo/crates/kronika-prometheus/tests/golden/basic.txt"
name=kronika-prom-golden
port=55433
metrics_port=9197
storage=$(mktemp -d /tmp/kronika-prom-golden.XXXXXX)
collector_pid=
container_id=
cleanup() {
  # only the collector this script started and the container it created
  [[ -z $collector_pid ]] || kill "$collector_pid" 2>/dev/null || true
  [[ -z $container_id ]] || docker rm -f "$container_id" >/dev/null 2>&1 || true
  rm -rf "$storage"
}
trap cleanup EXIT

docker_args=(
  run -d --rm --name "$name"
  -e POSTGRES_PASSWORD=golden -e POSTGRES_USER=monitor -e POSTGRES_DB=appdb
  -p "127.0.0.1:${port}:5432" postgres:16-alpine
)
container_id=$(docker "${docker_args[@]}") || exit
for _ in $(seq 1 30); do
  docker exec "$name" pg_isready -U monitor -d appdb >/dev/null 2>&1 && break
  sleep 1
done
docker exec "$name" psql -U monitor -d appdb -c "create database seconddb;" >/dev/null

export PATH="$HOME/.cargo/bin:$PATH"
(cd "$repo" && cargo build --quiet --bin kronika-collector --target x86_64-unknown-linux-musl)
collector="$repo/target/x86_64-unknown-linux-musl/debug/kronika-collector"

collector_args=(
  --storage-dir "$storage"
  --pg-dsn "host=127.0.0.1 port=$port user=monitor password=golden dbname=appdb"
  --prometheus-listen "127.0.0.1:$metrics_port"
  --interval-s 5
)
setsid nohup "$collector" "${collector_args[@]}" >/dev/null 2>&1 &
collector_pid=$!
sleep 12

sanitize() {
  # values and timestamps vary with the live server and clock; the golden
  # pins families, labels, ordering, escaping and the presence of timestamps
  local rules=(
    -e 's/(pgwatch_[a-z_]+(\{[^}]*\})?) -?[0-9.e+-]+ [0-9]+$/\1 <value> <ts>/'
    -e 's/(kronika_(start_time_seconds|prometheus_(last_fetch_timestamp|fetch_duration)_seconds[_a-z{}=",0-9.]*)) [-0-9.e+]+$/\1 <value>/'
    -e 's/(pgwatch_exporter_total_scrapes) [0-9]+$/\1 <n>/'
    -e 's/(pgwatch_exporter_last_scrape_errors) [0-9]+$/\1 <n>/'
    -e 's/(sys_id=")[0-9]+(")/\1<sysid>\2/'
    -e 's/((dbname|database)=")127[0-9.]+_/\1<host>_/'
  )
  sed -E "${rules[@]}"
}

scraped=$(mktemp)
curl -sf "http://127.0.0.1:$metrics_port/metrics" | sanitize > "$scraped"
if [[ ${UPDATE_GOLDEN:-0} == 1 ]]; then
  cp "$scraped" "$golden"
  echo "golden updated: $(grep -c '^pgwatch_' "$scraped") pgwatch_ sample lines"
elif diff -u "$golden" "$scraped"; then
  echo "golden OK: $(grep -c '^pgwatch_' "$scraped") pgwatch_ sample lines"
else
  echo "golden mismatch: run with UPDATE_GOLDEN=1 after reviewing the diff" >&2
  exit 1
fi
