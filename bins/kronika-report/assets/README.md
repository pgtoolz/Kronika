# Generated report assets

[Русская версия](README.ru.md)

The interface shell is built from the main React sources in
`bins/kronika-web/ui`. The JavaScript bindings and compressed WebAssembly are
built from `crates/kronika-report-wasm` with the pinned Rust, Clang and
wasm-bindgen versions. Repository checks reproduce these files byte-for-byte.

The reference build uses Ubuntu 24.04, Clang 18.1.3 and `llvm-ar-18`:

```sh
sudo apt-get install clang-18 llvm-18
```

The script defaults to `clang-18` and `llvm-ar-18`. Set
`CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` if those tools have
different paths. Other Clang versions are rejected because they change the
compiled zstd code, even with compiler identification removed.

From the repository root, run `scripts/report-assets.sh build` with
`wasm-bindgen 0.2.127` on `PATH`, or set `WASM_BINDGEN` to its executable path.
`scripts/report-assets.sh build --download-bindgen` downloads the pinned static
x86_64 Linux musl release if no local executable is found, and verifies its
SHA-256 before use.
Use `scripts/report-assets.sh check` to compare a fresh build with the committed
JavaScript and deterministic gzip files. `CARGO_BIN` and `NODE_BIN` select the
Cargo and Node executables when they are not first on `PATH`.
The build fixes path remaps for the repository and Cargo home and the
`const-random` seed, removes C compiler identification, and disables incremental
compilation regardless of the local Cargo setting. Compression uses the lockfile-pinned `pako` encoder from
the UI dependencies because Apple and GNU gzip produce different bytes.

The raw generated WebAssembly is 10,194,936 bytes. Its committed gzip form is
2,471,258 bytes with SHA-256
`60fc644eb5aa944ab28e55c905b67d2730efbea45aad0c285bc535f981f677e9`.
The 3,885-byte JavaScript binding has SHA-256
`4635ae734e8c1e1aeb463ae1096f4fdc2a65d98e715b55cee9fe46956f29cba8`.
