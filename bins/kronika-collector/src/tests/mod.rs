//! Tests covering several collector modules.
//!
//! Unit tests also live in this directory. Their owning modules load them with
//! `#[path]` to preserve access to private items and existing test names.

mod logs;
mod pg_configuration;
mod shutdown;
mod zms;
