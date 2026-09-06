use super::{ExtensionSchema, ExtensionVersion, INVENTORY_QUERY, parse_version};

const fn version(major: u32, minor: u32) -> ExtensionVersion {
    ExtensionVersion { major, minor }
}

#[test]
fn a_two_part_version_reads_as_its_two_numbers() {
    assert_eq!(parse_version("1.10"), Some(version(1, 10)));
    assert_eq!(parse_version("2.0"), Some(version(2, 0)));
}

#[test]
fn a_bare_major_has_a_zero_minor() {
    assert_eq!(parse_version("2"), Some(version(2, 0)));
}

#[test]
fn a_patch_or_suffix_does_not_change_the_column_set() {
    assert_eq!(parse_version("1.10.2"), Some(version(1, 10)));
    assert_eq!(parse_version("1.10-beta1"), Some(version(1, 10)));
}

#[test]
fn something_that_is_not_a_version_is_rejected() {
    assert_eq!(parse_version(""), None);
    assert_eq!(parse_version("unknown"), None);
    assert_eq!(parse_version(".5"), None);
}

#[test]
fn versions_order_by_major_before_minor() {
    assert!(parse_version("1.12") > parse_version("1.9"));
    assert!(parse_version("2.0") > parse_version("1.12"));
}

#[test]
fn schema_qualification_quotes_every_identifier_boundary() {
    let schema = ExtensionSchema::new("odd\"schema; DROP SCHEMA public; --");
    assert_eq!(
        schema.qualify("pg_stat_statements"),
        "\"odd\"\"schema; DROP SCHEMA public; --\".\"pg_stat_statements\""
    );
    assert_eq!(schema.name(), "odd\"schema; DROP SCHEMA public; --");
}

#[test]
fn inventory_is_one_marked_catalog_query_with_exact_capability_checks() {
    assert!(INVENTORY_QUERY.contains("kronika:"));
    assert!(INVENTORY_QUERY.contains("pg_catalog.pg_extension"));
    assert!(INVENTORY_QUERY.contains("pg_catalog.pg_depend"));
    assert!(INVENTORY_QUERY.contains("p.proallargtypes"));
    assert!(INVENTORY_QUERY.contains("p.proargmodes"));
    assert!(INVENTORY_QUERY.contains("p.proargnames"));
    assert!(INVENTORY_QUERY.contains("pg_catalog.generate_subscripts"));
    assert!(INVENTORY_QUERY.contains("actual.function_oid = f.function_oid"));
    assert!(INVENTORY_QUERY.contains("has_schema_privilege"));
    assert!(INVENTORY_QUERY.contains("has_function_privilege"));
    assert!(INVENTORY_QUERY.contains("pg_has_role('pg_read_all_stats', 'USAGE')"));
    assert!(INVENTORY_QUERY.contains("'26 26 20 20'"));
    assert!(INVENTORY_QUERY.contains("f.proname = 'pg_store_plans_textplan'"));
    assert!(INVENTORY_QUERY.contains("f.proargtypes = '25'::pg_catalog.oidvector"));
    assert!(INVENTORY_QUERY.contains("AS store_plans_text_converter"));
    assert!(INVENTORY_QUERY.contains("queryid_stat_statements"));
    assert!(INVENTORY_QUERY.contains("store_plans_ossc_columns"));
    assert!(INVENTORY_QUERY.contains("store_plans_datasentinel_columns"));
    assert!(INVENTORY_QUERY.contains("'datasentinel', 'relids'"));
    assert!(INVENTORY_QUERY.contains("'datasentinel', 'cmd_type'"));
    assert!(INVENTORY_QUERY.contains("pg_catalog.pg_attribute"));
    assert!(INVENTORY_QUERY.contains("a.attname = 'dealloc'"));
    assert!(INVENTORY_QUERY.contains("a.attname = 'stats_reset'"));
    assert!(!INVENTORY_QUERY.contains("$1"));
}
