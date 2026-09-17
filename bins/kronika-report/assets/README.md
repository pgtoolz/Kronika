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
across build hosts. Compression uses the lockfile-pinned `pako` encoder from
the UI dependencies because Apple and GNU gzip produce different bytes.

The raw generated WebAssembly is 10,068,598 bytes. Its committed gzip form is
2,428,738 bytes with SHA-256
`197e5c32d05271ee5ffcf33b124b9d79807122436e43cc32c80b4bf5e53f6dc1`.
The 3,885-byte JavaScript binding has SHA-256
`4635ae734e8c1e1aeb463ae1096f4fdc2a65d98e715b55cee9fe46956f29cba8`.
