#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
toolchain=${DYLINT_TOOLCHAIN:-nightly-2026-05-28}
host=${DYLINT_HOST:-$(rustc "+$toolchain" -vV | sed -n 's/^host: //p')}
lint_crate="$repo_root/lints/kronika_lints"
library_target="$repo_root/target/dylint/libraries/${toolchain}-${host}"
lint_library="$library_target/release/libkronika_lints@${toolchain}-${host}.so"
mordant_library="$library_target/release/libmordant@${toolchain}-${host}.so"
fixture_manifest="$lint_crate/fixtures/Cargo.toml"
dylint_toml=$(<"$repo_root/dylint.toml")
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

unset CARGO_BUILD_TARGET

version=$(cargo dylint --version)
if [[ $version != "cargo-dylint 6.0.4" ]]; then
    echo "expected cargo-dylint 6.0.4, got: $version" >&2
    exit 1
fi

# Running from a neutral directory prevents the repository's musl release
# default from leaking into the host-only lint library.
(
    cd "$scratch"
    env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS cargo "+$toolchain" build \
        --manifest-path "$lint_crate/Cargo.toml" \
        --locked \
        --release \
        --target-dir "$library_target" \
        --config 'target."cfg(all())".rustflags=["-D","warnings","-C","linker=dylint-link"]'
)
test -f "$lint_library"

# Build the single exact Mordant revision declared by the workspace metadata.
# Fixtures and the workspace gate below load this same library file.
(
    cd "$scratch"
    cargo dylint list --all --manifest-path "$repo_root/Cargo.toml" >/dev/null
)
test -f "$mordant_library"

positive="$scratch/positive.json"
if (
    cd "$repo_root"
    CARGO_TARGET_DIR="$repo_root/target" \
    DYLINT_TOML="$dylint_toml" \
    DYLINT_RUSTFLAGS="-D warnings" cargo dylint \
        --no-metadata \
        --lib-path "$lint_library" \
        --lib-path "$mordant_library" \
        --manifest-path "$fixture_manifest" \
        --pipe-stdout "$positive" \
        -- \
        --bin positive \
        --locked \
        --target "$host" \
        --message-format=json
); then
    echo "positive lint fixture unexpectedly passed" >&2
    exit 1
fi

python3 - "$positive" <<'PY'
import collections
import json
import sys

codes = collections.Counter()
with open(sys.argv[1], encoding="utf-8") as stream:
    for line in stream:
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        diagnostic = message.get("message", {})
        code = diagnostic.get("code") or {}
        name = code.get("code")
        if name is not None:
            codes[name] += 1

expected = collections.Counter(
    identity_enum_match=1,
    borrowed_forwarding_closure=1,
    discarded_error=1,
    same_match_twice=1,
    reimplemented_helper=1,
    bare_bool_args=1,
)
if codes != expected:
    raise SystemExit(f"unexpected positive fixture diagnostics: {codes!r}")
PY

negative="$scratch/negative.json"
(
    cd "$repo_root"
    CARGO_TARGET_DIR="$repo_root/target" \
    DYLINT_TOML="$dylint_toml" \
    DYLINT_RUSTFLAGS="-D warnings" cargo dylint \
        --no-metadata \
        --lib-path "$lint_library" \
        --lib-path "$mordant_library" \
        --manifest-path "$fixture_manifest" \
        --pipe-stdout "$negative" \
        -- \
        --bin negative \
        --locked \
        --target "$host" \
        --message-format=json
)

python3 - "$negative" <<'PY'
import json
import sys

for line in open(sys.argv[1], encoding="utf-8"):
    try:
        message = json.loads(line)
    except json.JSONDecodeError:
        continue
    diagnostic = message.get("message", {})
    code = (diagnostic.get("code") or {}).get("code")
    if code in {
        "identity_enum_match",
        "borrowed_forwarding_closure",
        "same_match_twice",
        "reimplemented_helper",
        "bare_bool_args",
        "discarded_error",
    }:
        raise SystemExit(f"negative lint fixture emitted {code}")
PY

cd "$repo_root"
DYLINT_TOML="$dylint_toml" DYLINT_RUSTFLAGS="-D warnings" cargo dylint \
    --no-metadata \
    --lib-path "$lint_library" \
    --lib-path "$mordant_library" \
    --workspace \
    -- \
    --all-targets \
    --locked \
    --target "$host"
