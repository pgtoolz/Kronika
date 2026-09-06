# Generated report assets

[Русская версия](README.ru.md)

The interface shell is built from the main React sources in
`bins/kronika-web/ui`. The JavaScript bindings and compressed WebAssembly are
built from `crates/kronika-report-wasm` with the repository-pinned Rust and
wasm-bindgen versions. Repository checks reproduce these files byte-for-byte.

Run `scripts/report-assets.sh build` with `WASM_BINDGEN` set to a
`wasm-bindgen 0.2.127` executable. Passing `--download-bindgen` explicitly
downloads the pinned static x86_64 Linux musl release and verifies its SHA-256 before use.
Use `scripts/report-assets.sh check` to compare a fresh build with the committed
JavaScript and deterministic gzip files. `CARGO_BIN` and `NODE_BIN` select the
Cargo and Node executables when they are not first on `PATH`.
The build fixes path remaps for the repository and Cargo home, the
`const-random` seed, and C compiler identification so bytes remain identical
across build hosts.

wasm-bindgen first emits its `web` target. The build inserts one bounded
`initEmbedded` entry point at pinned generated-code markers, then esbuild keeps
only that entry point and `ReportSession` in the classic-script
`KronikaReportWasm` global. The committed binding has no URL or network loader.
The report compiles its embedded bytes and passes the resulting
`WebAssembly.Module` to `initEmbedded`, which instantiates asynchronously.

The raw generated WebAssembly is 9,910,988 bytes. Its committed gzip form is
2,395,947 bytes with SHA-256
`f2d246ed04e6239b27a93e9069e3662bcf740e6480746d52160c9b3aaadaa144`.
The 3,885-byte JavaScript binding has SHA-256
`4635ae734e8c1e1aeb463ae1096f4fdc2a65d98e715b55cee9fe46956f29cba8`.

Each input `Uint8Array` is copied once into WebAssembly linear memory by the
generated binding. Rust adopts those allocations as `Vec<u8>` values and moves
them into the retained `ReportEngine` without another complete ZMS or IDX
copy. Returned NDJSON is assembled from the existing streamed records and is
copied once from WebAssembly into JavaScript.
