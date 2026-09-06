#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

usage() {
  cat <<'USAGE'
Usage: scripts/package-release.sh [--target TARGET] [--bin-dir DIR] [--output-dir DIR]

Build and package all four static Linux binaries; publish nothing.
--target TARGET  x86_64-unknown-linux-musl (default) or aarch64-unknown-linux-musl.
--bin-dir DIR     Package existing binaries instead of compiling them.
--output-dir DIR  Destination for the archive and its checksum (default: dist).
USAGE
}

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
bin_dir=
output_dir="$repo/dist"
target=x86_64-unknown-linux-musl
binaries=(kronika-collector kronika-web kronika-dump kronika-report)
while (($#)); do
  case "$1" in
    --target|--bin-dir|--output-dir)
      if (($# < 2)) || [[ -z "$2" ]]; then usage >&2; exit 2; fi
      case "$1" in
        --target) target=$2 ;;
        --bin-dir) bin_dir=$2 ;;
        --output-dir) output_dir=$2 ;;
      esac
      shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
case "$target" in
  x86_64-unknown-linux-musl) machine='Advanced Micro Devices X86-64'; cpu=x86-64 ;;
  aarch64-unknown-linux-musl) machine=AArch64; cpu=generic ;;
  *) echo "Unsupported release target: $target" >&2; exit 2 ;;
esac

if [[ -n $(git -C "$repo" status --porcelain --untracked-files=normal) ]]; then
  echo 'Commit or set aside checkout changes before packaging.' >&2
  exit 1
fi
(cd "$repo" && sha256sum --check licenses/dependency-inputs.sha256)
revision=$(git -C "$repo" rev-parse HEAD)
epoch=$(git -C "$repo" show -s --format=%ct HEAD)
version=$(python3 - "$repo/Cargo.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], 'rb') as source:
    print(tomllib.load(source)['workspace']['package']['version'])
PY
)
[[ "$version" =~ ^[0-9A-Za-z.+-]+$ ]] || { echo 'Invalid package version' >&2; exit 1; }
name="kronika-$version-$target"
build_mode=prebuilt
if [[ -z "$bin_dir" ]]; then
  build_mode=source
  [[ $(uname -m) == "${target%%-*}" ]] || {
    echo "Build $target on a native ${target%%-*} Linux machine." >&2; exit 1;
  }
  # Use the target's baseline ISA even on newer CI processors.
  export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=$cpu"
  build_cflags=${CFLAGS_x86_64_unknown_linux_musl:-}
  if [[ "$target" == aarch64-unknown-linux-musl ]]; then
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
    export CC_aarch64_unknown_linux_musl=musl-gcc
    # Avoid native libgcc outline helpers that depend on glibc symbols.
    export CFLAGS_aarch64_unknown_linux_musl="${CFLAGS_aarch64_unknown_linux_musl:-} -mno-outline-atomics"
    build_cflags=$CFLAGS_aarch64_unknown_linux_musl
  fi
  packages=()
  for binary in "${binaries[@]}"; do packages+=(-p "$binary"); done
  build_command=(cargo build --release --locked --target "$target" "${packages[@]}")
  build_compiler=$(cd "$repo" && "${RUSTC:-rustc}" --version)
  (
    cd "$repo"
    "${build_command[@]}"
  )
  bin_dir="${CARGO_TARGET_DIR:-$repo/target}/$target/release"
  [[ "$bin_dir" = /* ]] || bin_dir="$repo/$bin_dir"
fi

for binary in "${binaries[@]}"; do
  path="$bin_dir/$binary"
  [[ -f "$path" && -x "$path" && ! -L "$path" ]] || {
    echo "Missing regular executable: $path" >&2; exit 1;
  }
  readelf -h "$path" | grep -q "Machine:.*$machine"
  if readelf -l "$path" | grep -q INTERP || readelf -d "$path" | grep -q NEEDED; then
    echo "Not a static executable: $path" >&2
    exit 1
  fi
done

mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd)
archive="$output_dir/$name.tar.gz"
[[ ! -e "$archive" && ! -e "$archive.sha256" ]] || {
  echo "Output already exists: $archive or its checksum" >&2; exit 1;
}
package_tmp=$(mktemp -d "$output_dir/.kronika-package.XXXXXX")
trap 'rm -rf -- "$package_tmp"' EXIT
stage="$package_tmp/$name"
mkdir -p "$stage/licenses"
for binary in "${binaries[@]}"; do install -m 0755 "$bin_dir/$binary" "$stage/"; done
install -m 0644 "$repo/LICENSE" "$repo/INSTALL.md" "$repo/INSTALL.ru.md" "$stage/"
install -m 0644 "$repo/THIRD_PARTY_LICENSES.html" "$stage/"
install -m 0644 "$repo/licenses/dependency-inputs.sha256" "$stage/licenses/"
python3 - "$repo" "$stage" "$revision" <<'PY'
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
from urllib.parse import quote, unquote, urlsplit

repo, stage = map(Path, sys.argv[1:3])
revision = sys.argv[3]
repository = tomllib.loads((repo / 'Cargo.toml').read_text())['workspace']['package']['repository']
files = subprocess.check_output(['git', '-C', repo, 'ls-files', '-z']).decode().split('\0')
for name in filter(None, files):
    path = Path(name)
    documentation = (
        name in ('README.md', 'README.ru.md')
        or (path.parts[0] == 'docs'
            and path.suffix in ('.md', '.png', '.svg', '.drawio'))
        or (len(path.parts) == 3 and path.parts[0] == 'bins'
            and path.name in ('README.md', 'README.ru.md'))
        or (len(path.parts) == 3 and path.parts[0] == 'crates'
            and path.parts[1] in ('kronika-format', 'kronika-writer', 'kronika-layout')
            and path.name in ('README.md', 'README.ru.md'))
    )
    if documentation:
        target = stage / path
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(repo / path, target)

# Keep bundled guides local; source references open the packaging revision.
for document in sorted(stage.rglob('*.md')):
    def source_link(match):
        href = match[1]
        url = urlsplit(href)
        if url.scheme or url.netloc or not url.path:
            return match[0]
        target = (document.parent / unquote(url.path)).resolve()
        relative = target.relative_to(stage.resolve())
        source = repo / relative
        if target.exists() or not source.exists():
            return match[0]
        kind = 'tree' if source.is_dir() else 'blob'
        link = f'{repository}/{kind}/{revision}/{quote(relative.as_posix())}'
        if url.fragment:
            link += '#' + url.fragment
        return '](' + link + ')'
    document.write_text(re.sub(r'\]\(([^\s)]+)\)', source_link, document.read_text()))
PY
install -m 0644 "$repo/bins/kronika-web/ui/assets/IBMPlexSans-LICENSE.txt" \
  "$repo/bins/kronika-web/ui/assets/JetBrainsMono-LICENSE.txt" "$stage/licenses/"
printf 'version=%s\npackage_source_revision=%s\ntarget=%s\nsource_date_epoch=%s\nbuild_mode=%s\n' \
  "$version" "$revision" "$target" "$epoch" "$build_mode" > "$stage/BUILDINFO"
if [[ "$build_mode" == source ]]; then
  (
    cd "$repo"
    printf 'build_command='
    printf '%q ' "${build_command[@]}"
    printf '\ncompiler=%s\nrustflags=%s\ncflags=%s\ntarget_cflags=%s\narch_cflags=%s\n' \
      "$build_compiler" "$RUSTFLAGS" "${CFLAGS:-}" "${TARGET_CFLAGS:-}" "$build_cflags"
  ) >> "$stage/BUILDINFO"
fi
(
  cd "$stage"
  find . -type f -print0 | sort -z | xargs -0 sha256sum > "$package_tmp/SHA256SUMS"
)
mv -- "$package_tmp/SHA256SUMS" "$stage/"
tar --sort=name --format=gnu --mtime="@$epoch" --owner=0 --group=0 \
  --numeric-owner --mode='u+rwX,go+rX,go-w' -C "$package_tmp" -cf - "$name" \
  | gzip -n -9 > "$package_tmp/archive.tar.gz"
mv -- "$package_tmp/archive.tar.gz" "$archive"
(
  cd "$output_dir"
  sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
)
printf '%s\n' "$archive" "$archive.sha256"
