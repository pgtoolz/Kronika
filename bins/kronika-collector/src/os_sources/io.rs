//! Optional procfs reads and dictionary failures shared by OS collectors.

use std::io::ErrorKind;

use kronika_registry::StrId;
use kronika_source_os::ProcFs;
use kronika_writer::Interner;

use crate::logging::{LogLevel, field, layout_id, log_event, section_name};

pub(super) fn read_optional_os_file(
    fs: &ProcFs,
    rel: &'static str,
    type_id: u32,
) -> Option<String> {
    match fs.read_raw(rel) {
        Ok(content) => Some(content),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => {
            log_degraded(type_id, rel, &err);
            None
        }
    }
}

/// Intern one OS string, logging degradation and returning `None` on failure
/// so the caller skips only the affected row.
pub(super) fn intern_str(
    interner: &mut Interner,
    type_id: u32,
    origin: &'static str,
    value: &str,
) -> Option<StrId> {
    match interner.intern(value.as_bytes()) {
        Ok(id) => Some(StrId(id.get())),
        Err(err) => {
            log_degraded(type_id, origin, &err);
            None
        }
    }
}

/// Emit a `collection_degraded` event with the section identity and reason.
pub(super) fn log_degraded(type_id: u32, source: &'static str, reason: &dyn std::fmt::Display) {
    log_event(
        LogLevel::Warn,
        "collection_degraded",
        &[
            field("collection", section_name(type_id)),
            field("type_id", type_id),
            field("layout_id", layout_id(type_id)),
            field("source", source),
            field("reason", reason),
        ],
    );
}
