//! Moving typed rows into the snapshot window.

use crate::logging::{layout_id, section_name};
use anyhow::Result;
use kronika_writer::SectionBuffers;

/// Buffer one typed snapshot row, mapping a full buffer to an error.
pub(crate) fn buffer_row<S: kronika_registry::Section + 'static>(
    buffers: &mut SectionBuffers,
    row: S,
) -> Result<()> {
    let type_id = S::CONTRACT.type_id.get();
    buffers.push(row).map_err(|_row| {
        anyhow::anyhow!(
            "section buffer is full: collection={} type_id={} layout_id={}",
            section_name(type_id),
            type_id,
            layout_id(type_id)
        )
    })
}
