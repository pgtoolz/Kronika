//! Extension selection, permissions, and capability invalidation.

use std::collections::{BTreeMap, BTreeSet};

use crate::extension::{ExtensionSchema, InventoryEntry};
use crate::statements::{StatementsCapability, StatementsVersion};
use crate::store_plans::{Flavour, StorePlansCapability};

use super::super::PgCollector;
use super::super::capabilities::{
    DatabaseCapabilities, ExtensionKind, capabilities_from_inventory, selected_statements,
    selected_statements_info, selected_store_plans, selected_store_plans_info,
};
use super::super::execution::capability_sqlstate;

fn statements(version: StatementsVersion) -> StatementsCapability {
    StatementsCapability {
        version,
        schema: ExtensionSchema::new("monitoring"),
    }
}

fn plans(flavour: Flavour) -> StorePlansCapability {
    StorePlansCapability {
        flavour,
        schema: ExtensionSchema::new("monitoring"),
    }
}

fn capabilities(
    statements: Option<StatementsCapability>,
    plans: Option<StorePlansCapability>,
) -> DatabaseCapabilities {
    DatabaseCapabilities {
        generation: 7,
        statements,
        store_plans: plans,
        ..DatabaseCapabilities::default()
    }
}

#[test]
fn statements_choose_the_richest_layout_before_database_preference() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(Some(statements(StatementsVersion::V1)), None),
        ),
        (
            "beta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
        (
            "zeta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
    ]);

    assert_eq!(
        selected_statements(&capabilities, Some("alpha"), &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned()),
        "an older current-database installation must not hide newer counters"
    );
    assert_eq!(
        selected_statements(&capabilities, Some("zeta"), &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("zeta".to_owned()),
        "the current database wins between equal richest layouts"
    );
    assert_eq!(
        selected_statements(&capabilities, None, &BTreeSet::new()).map(|(database, _, _)| database),
        Some("beta".to_owned()),
        "the database name is the deterministic final tie-break"
    );
}

#[test]
fn statements_fallback_is_deterministic_across_databases_and_layouts() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(Some(statements(StatementsVersion::V1)), None),
        ),
        (
            "beta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
        (
            "zeta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
    ]);
    let mut excluded = BTreeSet::new();

    assert_eq!(
        selected_statements(&capabilities, None, &excluded).map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    excluded.insert("beta".to_owned());
    assert_eq!(
        selected_statements(&capabilities, None, &excluded).map(|(database, _, _)| database),
        Some("zeta".to_owned())
    );
    excluded.insert("zeta".to_owned());
    assert_eq!(
        selected_statements(&capabilities, None, &excluded).map(|(database, _, _)| database),
        Some("alpha".to_owned())
    );
}

#[test]
fn main_readers_and_info_views_are_selected_independently() {
    let schema = ExtensionSchema::new("monitoring");
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            DatabaseCapabilities {
                statements_info: Some(schema.clone()),
                store_plans_info: Some(schema),
                ..DatabaseCapabilities::default()
            },
        ),
        (
            "beta".to_owned(),
            capabilities(
                Some(statements(StatementsVersion::V1)),
                Some(plans(Flavour::OsscCompatible)),
            ),
        ),
    ]);

    assert_eq!(
        selected_statements(&capabilities, None, &BTreeSet::new()).map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_statements_info(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned())
    );
    assert_eq!(
        selected_store_plans(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_store_plans_info(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned())
    );
}

#[test]
fn statements_info_remains_available_without_full_statement_visibility() {
    let capabilities = capabilities_from_inventory(
        &[InventoryEntry {
            name: "pg_stat_statements".to_owned(),
            extversion: "1.12".to_owned(),
            schema: ExtensionSchema::new("monitoring"),
            schema_usable: true,
            full_visibility: false,
            statements_info: true,
            store_plans_info: false,
            statements_reader: true,
            store_plans_zero_arg: false,
            store_plans_bool_arg: false,
            store_plans_key_getter: false,
            store_plans_text_converter: false,
            store_plans_ossc_columns: false,
            store_plans_vadv_columns: false,
            store_plans_datasentinel_columns: false,
        }],
        7,
        18,
    );

    assert!(capabilities.statements.is_none());
    assert_eq!(
        capabilities
            .statements_info
            .as_ref()
            .map(ExtensionSchema::name),
        Some("monitoring")
    );
}

#[test]
fn store_plans_flavours_are_not_ranked() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(None, Some(plans(Flavour::OsscCompatible))),
        ),
        (
            "beta".to_owned(),
            capabilities(None, Some(plans(Flavour::Datasentinel))),
        ),
        (
            "gamma".to_owned(),
            capabilities(None, Some(plans(Flavour::Vadv))),
        ),
        (
            "zeta".to_owned(),
            capabilities(None, Some(plans(Flavour::Vadv))),
        ),
    ]);

    assert_eq!(
        selected_store_plans(&capabilities, Some("alpha"), &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned()),
        "the current database wins without ranking independent implementations"
    );
    assert_eq!(
        selected_store_plans(&capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("alpha".to_owned()),
        "the database name is the deterministic fallback"
    );
}

#[test]
fn capability_invalidation_preserves_independent_info_and_tracks_a_move() {
    let mut sources = PgCollector::default();
    let schema = ExtensionSchema::new("monitoring");
    sources.capabilities.insert(
        "alpha".to_owned(),
        DatabaseCapabilities {
            generation: 7,
            statements: Some(statements(StatementsVersion::V1)),
            store_plans: Some(plans(Flavour::OsscCompatible)),
            statements_info: Some(schema.clone()),
            store_plans_info: Some(schema.clone()),
        },
    );
    sources.invalidate_capability("alpha", ExtensionKind::Statements);
    sources.invalidate_capability("alpha", ExtensionKind::StorePlans);
    assert!(selected_statements(&sources.capabilities, None, &BTreeSet::new()).is_none());
    assert!(selected_store_plans(&sources.capabilities, None, &BTreeSet::new()).is_none());
    assert!(selected_statements_info(&sources.capabilities, None, &BTreeSet::new()).is_some());
    assert!(selected_store_plans_info(&sources.capabilities, None, &BTreeSet::new()).is_some());

    sources.invalidate_capability("alpha", ExtensionKind::StatementsInfo);
    sources.invalidate_capability("alpha", ExtensionKind::StorePlansInfo);
    sources.capabilities.insert(
        "beta".to_owned(),
        DatabaseCapabilities {
            statements_info: Some(schema.clone()),
            store_plans_info: Some(schema),
            ..DatabaseCapabilities::default()
        },
    );
    assert_eq!(
        selected_statements_info(&sources.capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_store_plans_info(&sources.capabilities, None, &BTreeSet::new())
            .map(|(database, _, _)| database),
        Some("beta".to_owned())
    );
}

#[test]
fn runtime_capability_failures_schedule_rediscovery() {
    for code in ["42P01", "42883", "42704", "42703", "3F000", "42501"] {
        assert!(capability_sqlstate(code), "{code}");
    }
    assert!(!capability_sqlstate("22003"));
}
