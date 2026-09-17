#[test]
fn core_storage_module_has_no_platform_or_transport_identifiers() {
    let source = include_str!("../resource.rs");
    for rejected in [
        "std::path",
        "std::fs::File",
        "LocalDir",
        "hyper::",
        "tokio::",
        "http::",
    ] {
        assert!(!source.contains(rejected), "core API contains {rejected}");
    }
}
