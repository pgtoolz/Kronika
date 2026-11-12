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

Choose the build command for your machine. On x86-64 Linux:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report-cli
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
  -p kronika-report-cli
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

Regenerate the report WebAssembly on x86_64 Ubuntu 24.04 with the
`x86_64-unknown-linux-gnu` Rust host. Install Clang 18.1.3 and `llvm-ar-18`
with `sudo apt-get install clang-18 llvm-18`.
See the [asset build reference](../crates/kronika-report/assets/README.md) for tool paths.

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.96.0
make ui-install
make report-assets REPORT_ASSET_FLAGS=--download-bindgen
make report-assets-check REPORT_ASSET_FLAGS=--download-bindgen
```

These commands rebuild the web page, the HTML report interface and its
WebAssembly engine, then check that they match the files in the repository. The report's query engine executes on the browser's main
thread. Target definitions: [Makefile](../Makefile).

### Chromium and WebKit regression

After `make ui-install`, install the browsers selected by the locked Playwright
version and run the production-artifact interaction suite:

```sh
npm --prefix bins/kronika-web/ui exec -- playwright install --with-deps chromium webkit
make ui-browser-check
```

The suite requires the pinned Rust toolchain: it generates an actual standalone
HTML report from `crates/kronika-report/tests/fixtures/standalone.zms` using
`kronika-report`, then checks its WASM runtime via `file://`. It tests desktop,
320px/390px phone and tablet configurations in both engines, including explicit
export handoff after a delayed response, saved bytes and URL cleanup. Traces,
failure screenshots and input hashes go to
`bins/kronika-web/ui/.browser-results/`. Existing Chrome/CDP coverage remains
available with `npm --prefix bins/kronika-web/ui run test:browser:artifact`.

To explicitly use an installed Chrome instead of Playwright's Chromium locally,
run `KRONIKA_CHROMIUM_CHANNEL=chrome make ui-browser-check`; the selected browser
version and channel are recorded with the inputs. CI uses Playwright's Chromium.

These tests use a fixture HTTP API and emulated touch events. They do not certify
native Safari, iOS Files, a physical keyboard, safe areas, VoiceOver, memory limits
or the Safari 16.4 floor. Track those separately in the
[Safari R1 specification](safari-compat-plan.ru.md).

## Archive

From a clean committed checkout:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

[Archive reference](releases.md) defines targets, output names, build metadata,
checksums, packaged documents and CI checks.
