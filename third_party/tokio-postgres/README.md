# tokio-postgres (vendored)

Upstream `tokio-postgres` 0.7.18 release source (crates.io tarball) with the
changes of rust-postgres/rust-postgres#1358 ported on top: `SimpleColumn`
exposes the column type OID and type modifier in simple-query results.

The 0.7.18 release itself does not report column types for the simple query
protocol, and the Kronika Prometheus exporter needs exact types to match its
exposition contract. Everything else matches the release byte-for-byte
(plus the manifest adjustments needed for a path dependency).

Delete this directory and drop the `[patch.crates-io]` entry in the workspace
`Cargo.toml` when an upstream release ships the capability.

Upstream licenses: LICENSE-MIT, LICENSE-APACHE (also in NOTICE).
