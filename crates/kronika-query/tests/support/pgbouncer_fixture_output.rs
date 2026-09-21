use kronika_layout::{SegmentAddress, SegmentId};

pub(super) fn write(directory: &tempfile::TempDir, segment_id: i64) {
    if let Some(output) = std::env::var_os("KRONIKA_PGBOUNCER_TEST_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        let address =
            SegmentAddress::new(SegmentId::new(segment_id).expect("id")).expect("address");
        let relative = std::path::PathBuf::from(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component())
            .join(address.zms_name());
        std::fs::create_dir_all(output.join(relative.parent().expect("parent")))
            .expect("directory");
        let source = directory.path().join(&relative);
        std::fs::copy(&source, output.join(relative)).expect("storage fixture");
        std::fs::copy(source, output.join("recording.zms")).expect("report fixture");
        std::fs::write(output.join("segment-id"), segment_id.to_string()).expect("id");
        std::fs::write(output.join("sources"), "3").expect("sources");
    }
}
