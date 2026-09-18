#!/usr/bin/env bash
# Reproduce the browser bindings embedded by kronika-report.
set -euo pipefail

cd "$(dirname "$0")/.."

MODE=${1:-build}
DOWNLOAD=${2:-}
TOOLCHAIN=${RUST_TOOLCHAIN:-1.96.0}
CARGO=${CARGO_BIN:-cargo}
NODE=${NODE_BIN:-node}
CARGO_HOME_PATH=${CARGO_HOME:-$HOME/.cargo}
BINDGEN_VERSION=0.2.127
BINDGEN_ARCHIVE=wasm-bindgen-${BINDGEN_VERSION}-x86_64-unknown-linux-musl.tar.gz
BINDGEN_SHA256=61d4a7dc85acfa0d2354ccc0b8361928c7e52a746d17f28ebaa795ed3dc1614a
# zstd's C code changes the WASM bytes when compiled by a different Clang.
CLANG_VERSION=18.1.3
ASSET_DIRECTORY=crates/kronika-report/assets
JAVASCRIPT_ASSET=${ASSET_DIRECTORY}/kronika-report-wasm.js
WASM_ASSET=${ASSET_DIRECTORY}/kronika-report-wasm.wasm.gz

validate_javascript() {
	local javascript=$1
	if [[ ! -f $javascript ]]; then
		echo "missing generated browser bindings: $javascript" >&2
		exit 1
	fi
	if grep -Eiq '</script|sourceMappingURL' "$javascript"; then
		echo "generated browser bindings are unsafe inside an inline script" >&2
		exit 1
	fi
	if grep -Eq '\b(fetch|Request|URL)\b' "$javascript"; then
		echo "generated browser bindings retain a network loader" >&2
		exit 1
	fi
	if ! grep -q 'KronikaReportWasm' "$javascript" || ! grep -q 'initEmbedded' "$javascript"; then
		echo "generated browser bindings do not expose the embedded initializer" >&2
		exit 1
	fi
	if grep -q 'WebAssembly\.Instance' "$javascript"; then
		echo "generated browser bindings retain synchronous instantiation" >&2
		exit 1
	fi
}

validate_gzip() {
	local compressed=$1
	if [[ ! -f $compressed ]]; then
		echo "missing compressed WebAssembly: $compressed" >&2
		exit 1
	fi
	gzip --test "$compressed"
	if [[ $(od -An -tu1 -N8 "$compressed" | tr -s ' ' | sed 's/^ //; s/ $//') != "31 139 8 0 0 0 0 0" ]]; then
		echo "generated WebAssembly gzip has a non-deterministic header" >&2
		exit 1
	fi
}

validate_wasm_paths() {
	local wasm=$1
	if grep -aFq "$CARGO_HOME_PATH" "$wasm" || grep -aFq "$(pwd)" "$wasm" ||
		grep -aFq "$rustc_sysroot" "$wasm"; then
		echo "generated WebAssembly retains an absolute build path" >&2
		exit 1
	fi
}

if [[ $MODE != build && $MODE != check ]]; then
	echo "usage: $0 {build|check} [--download-bindgen]" >&2
	exit 2
fi
if [[ -n $DOWNLOAD && $DOWNLOAD != --download-bindgen ]]; then
	echo "usage: $0 {build|check} [--download-bindgen]" >&2
	exit 2
fi

# Rust type IDs and symbol hashes differ between host toolchains, even for WASM.
rustc_info=$(rustc +"$TOOLCHAIN" --version --verbose)
rustc_host=$(sed -n 's/^host: //p' <<<"$rustc_info")
if [[ $rustc_host != x86_64-unknown-linux-gnu ]]; then
	echo "report assets require an x86_64 Linux GNU Rust host; use x86_64 Ubuntu 24.04" >&2
	exit 1
fi
rustc_commit=$(sed -n 's/^commit-hash: //p' <<<"$rustc_info")
rustc_sysroot=$(rustc +"$TOOLCHAIN" --print sysroot)

if [[ $MODE == check ]]; then
	validate_javascript "$JAVASCRIPT_ASSET"
	validate_gzip "$WASM_ASSET"
fi

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT

bindgen=${WASM_BINDGEN:-}
if [[ -z $bindgen ]]; then
	bindgen=$(command -v wasm-bindgen || true)
fi
if [[ -z $bindgen && $DOWNLOAD == --download-bindgen ]]; then
	archive=$temporary/$BINDGEN_ARCHIVE
	curl --fail --location --retry 3 --silent --show-error \
		"https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${BINDGEN_VERSION}/${BINDGEN_ARCHIVE}" \
		--output "$archive"
	printf '%s  %s\n' "$BINDGEN_SHA256" "$archive" | sha256sum --check --strict
	tar --extract --gzip --file "$archive" --directory "$temporary"
	bindgen=$temporary/wasm-bindgen-${BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen
fi
if [[ -z $bindgen ]]; then
	echo "wasm-bindgen ${BINDGEN_VERSION} is required; set WASM_BINDGEN or pass --download-bindgen" >&2
	exit 1
fi
if [[ $("$bindgen" --version) != "wasm-bindgen ${BINDGEN_VERSION}" ]]; then
	echo "expected wasm-bindgen ${BINDGEN_VERSION}, got: $("$bindgen" --version)" >&2
	exit 1
fi

export CC_wasm32_unknown_unknown=${CC_wasm32_unknown_unknown:-clang-18}
export AR_wasm32_unknown_unknown=${AR_wasm32_unknown_unknown:-llvm-ar-18}
if [[ $("$CC_wasm32_unknown_unknown" -dumpversion) != "$CLANG_VERSION" ]]; then
	echo "report assets require Clang ${CLANG_VERSION}; set CC_wasm32_unknown_unknown to its executable" >&2
	exit 1
fi
remap_flags="--remap-path-prefix=${CARGO_HOME_PATH}=/cargo-home"
remap_flags+=$'\x1f'
remap_flags+="--remap-path-prefix=$(pwd)=/workspace"
# Installing rust-src otherwise replaces /rustc paths with local toolchain paths.
remap_flags+=$'\x1f'
remap_flags+="--remap-path-prefix=${rustc_sysroot}/lib/rustlib/src/rust=/rustc/${rustc_commit}"
# Ignore local C flags and Cargo's incremental setting when reproducing assets.
env -u CFLAGS -u RUSTFLAGS \
	-u TARGET_CFLAGS -u CFLAGS_wasm32-unknown-unknown \
	-u CC_wasm32-unknown-unknown -u AR_wasm32-unknown-unknown \
	-u ARFLAGS -u TARGET_ARFLAGS -u ARFLAGS_wasm32_unknown_unknown \
	-u ARFLAGS_wasm32-unknown-unknown -u CRATE_CC_NO_DEFAULTS \
	CFLAGS_wasm32_unknown_unknown=-fno-ident \
	CONST_RANDOM_SEED=kronika-report-wasm-v1 \
	CARGO_INCREMENTAL=0 \
	CARGO_ENCODED_RUSTFLAGS="$remap_flags" \
	"$CARGO" +"$TOOLCHAIN" build --locked --release \
	--target wasm32-unknown-unknown \
	-p kronika-report-wasm

target_directory=${CARGO_TARGET_DIR:-target}
raw_wasm=$target_directory/wasm32-unknown-unknown/release/kronika_report_wasm.wasm
generated=$temporary/generated
mkdir -p "$generated"
"$bindgen" \
	--target web \
	--no-typescript \
	--out-dir "$generated" \
	--out-name kronika-report-wasm \
	"$raw_wasm"

module_javascript=$generated/kronika-report-wasm.js
javascript=$temporary/kronika-report-wasm.js
bound_wasm=$generated/kronika-report-wasm_bg.wasm
compressed_wasm=$temporary/kronika-report-wasm.wasm.gz
"$NODE" bins/kronika-web/ui/scripts/bundle-report-wasm.mjs \
	"$module_javascript" \
	"$javascript" \
	"$bound_wasm" \
	"$compressed_wasm"
validate_wasm_paths "$bound_wasm"

validate_javascript "$javascript"
validate_gzip "$compressed_wasm"

printf 'kronika-report-wasm raw=%s gzip=%s js=%s\n' \
	"$(wc -c <"$bound_wasm")" \
	"$(wc -c <"$compressed_wasm")" \
	"$(wc -c <"$javascript")"
sha256sum "$bound_wasm" "$compressed_wasm" "$javascript"

if [[ $MODE == check ]]; then
	cmp "$javascript" "$JAVASCRIPT_ASSET"
	cmp "$compressed_wasm" "$WASM_ASSET"
else
	install -m 0644 "$javascript" "$JAVASCRIPT_ASSET"
	install -m 0644 "$compressed_wasm" "$WASM_ASSET"
fi
