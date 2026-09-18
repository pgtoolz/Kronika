//! Schedule-independent source acquisition with collector diagnostics.

use super::io::{collected_rows, intern_str};
use crate::logging::log_collection_finish;
use kronika_registry::{Section, os_numa::OsNuma, os_topology::OsTopology};
use kronika_source_os::{ProcFs, SysFs, topology};
use kronika_writer::Interner;
use std::time::Instant;

pub(super) fn collect_numa(sys: &SysFs, scope: u8, ts: i64) -> Vec<OsNuma> {
    let started = Instant::now();
    let rows = topology::collect_numa(sys, scope, ts);
    if !rows.is_empty() {
        log_collection_finish(
            OsNuma::CONTRACT.type_id.get(),
            "sysfs",
            rows.len(),
            started.elapsed(),
        );
    }
    rows
}

pub(super) fn collect_topology(
    fs: &ProcFs,
    sys: &SysFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
) -> Vec<OsTopology> {
    let started = Instant::now();
    let type_id = OsTopology::CONTRACT.type_id.get();
    let rows = topology::collect_topology(fs, sys, scope, ts, |value| {
        intern_str(interner, type_id, "cpuinfo", value)
    });
    collected_rows(rows, "cpuinfo", started)
}
