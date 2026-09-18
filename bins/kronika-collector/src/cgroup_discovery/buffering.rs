//! Admit source-owned cgroup rows into collector buffers.

use anyhow::Result;
use kronika_registry::{StrId, os_cgroup_context::OsCgroupContextV2};
use kronika_source_os::cgroup::AncestorContext;
use kronika_source_os::cgroup::discovered_sections::{self, DiscoveredSection};
use kronika_source_os::cgroup::discovery::{DiscoveredGroup, DiscoveredIo};
use kronika_writer::{Interner, SectionBuffers};

use crate::buffering::buffer_row;

fn intern(interner: &mut Interner, value: &str) -> Result<StrId> {
    interner
        .intern(value.as_bytes())
        .map(|id| StrId(id.get()))
        .map_err(|error| anyhow::anyhow!("intern discovered cgroup: {error}"))
}

pub(crate) fn context_section(
    interner: &mut Interner,
    selected: &AncestorContext,
) -> Result<OsCgroupContextV2> {
    discovered_sections::context_section(selected, |value| intern(interner, value))
}

pub(super) fn push_group(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    group: &DiscoveredGroup,
    primary: Option<&AncestorContext>,
) -> Result<()> {
    discovered_sections::emit_group_sections(
        group,
        primary,
        |value| intern(interner, value),
        |row| push(buffers, &row),
    )
}

pub(super) fn push_io(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    row: &DiscoveredIo,
    primary: Option<&AncestorContext>,
) -> Result<()> {
    discovered_sections::emit_io_sections(
        row,
        primary,
        |value| intern(interner, value),
        |row| push(buffers, &row),
    )
}

fn push(buffers: &mut SectionBuffers, row: &DiscoveredSection) -> Result<()> {
    match row {
        DiscoveredSection::Group(row) => buffer_row(buffers, *row),
        DiscoveredSection::Cpu(row) => buffer_row(buffers, *row),
        DiscoveredSection::Memory(row) => buffer_row(buffers, *row),
        DiscoveredSection::Pids(row) => buffer_row(buffers, *row),
        DiscoveredSection::Io(row) => buffer_row(buffers, *row),
        DiscoveredSection::PrimaryCpu(row) => buffer_row(buffers, *row),
        DiscoveredSection::PrimaryMemory(row) => buffer_row(buffers, *row),
        DiscoveredSection::PrimaryPids(row) => buffer_row(buffers, *row),
        DiscoveredSection::PrimaryIo(row) => buffer_row(buffers, *row),
    }
}

#[cfg(test)]
#[path = "../tests/cgroup_discovery/buffering.rs"]
mod tests;
