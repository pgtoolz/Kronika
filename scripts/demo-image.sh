#!/usr/bin/env bash
# Builds and manages the Compose-based interactive demo.
set -euo pipefail

cd "$(dirname "$0")/.."

export DEMO_IMAGE=${DEMO_IMAGE:-kronika-demo:local}
export DEMO_VCS_REF=${DEMO_VCS_REF:-$(git rev-parse --verify HEAD 2>/dev/null || printf unknown)}
DEMO_PORT=${DEMO_PORT:-8080}
DEMO_BIND_ADDRESS=${DEMO_BIND_ADDRESS:-127.0.0.1}
export DEMO_PORT DEMO_BIND_ADDRESS
COMPOSE=(docker compose --file compose.demo.yml)

deps_key() {
    {
        cat Cargo.toml Cargo.lock rust-toolchain.toml
        find crates bins -name Cargo.toml -print0 | sort -z | xargs -0 cat
    } | sha256sum | cut -d' ' -f1
}

case "${1:-up}" in
deps-key)
    deps_key
    ;;
build)
    "${COMPOSE[@]}" build
    ;;
up|run)
    "${COMPOSE[@]}" up --build --detach --wait --wait-timeout "${DEMO_WAIT_TIMEOUT:-240}"
    echo "Kronika demo: http://${DEMO_BIND_ADDRESS}:${DEMO_PORT}/ (demo / forensics)"
    ;;
stop)
    "${COMPOSE[@]}" stop
    ;;
clean)
    "${COMPOSE[@]}" down --volumes --remove-orphans
    ;;
status)
    "${COMPOSE[@]}" ps
    container=$("${COMPOSE[@]}" ps --quiet kronika)
    if [[ -n $container ]]; then
        docker inspect --format '{{json .State.Health}}' "$container"
    fi
    ;;
logs)
    "${COMPOSE[@]}" logs --follow
    ;;
*)
    echo "usage: $0 {deps-key|build|up|run|stop|clean|status|logs}" >&2
    exit 2
    ;;
esac
