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
cleanup() {
  local pids
  pids=$(ps -eo pid,comm | awk -v n="$name" '$2 ~ /^kronika-collect/ {print $1}')
  [[ -z $pids ]] || kill $pids 2>/dev/null || true
  docker rm -f "$name" >/dev/null 2>&1 || true
  rm -rf "$storage"
}
trap cleanup EXIT

docker run -d --rm --name "$name" \
  -e POSTGRES_PASSWORD=golden -e POSTGRES_USER=monitor -e POSTGRES_DB=appdb \
  -p "127.0.0.1:${port}:5432" postgres:16-alpine >/dev/null
for _ in $(seq 1 30); do
  docker exec "$name" pg_isready -U monitor -d appdb >/dev/null 2>&1 && break
  sleep 1
done
docker exec "$name" psql -U monitor -d appdb -c "create database seconddb;" >/dev/null

export PATH="$HOME/.cargo/bin:$PATH"
(cd "$repo" && cargo build --quiet --bin kronika-collector --target x86_64-unknown-linux-musl)
collector="$repo/target/x86_64-unknown-linux-musl/debug/kronika-collector"

setsid nohup "$collector" \
  --storage-dir "$storage" \
  --pg-dsn "host=127.0.0.1 port=$port user=monitor password=golden dbname=appdb" \
  --prometheus-listen "127.0.0.1:$metrics_port" \
  --interval-s 5 >/dev/null 2>&1 &
sleep 12

sanitize() {
  # values and timestamps vary with the live server and clock; the golden
  # pins families, labels, ordering, escaping and the presence of timestamps
  sed -E \
    -e 's/(pgwatch_[a-z_]+(\{[^}]*\})?) -?[0-9.e+-]+ [0-9]+$/\1 <value> <ts>/' \
    -e 's/(kronika_(start_time_seconds|prometheus_(last_fetch_timestamp|fetch_duration)_seconds[a-z_{}=",0-9]*) )[-0-9.e+]+$/\1 <value>/' \
    -e 's/(pgwatch_exporter_total_scrapes) [0-9]+$/\1 <n>/' \
    -e 's/(pgwatch_exporter_last_scrape_errors) [0-9]+$/\1 <n>/' \
    -e 's/(sys_id=")[0-9]+(")/\1<sysid>\2/' \
    -e 's/((dbname|database)=")127[0-9.]+_/\1<host>_/'
}

scraped=$(mktemp)
curl -sf "http://127.0.0.1:$metrics_port/metrics" | sanitize > "$scraped"
if diff -u "$golden" "$scraped"; then
  echo "golden OK: $(grep -c '^pgwatch_' "$scraped") pgwatch_ sample lines"
else
  echo "golden mismatch: update $golden after reviewing the diff" >&2
  exit 1
fi
