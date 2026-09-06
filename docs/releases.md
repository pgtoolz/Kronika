# Linux archive reference

[Русская версия](releases.ru.md) · [Install](../INSTALL.md)

## Release v1.0.0

[Kronika v1.0.0](https://github.com/pgtoolz/Kronika/releases/tag/v1.0.0)
contains four programs for Linux x86-64 and ARM64.

<a id="download"></a>
## Download

| Architecture | Archive | Checksum |
| --- | --- | --- |
| x86-64 | [kronika-1.0.0-x86_64-unknown-linux-musl.tar.gz](https://github.com/pgtoolz/Kronika/releases/download/v1.0.0/kronika-1.0.0-x86_64-unknown-linux-musl.tar.gz) | [SHA-256](https://github.com/pgtoolz/Kronika/releases/download/v1.0.0/kronika-1.0.0-x86_64-unknown-linux-musl.tar.gz.sha256) |
| ARM64 | [kronika-1.0.0-aarch64-unknown-linux-musl.tar.gz](https://github.com/pgtoolz/Kronika/releases/download/v1.0.0/kronika-1.0.0-aarch64-unknown-linux-musl.tar.gz) | [SHA-256](https://github.com/pgtoolz/Kronika/releases/download/v1.0.0/kronika-1.0.0-aarch64-unknown-linux-musl.tar.gz.sha256) |

Follow the [download and installation commands](../INSTALL.md#1-download-and-extract).
The [HTML example](https://github.com/pgtoolz/Kronika/releases/download/v1.0.0/kronika-v1.0.0.html)
opens offline; it is also [available in the browser](https://pgtoolz.github.io/Kronika/reports/kronika-v1.0.0.html).

<a id="members-and-identity"></a>
## Archive contents and build version

```text
kronika-<cargo-version>-<target>/
  kronika-collector   kronika-web   kronika-dump   kronika-report
  BUILDINFO          SHA256SUMS    LICENSE       THIRD_PARTY_LICENSES.html
  README.md          README.ru.md  INSTALL.md    INSTALL.ru.md
  docs/              bins/        crates/       licenses/
```

The four executables are static Linux ELF files. Documentation includes paired
guides, real PNG figures, light/dark SVG diagrams, editable draw.io sources and
license notices. `bins/` and `crates/` contain linked guides. Links to source
files outside the archive resolve to the packaging commit on GitHub.

| File or field | Meaning |
| --- | --- |
| Filename | Workspace version and target. |
| `BUILDINFO.package_source_revision` | Full commit of the clean packaging checkout. |
| `BUILDINFO.build_mode` | `source`: compiled by the packaging command. `prebuilt`: supplied with `--bin-dir`; binary source/compiler identity is not recorded by this mode. |
| `BUILDINFO.source_date_epoch` | Packaging commit timestamp, Unix seconds. |
| `BUILDINFO` source-build fields | Build command, Rust compiler, Rust flags and C flags. |
| `SHA256SUMS` | SHA-256 of every file except the manifest itself. |
| `<archive>.sha256` | SHA-256 of the compressed archive. |

<a id="native-targets-and-userspace-matrix"></a>
## Architectures and distribution checks

| Target | Native build runner | CPU target |
| --- | --- | --- |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | `x86-64` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | `generic` |

Each build runs on its matching architecture and builds the four programs
once. That same archive is checked in the following distribution environments:

| Userspace | Container image | Architectures |
| --- | --- | --- |
| Ubuntu 22.04 LTS | `ubuntu:22.04` | x86-64, ARM64 |
| Ubuntu 24.04 LTS | `ubuntu:24.04` | x86-64, ARM64 |
| Ubuntu 26.04 LTS | `ubuntu:26.04` | x86-64, ARM64 |
| Debian 12 | `debian:bookworm-slim` | x86-64, ARM64 |
| Debian 13 | `debian:trixie-slim` | x86-64, ARM64 |
| CentOS Stream 9 | `quay.io/centos/centos:stream9` | x86-64, ARM64 |
| CentOS Stream 10 | `quay.io/centos/centos:stream10` | x86-64, ARM64 |
| Fedora 44 | `fedora:44` | x86-64, ARM64 |
| Alpine 3.24 | `alpine:3.24` | x86-64, ARM64 |
| Rocky Linux 9 | `rockylinux/rockylinux:9` | x86-64, ARM64 |
| openSUSE Leap 16.0 | `registry.opensuse.org/opensuse/leap:16.0` | x86-64, ARM64 |

Each row checks archive/member checksums, documentation membership, ELF
architecture, absence of `INTERP`/`NEEDED`, and the four programs' help/version
and argument handling. The `portability-*` artifact records the resolved image
digest, `/etc/os-release`, kernel/architecture and checker output. Containers
use the build machine's kernel; these checks establish execution in the listed
distribution environments with that kernel, not with every Linux kernel. Matrix definition:
[release-package.yml](../.github/workflows/release-package.yml).

## Development builds

With an authenticated [GitHub CLI](https://cli.github.com/manual/gh_run_download),
select a successful workflow run:

```sh
gh run list --repo pgtoolz/Kronika --workflow release-package.yml --status success --limit 10
run_id=REPLACE_WITH_RUN_ID
gh run view "$run_id" --repo pgtoolz/Kronika
source_revision=$(gh run view "$run_id" --repo pgtoolz/Kronika --json headSha --jq .headSha)
target=x86_64-unknown-linux-musl
gh run download "$run_id" --repo pgtoolz/Kronika \
  --name "kronika-$source_revision-$target" --dir kronika-download
cd kronika-download
archive="kronika-1.0.0-$target.tar.gz"
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
```

For ARM64, set `target=aarch64-unknown-linux-musl`. `gh run view` lists both
builds on their matching architectures and the 22 distribution checks. The downloaded artifact contains
`.tar.gz` and `.tar.gz.sha256`; after verification and extraction, continue with
[installation](../INSTALL.md#2-install).

## Package

Requirements: a working copy with all changes committed, Linux on the target
architecture, the
[pinned build toolchain](build.md), GNU tar, gzip, binutils and Python 3.11+.

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

On native ARM64, use `--target aarch64-unknown-linux-musl`. Default target:
`x86_64-unknown-linux-musl`. Default output directory: `dist`.

To package existing static binaries:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl \
  --bin-dir target/x86_64-unknown-linux-musl/release \
  --output-dir dist-review
```

The script rejects dirty checkouts, unsupported targets, dynamic or
wrong-architecture executables and existing output paths. Dependency notices
are checked against `licenses/dependency-inputs.sha256`.

Archive member order, permissions, owner/group, timestamps and gzip metadata are
fixed. Equal binary/document bytes, source revision and build mode produce equal
archives. CI compares two packages of the same binaries byte for byte.
Source: [package-release.sh](../scripts/package-release.sh).

## Checks

With `strace`, Node.js 22 and Chromium/Google Chrome installed, pass one archive:

```sh
scripts/check-release.sh dist/kronika-1.0.0-x86_64-unknown-linux-musl.tar.gz
```

| Mode | Checks |
| --- | --- |
| Default | Archive checks; CLI checks; real OS collection followed by dump; fixture slicing; two identical HTML reports; authenticated web catalog; MCP discovery; direct-file browser interactivity and network-request checks. |
| `--no-browser` | All default checks except the browser step; used by native ARM64 CI. |
| `--cli-only` | Archive and CLI checks; used by each userspace row. |

CLI checks execute unprivileged processes in a read-only working directory with
empty and invalid configuration environments, deadlines and exact stdout,
stderr and exit-status assertions. Native modes also trace help/version calls
for storage, logging, thread, process and network startup. Sources:
[check-release.sh](../scripts/check-release.sh), [check-cli.py](../scripts/check-cli.py).
