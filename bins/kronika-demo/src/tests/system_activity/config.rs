use super::{
    CPU_ENV, DIRECTORY_ENV, DISK_RATE_ENV, ENABLED_ENV, FILE_ENV, FLUSH_ENV, MEMORY_ENV,
    NETWORK_RATE_ENV, SystemActivityConfig,
};
use std::path::Path;

fn read(values: &[(&str, &str)]) -> anyhow::Result<Option<SystemActivityConfig>> {
    let mut command = crate::config::command().mut_args(|arg| arg.env(None::<&str>));
    let mut args = vec!["kronika-demo".to_owned()];
    for (key, value) in values {
        let long = command
            .get_arguments()
            .find(|arg| arg.get_id().as_str() == *key)
            .unwrap()
            .get_long()
            .unwrap();
        args.push(format!("--{long}"));
        args.push((*value).to_owned());
    }
    let matches = command.try_get_matches_from_mut(args)?;
    SystemActivityConfig::from_matches(&matches, Path::new("/demo"), Path::new("/demo/segments"))
}

#[test]
fn the_default_profile_is_enabled_and_bounded() {
    let config = read(&[]).unwrap().unwrap();
    assert_eq!(config.directory, Path::new("/demo/system-activity"));
    assert_eq!(config.cpu_percent, 12);
    assert_eq!(config.memory_mib, 32);
    assert_eq!(config.file_mib, 8);
    assert_eq!(config.disk_kib_per_s, 32);
    assert_eq!(config.network_kib_per_s, 32);
    assert_eq!(config.flush_interval_s, 5);
}

#[test]
fn the_explicit_flag_disables_the_profile_without_reading_other_controls() {
    let config = read(&[(ENABLED_ENV, "false"), (CPU_ENV, "not-a-number")]).unwrap();
    assert!(config.is_none());
}

#[test]
fn booleans_and_numeric_controls_are_strict() {
    assert!(read(&[(ENABLED_ENV, "1")]).is_err());
    for (key, value) in [
        (CPU_ENV, "0"),
        (CPU_ENV, "26"),
        (MEMORY_ENV, "7"),
        (MEMORY_ENV, "129"),
        (FILE_ENV, "0"),
        (FILE_ENV, "33"),
        (DISK_RATE_ENV, "0"),
        (DISK_RATE_ENV, "257"),
        (NETWORK_RATE_ENV, "0"),
        (NETWORK_RATE_ENV, "257"),
        (FLUSH_ENV, "0"),
        (FLUSH_ENV, "11"),
    ] {
        let error = read(&[(key, value)]).unwrap_err().to_string();
        assert!(error.contains(key), "{key} is missing from {error:?}");
    }
}

#[test]
fn the_scratch_path_cannot_contain_or_be_contained_by_storage() {
    for directory in ["/demo", "/demo/segments", "/demo/segments/activity"] {
        let error = read(&[(DIRECTORY_ENV, directory)]).unwrap_err().to_string();
        assert!(error.contains(DIRECTORY_ENV));
    }
    assert!(read(&[(DIRECTORY_ENV, "/demo/elsewhere")]).is_ok());
}

#[test]
fn lexical_parent_components_cannot_hide_a_storage_overlap() {
    assert!(read(&[(DIRECTORY_ENV, "/demo/other/../segments/activity")]).is_err());
}

#[test]
fn a_flush_cannot_cover_more_data_than_the_ring_holds() {
    let error = read(&[(FILE_ENV, "1"), (DISK_RATE_ENV, "256"), (FLUSH_ENV, "10")])
        .unwrap_err()
        .to_string();
    assert!(error.contains(DISK_RATE_ENV));
    assert!(error.contains(FLUSH_ENV));
    assert!(error.contains(FILE_ENV));
}

#[test]
fn a_blank_explicit_directory_is_rejected() {
    let error = read(&[(DIRECTORY_ENV, "  ")]).unwrap_err().to_string();
    assert!(error.contains(DIRECTORY_ENV));
}
