//! Cross-module web tests. Unit tests live here too and are loaded by their
//! owning modules through `#[path]` so they retain access to private items.

pub(crate) mod artifacts;
mod http;
mod multi_layout;
