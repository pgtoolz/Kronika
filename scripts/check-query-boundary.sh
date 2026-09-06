#!/usr/bin/env bash
set -euo pipefail

if ! command -v rg >/dev/null 2>&1; then
    echo "check-query-boundary.sh requires ripgrep (rg) on PATH" >&2
    exit 1
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
crate_root="$repo_root/crates/kronika-query"
source_root="$crate_root/src"
api_root="$repo_root/crates/kronika-api"
api_source_root="$api_root/src"
report_root="$repo_root/bins/kronika-report"
report_source_root="$report_root/src/lib.rs"
wasm_root="$repo_root/crates/kronika-report-wasm"
wasm_source_root="$wasm_root/src"
cargo_bin=${CARGO_BIN:-cargo}
rust_toolchain=${RUST_TOOLCHAIN:-1.96.0}
failed=0

if rg -n -i '\bproduct\b' "$crate_root"; then
    echo "kronika-query must not use product naming" >&2
    failed=1
fi

if rg -n \
    'std::(fs|path)(::|\b)|\b(Path|PathBuf|File|LocalDir|Instant)\b|\b(hyper|tokio|rmcp|http)(::|_)' \
    "$source_root"; then
    echo "kronika-query source crosses its native storage/transport boundary" >&2
    failed=1
fi

if rg -n \
    'std::(fs|net|path)(::|\b)|\b(Path|PathBuf|File|LocalDir|Mutex|Instant)\b|\b(hyper|tokio|rmcp|http|url|reqwest|ureq|oauth2|jsonwebtoken|openidconnect|wasm_bindgen)(::|_)|async[[:space:]]+fn' \
    "$api_source_root"; then
    echo "kronika-api source crosses its portable parsing boundary" >&2
    failed=1
fi

if rg -n \
    'std::(fs|net|path)(::|\b)|\b(Path|PathBuf|File|LocalDir|Mutex|Instant)\b|\b(hyper|tokio|rmcp|http|url|reqwest|ureq|oauth2|jsonwebtoken|openidconnect|wasm_bindgen)(::|_)|async[[:space:]]+fn|^[[:space:]]*(pub[[:space:]]+)?trait[[:space:]]|\bsegment_id[[:space:]]*:[[:space:]]*String\b' \
    "$report_source_root"; then
    echo "kronika-report source crosses its portable composition boundary" >&2
    failed=1
fi

if rg -n \
    '\.(clone|to_vec|to_owned)\(|copy_from_slice|extend_from_slice' \
    "$report_source_root"; then
    echo "kronika-report source copies or clones owned input" >&2
    failed=1
fi

if rg -n \
    'std::(fs|net|path)(::|\b)|\b(Path|PathBuf|File|LocalDir|Mutex|Instant)\b|\b(hyper|tokio|rmcp|http|url|reqwest|ureq|oauth2|jsonwebtoken|openidconnect)(::|_)|async[[:space:]]+fn' \
    "$wasm_source_root"; then
    echo "kronika-report-wasm source crosses its embedded adapter boundary" >&2
    failed=1
fi

if rg -n \
    '\bQueryRequest::|serde_json|\b(zms|idx)[[:space:]]*\.[[:space:]]*(clone|to_vec|to_owned)\(' \
    "$wasm_source_root"; then
    echo "kronika-report-wasm duplicates query dispatch, rendering, or owned input bytes" >&2
    failed=1
fi

dependency_names=$(
    "$cargo_bin" "+$rust_toolchain" tree --locked -p kronika-query \
        --edges normal --prefix none |
        sed -E 's/ v[0-9].*$//' |
        sort -u
)
if printf '%s\n' "$dependency_names" |
    rg '^(http|http-body.*|hyper.*|tokio.*|rmcp.*)$'; then
    echo "kronika-query dependency graph contains a native transport/runtime crate" >&2
    failed=1
fi

api_dependency_names=$(
    "$cargo_bin" "+$rust_toolchain" tree --locked -p kronika-api \
        --edges normal,build --prefix none |
        sed -E 's/ v[0-9].*$//' |
        sort -u
)
if printf '%s\n' "$api_dependency_names" |
    rg '^(http|http-body.*|hyper.*|tokio.*|rmcp.*|url|reqwest.*|ureq|oauth2|jsonwebtoken|openidconnect|native-tls|rustls.*|rustix|errno|socket2|mio|wasm-bindgen.*)$'; then
    echo "kronika-api dependency graph contains a transport/runtime binding" >&2
    failed=1
fi

report_dependency_names=$(
    "$cargo_bin" "+$rust_toolchain" tree --locked -p kronika-report --no-default-features \
        --edges normal --prefix none |
        sed -E 's/ v[0-9].*$//' |
        sort -u
)
if printf '%s\n' "$report_dependency_names" |
    rg '^(http|http-body.*|hyper.*|tokio.*|rmcp.*|url|reqwest.*|ureq|oauth2|jsonwebtoken|openidconnect|native-tls|rustls.*|rustix|errno|socket2|mio|wasm-bindgen.*)$'; then
    echo "kronika-report dependency graph contains a transport/runtime binding" >&2
    failed=1
fi

wasm_dependency_names=$(
    "$cargo_bin" "+$rust_toolchain" tree --locked -p kronika-report-wasm \
        --target wasm32-unknown-unknown --no-default-features \
        --edges normal,build --prefix none |
        sed -E 's/ v[0-9].*$//' |
        sort -u
)
if printf '%s\n' "$wasm_dependency_names" |
    rg '^(http|http-body.*|hyper.*|tokio.*|rmcp.*|url|reqwest.*|ureq|oauth2|jsonwebtoken|openidconnect|native-tls|rustls.*|rustix|errno|socket2|mio)$'; then
    echo "kronika-report-wasm dependency graph contains a transport/runtime binding" >&2
    failed=1
fi

exit "$failed"
