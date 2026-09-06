#!/usr/bin/env bash
# Build and run the BDD image.
#
#   ./scripts/bdd-image.sh deps-key   # cache key of the dependency layer
#   ./scripts/bdd-image.sh build      # build the image
#   ./scripts/bdd-image.sh run        # build, then run the scenarios
#
# The dependency layer is keyed on the manifests and the lockfile, so a
# source-only change reuses it.
set -euo pipefail

cd "$(dirname "$0")/.."

IMAGE="${BDD_IMAGE:-kronika-bdd:local}"

deps_key() {
	# Same inputs the dependency stage copies.
	{
		cat Cargo.toml Cargo.lock rust-toolchain.toml
		find crates bins -name Cargo.toml -print0 | sort -z | xargs -0 cat
	} | sha256sum | cut -d' ' -f1
}

case "${1:-run}" in
deps-key)
	deps_key
	;;
build)
	docker build --file Dockerfile.bdd --tag "$IMAGE" .
	;;
run)
	docker build --file Dockerfile.bdd --tag "$IMAGE" .
	# --cpus is part of the contract: container.feature asserts the collector
	# records this quota and not the host's core count.
	docker run --rm --cpus=2 "$IMAGE"
	;;
*)
	echo "usage: $0 {deps-key|build|run}" >&2
	exit 2
	;;
esac
