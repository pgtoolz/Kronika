# Generated report assets

[Русская версия](README.ru.md)

The interface shell is built from the main React sources in
`bins/kronika-web/ui`. The JavaScript bindings and compressed WebAssembly are
built from `crates/kronika-report-wasm` with the repository-pinned Rust and
wasm-bindgen versions. Repository checks reproduce these files byte-for-byte.

From the repository root, run `scripts/report-assets.sh build` with
`wasm-bindgen 0.2.127` on `PATH`, or set `WASM_BINDGEN` to its executable path.
`scripts/report-assets.sh build --download-bindgen` downloads the pinned static
x86_64 Linux musl release if no local executable is found, and verifies its
SHA-256 before use.
Use `scripts/report-assets.sh check` to compare a fresh build with the committed
JavaScript and deterministic gzip files. `CARGO_BIN` and `NODE_BIN` select the
Cargo and Node executables when they are not first on `PATH`.
The build fixes path remaps for the repository and Cargo home, the
`const-random` seed, and C compiler identification so bytes remain identical
across build hosts.

The raw generated WebAssembly is 10,284,022 bytes. Its committed gzip form is
2,500,294 bytes with SHA-256
`34ad90157da424feaf9e1235a9e058da1e3635da1ec3cb262f2f98bad7e3504e`.
The 3,885-byte JavaScript binding has SHA-256
`4635ae734e8c1e1aeb463ae1096f4fdc2a65d98e715b55cee9fe46956f29cba8`.
