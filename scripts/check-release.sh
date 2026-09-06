#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

mode=full
case "${1:-}" in
  --cli-only) mode=cli; shift ;;
  --no-browser) mode=native; shift ;;
esac
if [[ $# -ne 1 || ! -f "$1" ]]; then
  echo "Usage: scripts/check-release.sh [--cli-only|--no-browser] ARCHIVE.tar.gz" >&2
  exit 2
fi
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
archive=$(realpath "$1")
(cd "$(dirname "$archive")" && sha256sum --check "$(basename "$archive").sha256")
release_tmp=$(mktemp -d "${TMPDIR:-/tmp}/kronika-release-check.XXXXXX")
trap 'rm -rf -- "$release_tmp"' EXIT
tar -xzf "$archive" -C "$release_tmp"
package="$release_tmp/$(basename "$archive" .tar.gz)"
(cd "$package" && sha256sum --check SHA256SUMS)
for required in kronika-collector kronika-web kronika-dump kronika-report LICENSE README.md README.ru.md INSTALL.md INSTALL.ru.md docs/storage-recovery.md docs/storage-recovery.ru.md BUILDINFO THIRD_PARTY_LICENSES.html; do
  test -s "$package/$required"
done
target=$(sed -n 's/^target=//p' "$package/BUILDINFO")
case "$target" in
  x86_64-unknown-linux-musl) machine='Advanced Micro Devices X86-64' ;;
  aarch64-unknown-linux-musl) machine=AArch64 ;;
  *) echo "Unsupported archive target: $target" >&2; exit 1 ;;
esac
for binary in "$package"/kronika-*; do
  test -f "$binary" && test -x "$binary" && test ! -L "$binary"
  readelf -h "$binary" | grep -q "Machine:.*$machine"
  if readelf -l "$binary" | grep -q INTERP || readelf -d "$binary" | grep -q NEEDED; then
    echo "Not a static executable: $binary" >&2
    exit 1
  fi
done

python3 - "$repo" "$package" "$release_tmp" "$mode" <<'PY'
import base64
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from urllib.parse import unquote, urlsplit

repo, package, scratch = map(Path, sys.argv[1:4])
mode = sys.argv[4]
members = {p.relative_to(package).as_posix() for p in package.rglob('*') if p.is_file()}
assert {p.name for p in package.glob('kronika-*')} == {
    'kronika-collector', 'kronika-web', 'kronika-dump', 'kronika-report',
}, 'archive must contain exactly the four product programs'
checksummed = {line.split('  ', 1)[1].removeprefix('./')
               for line in (package / 'SHA256SUMS').read_text().splitlines()}
assert checksummed == members - {'SHA256SUMS'}, 'checksum manifest omits package files'
for path in (repo / 'docs').rglob('*'):
    if path.is_file() and path.suffix in ('.md', '.png', '.svg', '.drawio'):
        assert path.relative_to(repo).as_posix() in members, f'missing documentation: {path}'
for directory in ('bins/kronika-collector', 'bins/kronika-web', 'bins/kronika-dump',
                  'bins/kronika-report', 'bins/kronika-demo', 'crates/kronika-format',
                  'crates/kronika-writer', 'crates/kronika-layout'):
    for name in ('README.md', 'README.ru.md'):
        assert f'{directory}/{name}' in members
for member in members:
    assert not set(Path(member).parts) & {'src', 'target', 'node_modules', '.git'}, member
for document in package.rglob('*.md'):
    for href in re.findall(r'\]\(([^\s)]+)\)', document.read_text()):
        url = urlsplit(href)
        if url.scheme or url.netloc or not url.path:
            continue
        target = (document.parent / unquote(url.path)).resolve()
        assert target.exists(), f'broken bundled link: {document}: {href}'
print(f'Archive documentation, static ELF and full checksum manifest passed: {len(members)} files')
cli_check = [sys.executable, repo / 'scripts/check-cli.py', package]
if mode != 'cli':
    cli_check.append('--strace')
subprocess.run(cli_check, check=True, timeout=120)
if mode == 'cli':
    sys.exit(0)
env = {k: v for k, v in os.environ.items() if not k.startswith('KRONIKA_')}
collector = subprocess.run([package / 'kronika-collector'], env=env,
                           capture_output=True, text=True, timeout=10)
assert collector.returncode != 0 and 'KRONIKA_STORAGE_DIR' in collector.stderr
capture = scratch / 'capture'
capture_env = dict(env, KRONIKA_STORAGE_DIR=str(capture), KRONIKA_INTERVAL_S='1',
                   KRONIKA_SEGMENT_MAX_AGE_S='1')
with (scratch / 'collector.log').open('w+') as log:
    collector = subprocess.Popen([package / 'kronika-collector'], env=capture_env,
                                 stdout=log, stderr=log)
    try:
        deadline = time.monotonic() + 30
        while not list(capture.rglob('*.zms')):
            if collector.poll() is not None or time.monotonic() >= deadline:
                log.seek(0)
                raise RuntimeError('collector did not publish an OS segment:\n' + log.read())
            time.sleep(0.1)
    finally:
        if collector.poll() is None:
            collector.terminate()
        try:
            collector.wait(timeout=10)
        except subprocess.TimeoutExpired:
            collector.kill()
            collector.wait()
    assert collector.returncode == 0, 'collector did not stop cleanly'
captured = subprocess.run([package / 'kronika-dump', capture, '--json'], env=env,
                          capture_output=True, text=True, check=True, timeout=30)
sections = [json.loads(line) for line in captured.stdout.splitlines()]
for name in ('os_cpu', 'os_process'):
    section = next(row for row in sections if row.get('section') == name and row['rows'] > 0)
    decoded = subprocess.run([package / 'kronika-dump', capture, '--json',
                              '--section', str(section['type_id']), '--limit', '1'],
                             env=env, capture_output=True, text=True, check=True, timeout=30)
    rows = [json.loads(line) for line in decoded.stdout.splitlines()]
    assert any(row.get('kind') == 'row' and row['row'] for row in rows), decoded.stdout
store = scratch / 'data'
segment_dir = store / '2024/02/29'
segment_dir.mkdir(parents=True)
fixture = repo / 'bins/kronika-report/tests/fixtures/standalone.zms'
shutil.copyfile(fixture, segment_dir / '1709164800000000.zms')
dump = subprocess.run([package / 'kronika-dump', store, '--json'], env=env,
                      capture_output=True, text=True, check=True, timeout=30)
assert dump.stdout.strip(), 'dump produced no section sizes'
slice_env = dict(env, KRONIKA_STORAGE_DIR=str(store))
subprocess.run([package / 'kronika-dump', 'slice',
                '--from', '2024-02-29T00:00:00Z', '--to', '2024-02-29T00:59:59Z',
                '--out', scratch / 'slice.zms'], env=slice_env, check=True, timeout=30)
for output in ('report.html', 'report-again.html'):
    subprocess.run([package / 'kronika-report',
                    '--from', '1709164800000000', '--to-exclusive', '1709168400000000',
                    scratch / 'slice.zms', scratch / output],
                   env=env, check=True, timeout=30)
assert (scratch / 'report.html').read_bytes() == (scratch / 'report-again.html').read_bytes()

with socket.socket() as probe:
    probe.bind(('127.0.0.1', 0))
    port = probe.getsockname()[1]
web_env = dict(env, KRONIKA_STORAGE_DIR=str(store),
               KRONIKA_WEB_LISTEN=f'127.0.0.1:{port}', KRONIKA_WEB_SOURCES='3',
               KRONIKA_WEB_USER='package-check', KRONIKA_WEB_PASSWORD='local-package-check')
base = f'http://127.0.0.1:{port}'
headers = {'Authorization': 'Basic ' + base64.b64encode(
    b'package-check:local-package-check').decode()}
with (scratch / 'web.log').open('w+') as log:
    web = subprocess.Popen([package / 'kronika-web'], env=web_env, stdout=log, stderr=log)
    try:
        for attempt in range(100):
            if web.poll() is not None:
                log.seek(0)
                raise RuntimeError(log.read())
            try:
                with urllib.request.urlopen(base, timeout=1) as response:
                    assert b'<html' in response.read().lower()
                break
            except (urllib.error.URLError, TimeoutError):
                time.sleep(0.1)
        else:
            raise RuntimeError('web did not become ready')
        try:
            urllib.request.urlopen(base + '/api/catalog', timeout=10)
        except urllib.error.HTTPError as error:
            assert error.code == 401
        else:
            raise AssertionError('catalog accepted missing credentials')
        request = urllib.request.Request(base + '/api/catalog', headers=headers)
        with urllib.request.urlopen(request, timeout=10) as response:
            assert b'os_process' in response.read()
        rpc_headers = dict(headers, **{'Content-Type': 'application/json',
                                      'Accept': 'application/json, text/event-stream'})
        request = urllib.request.Request(base + '/mcp', headers=rpc_headers,
            data=b'{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}')
        with urllib.request.urlopen(request, timeout=10) as response:
            tools = json.load(response)['result']['tools']
            assert any(tool['name'] == 'kronika_rank_metrics' for tool in tools)
    finally:
        if web.poll() is None:
            web.terminate()
        try:
            web.wait(timeout=10)
        except subprocess.TimeoutExpired:
            web.kill()
            web.wait()
print('Package smoke passed: OS collection and clean stop, dump, slice, deterministic report, web auth, MCP')
PY
if [[ "$mode" == full ]]; then
  "$repo/scripts/report-browser-smoke.sh" "$release_tmp/report.html"
fi
