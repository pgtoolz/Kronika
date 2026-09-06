# Build from source

[Русская версия](build.ru.md) · [Install](../INSTALL.md)

You can install a [prebuilt binary archive](../INSTALL.md) or build Kronika
from source using the instructions below.

## Toolchain

| Component | Version or requirement |
| --- | --- |
| Rust | `1.96.0`, pinned by [rust-toolchain.toml](../rust-toolchain.toml). Dependencies use `Cargo.lock`. |
| Build tools | C compiler, GNU make, `pkg-config`; static builds use `musl-gcc` for the host architecture. Debian/Ubuntu packages: `build-essential musl-tools pkg-config`. |
| Browser interface | Cargo uses the HTML and WebAssembly files already stored in the repository; building the Linux programs does not require Node.js. |
| Rebuilding the browser interface | Node.js `22.21.1`, `wasm32-unknown-unknown`, and the npm dependencies selected by the lockfile. |

## Native Linux binaries

With rustup and the build tools installed, clone the repository and enter it:

```sh
git clone https://github.com/pgtoolz/Kronika.git
cd Kronika
```

To build an exact revision, run `git checkout FULL_COMMIT` before `cargo build`;
use `git checkout v1.0.1` for the published release source.

Choose the build command for your machine. On x86-64 Linux:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Outputs: `target/x86_64-unknown-linux-musl/release/kronika-{collector,web,dump,report}`.
The default Cargo target in [.cargo/config.toml](../.cargo/config.toml) is
`x86_64-unknown-linux-musl`.

On an ARM64 Linux machine:

```sh
rustup target add aarch64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
CC_aarch64_unknown_linux_musl=musl-gcc \
CFLAGS_aarch64_unknown_linux_musl=-mno-outline-atomics \
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Outputs: `target/aarch64-unknown-linux-musl/release/`. The C flag emits inline
ARMv8 atomic operations.

Install the four programs from the repository directory. For ARM64, set
`binary_dir=target/aarch64-unknown-linux-musl/release` instead:

```sh
binary_dir=target/x86_64-unknown-linux-musl/release
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 "$binary_dir/kronika-collector" "$binary_dir/kronika-web" \
  "$binary_dir/kronika-dump" "$binary_dir/kronika-report" /usr/local/bin/
```

Then [start collection](../INSTALL.md#3-start-collector). The checks and browser
asset rebuild below are for development; they are not steps in a normal installation.

To build all programs, including the development demo, for an x86-64 Linux
machine using the GNU tools:

```sh
make build TARGET=x86_64-unknown-linux-gnu
```

`make build` defaults to rustc's host target.

## Checks

```sh
export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu
make fmt-check lint test
```

`lint` runs repository/Mordant Dylint rules and workspace Clippy with warnings
denied. [Dylint setup](../scripts/check-dylints.sh) defines its dependencies.
`test` runs unit and integration tests. Behavior tests (BDD) run separately in
GitHub Actions.

## Browser assets

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.96.0
make ui-install
make report-assets REPORT_ASSET_FLAGS=--download-bindgen
make report-assets-check REPORT_ASSET_FLAGS=--download-bindgen
```

These commands rebuild the web page, the HTML report interface and its
WebAssembly engine, then check that they match the files in the repository. The report's query engine executes on the browser's main
thread. Target definitions: [Makefile](../Makefile).

## Archive

From a clean committed checkout:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

[Archive reference](releases.md) defines targets, output names, build metadata,
checksums, packaged documents and CI checks.
