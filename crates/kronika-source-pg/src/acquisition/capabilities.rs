//! Discovered extension interfaces and deterministic database selection.

use std::collections::{BTreeMap, BTreeSet};

use crate::extension;
use crate::statements::{self, StatementsVersion};
use crate::store_plans;

use super::{PgCollector, PgObservation, PgWarning};

/// Each view is invalidated independently when its discovered interface changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExtensionKind {
    Statements,
    StatementsInfo,
    StorePlans,
    StorePlansInfo,
}

#[derive(Debug, Clone, Default)]
pub(super) struct DatabaseCapabilities {
    pub(super) generation: u64,
    pub(super) statements: Option<statements::StatementsCapability>,
    pub(super) store_plans: Option<store_plans::StorePlansCapability>,
    pub(super) statements_info: Option<extension::ExtensionSchema>,
    pub(super) store_plans_info: Option<extension::ExtensionSchema>,
}

impl PgCollector {
    pub(super) fn invalidate_capability(&mut self, database: &str, kind: ExtensionKind) {
        if let Some(capability) = self.capabilities.get_mut(database) {
            match kind {
                ExtensionKind::Statements => capability.statements = None,
                ExtensionKind::StatementsInfo => capability.statements_info = None,
                ExtensionKind::StorePlans => capability.store_plans = None,
                ExtensionKind::StorePlansInfo => capability.store_plans_info = None,
            }
        }
        self.last_discovery = None;
    }
}

pub(super) fn capabilities_from_inventory(
    inventory: &[extension::InventoryEntry],
    generation: u64,
    server_major: u32,
) -> DatabaseCapabilities {
    let statements_entry = inventory
        .iter()
        .find(|entry| entry.name == statements::EXTENSION);
    let store_plans_entry = inventory
        .iter()
        .find(|entry| entry.name == store_plans::EXTENSION);
    DatabaseCapabilities {
        generation,
        statements: statements_entry.and_then(|entry| statements::capability(entry, server_major)),
        store_plans: store_plans_entry.and_then(store_plans::capability),
        statements_info: statements_entry
            .filter(|entry| entry.schema_usable && entry.statements_info)
            .map(|entry| entry.schema.clone()),
        store_plans_info: store_plans_entry
            .filter(|entry| entry.schema_usable && entry.store_plans_info)
            .map(|entry| entry.schema.clone()),
    }
}

pub(super) fn warn_outdated_statements_layout(
    inventory: &[extension::InventoryEntry],
    server_major: u32,
    database: &str,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) {
    if server_major < 14 {
        return;
    }
    let Some(entry) = inventory
        .iter()
        .find(|entry| entry.name == statements::EXTENSION)
    else {
        return;
    };
    let outdated = extension::parse_version(&entry.extversion)
        .and_then(statements::statements_version)
        .is_some_and(|version| matches!(version, StatementsVersion::V1 | StatementsVersion::V2));
    if outdated {
        observe(PgObservation::Warning(
            PgWarning::StatementsExtensionUpdateRequired {
                database: database.to_owned(),
                extension_version: entry.extversion.clone(),
            },
        ));
    }
}

fn selected_capability<T: Clone>(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
    get: impl Fn(&DatabaseCapabilities) -> Option<&T>,
) -> Option<(String, u64, T)> {
    if let Some(current) = current_database
        && !excluded.contains(current)
        && let Some(capabilities) = capabilities.get(current)
        && let Some(capability) = get(capabilities)
    {
        return Some((
            current.to_owned(),
            capabilities.generation,
            capability.clone(),
        ));
    }
    capabilities
        .iter()
        .filter(|(database, _capabilities)| !excluded.contains(*database))
        .find_map(|(database, capabilities)| {
            get(capabilities).map(|capability| {
                (
                    database.clone(),
                    capabilities.generation,
                    capability.clone(),
                )
            })
        })
}

pub(super) const fn capabilities_match_generation(cached: Option<u64>, live: Option<u64>) -> bool {
    matches!((cached, live), (Some(cached), Some(live)) if cached == live)
}

pub(super) fn selected_statements(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, statements::StatementsCapability)> {
    let richest = capabilities
        .iter()
        .filter(|(database, _capabilities)| !excluded.contains(*database))
        .filter_map(|(_database, entry)| entry.statements.as_ref())
        .map(|capability| statements_rank(capability.version))
        .max()?;
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry
            .statements
            .as_ref()
            .filter(|capability| statements_rank(capability.version) == richest)
    })
}

const fn statements_rank(version: StatementsVersion) -> u8 {
    match version {
        StatementsVersion::V1 => 1,
        StatementsVersion::V2 => 2,
        StatementsVersion::V3 => 3,
        StatementsVersion::V4 => 4,
        StatementsVersion::V5 => 5,
        StatementsVersion::V6 => 6,
    }
}

pub(super) fn selected_statements_info(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, extension::ExtensionSchema)> {
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry.statements_info.as_ref()
    })
}

pub(super) fn selected_store_plans(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, store_plans::StorePlansCapability)> {
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry.store_plans.as_ref()
    })
}

pub(super) fn selected_store_plans_info(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, extension::ExtensionSchema)> {
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry.store_plans_info.as_ref()
    })
}
