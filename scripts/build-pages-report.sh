#!/usr/bin/env bash
# Build and exercise the self-contained report served at the Pages root.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# -ne 1 || $1 != *.html ]]; then
	echo "usage: $0 OUTPUT.html" >&2
	exit 2
fi

target=${CARGO_BUILD_TARGET:-x86_64-unknown-linux-gnu}
report_bin=${KRONIKA_REPORT_BIN:-${CARGO_TARGET_DIR:-target}/$target/release/kronika-report}
fixture_dir=bins/kronika-demo/fixtures
fixture=$fixture_dir/github-pages-hour.zms
slice=$fixture_dir/github-pages-hour.slice
checksum=$fixture_dir/github-pages-hour.zms.sha256
output=$1

if [[ ! -x $report_bin ]]; then
	echo "report generator is not executable: $report_bin" >&2
	exit 1
fi
if [[ ! -f $fixture || ! -f $slice || ! -f $checksum ]]; then
	echo "Pages demo recording files are missing" >&2
	exit 1
fi

mapfile -t requested <"$slice"
if [[ ${#requested[@]} -ne 2 ]]; then
	echo "Pages slice file must contain its two inclusive endpoints" >&2
	exit 1
fi
from_seconds=$(date --date="${requested[0]}" +%s)
to_seconds=$(date --date="${requested[1]}" +%s)
from_us=$((from_seconds * 1000000))
to_exclusive_us=$(((to_seconds + 1) * 1000000))
duration_us=$((to_exclusive_us - from_us))
if ((duration_us <= 0)); then
	echo "Pages slice must end after it starts" >&2
	exit 1
fi

(cd "$fixture_dir" && sha256sum --check --strict "$(basename "$checksum")")

temporary=$(mktemp -d "${TMPDIR:-/tmp}/kronika-pages-report.XXXXXX")
trap 'rm -rf -- "$temporary"' EXIT

"$report_bin" --from "$from_us" --to-exclusive "$to_exclusive_us" "$fixture" "$output"
"$report_bin" --from "$from_us" --to-exclusive "$to_exclusive_us" "$fixture" "$temporary/index.html"
cmp "$output" "$temporary/index.html"

"${NODE_BIN:-node}" --input-type=module - "$output" "$from_us" "$to_exclusive_us" <<'NODE'
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"

const [output, from, toExclusive] = process.argv.slice(2)
const html = readFileSync(output, "utf8")
const range = /__KRONIKA_REPORT_RUNTIME__=\{visibleFrom:"([^"]+)",visibleToExclusive:"([^"]+)"/.exec(html)
assert.deepEqual(range?.slice(1), [from, toExclusive], "Pages report must use the requested slice bounds")
NODE

scripts/report-browser-smoke.sh "$output"

printf 'pages-report from=%s to=%s duration_us=%s bytes=%s sha256=%s\n' \
	"${requested[0]}" \
	"${requested[1]}" \
	"$duration_us" \
	"$(wc -c <"$output")" \
	"$(sha256sum "$output" | cut -d ' ' -f 1)"
