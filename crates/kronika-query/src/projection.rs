//! Per-layout projection, typed equality filters, and chunk dictionaries.

use std::collections::{HashSet, hash_map::RandomState};

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment, StrId};
use kronika_registry::{ColumnClass, ColumnType, TypeContract, contract};

use crate::request::{DataRequest, Filter};
use crate::{DatasetSegment, QueryError};

/// One output field and whether this physical layout carries it.
#[derive(Debug, Clone)]
pub struct OutputField {
    /// Public field name retained in caller order.
    pub name: String,
    /// Physical column backing this output in the selected layout.
    pub column: Option<&'static str>,
}

/// One validated physical-layout query.
#[derive(Debug)]
pub struct Plan {
    /// Physical registry layout identity.
    pub type_id: u32,
    /// Static registry contract for the layout.
    pub contract: &'static TypeContract,
    /// Output projection in caller order.
    pub fields: Vec<OutputField>,
    /// Sorted physical columns required while scanning.
    pub projection: Vec<&'static str>,
    /// Timestamp column, when the layout carries one.
    pub timestamp: Option<&'static str>,
    /// First physical ordinal included by this plan.
    pub start_row: u64,
    filters: Vec<TypedFilter>,
    matches_none: bool,
    /// Number of physical rows available to the plan.
    pub rows: u64,
}

#[derive(Debug)]
enum TypedFilter {
    Cell {
        column: &'static str,
        wanted: Cell,
    },
    Bytes {
        column: &'static str,
        wanted: Vec<u8>,
        wanted_id: Option<u64>,
    },
}

/// Build compatible per-physical-layout plans without merging identities.
///
/// # Errors
///
/// Returns a query error when the requested section, projection, or filters are invalid.
pub fn plans(
    segment: &Segment,
    request: &DataRequest,
    history_coordinates: bool,
) -> Result<Vec<Plan>, QueryError> {
    let layouts: Vec<(u32, kronika_reader::Section)> = segment
        .layouts(&request.segment.section)
        .filter(|(type_id, _section)| request.type_id.is_none_or(|wanted| wanted == *type_id))
        .collect();
    if layouts.is_empty() {
        return Err(QueryError::NoSuchSection);
    }
    let contracts: Vec<&'static TypeContract> = layouts
        .iter()
        .map(|(type_id, _section)| contract(*type_id).ok_or(QueryError::NoSuchSection))
        .collect::<Result<_, _>>()?;
    let output_names = output_names(&contracts, &request.fields)?;
    validate_filter_names(&contracts, &request.filters)?;

    layouts
        .into_iter()
        .zip(contracts)
        .map(|((type_id, section), contract)| {
            let fields: Vec<OutputField> = output_names
                .iter()
                .map(|name| OutputField {
                    name: name.clone(),
                    column: contract.column(name).map(|column| column.name),
                })
                .collect();
            let mut matches_none = false;
            let filters = request
                .filters
                .iter()
                .filter_map(|filter| match typed_filter(contract, filter) {
                    Ok(Some(filter)) => Some(Ok(filter)),
                    Ok(None) => {
                        matches_none = true;
                        None
                    }
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let timestamp = contract
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name);
            let projection =
                projection(contract, &fields, timestamp, &filters, history_coordinates);
            Ok(Plan {
                type_id,
                contract,
                fields,
                projection,
                timestamp,
                start_row: 0,
                filters,
                matches_none,
                rows: section.rows,
            })
        })
        .collect()
}

pub(crate) fn apply_tail(
    plans: &mut [Plan],
    prior: Option<&DatasetSegment>,
) -> Result<(), QueryError> {
    let Some(prior) = prior else {
        return Ok(());
    };
    for plan in plans {
        plan.start_row = prior
            .sections()
            .iter()
            .find(|section| section.type_id == plan.type_id)
            .map_or(0, |section| section.rows);
        if plan.start_row > plan.rows {
            return Err(QueryError::BadCursor);
        }
    }
    Ok(())
}

fn projection(
    contract: &'static TypeContract,
    fields: &[OutputField],
    timestamp: Option<&'static str>,
    filters: &[TypedFilter],
    history_coordinates: bool,
) -> Vec<&'static str> {
    let mut projection: Vec<&'static str> = fields
        .iter()
        .filter_map(|field| field.column)
        .chain(filters.iter().map(TypedFilter::column))
        .collect();
    if history_coordinates {
        projection.extend(crate::row_key::identity_columns(contract));
        projection.extend(timestamp);
    }
    projection.sort_unstable();
    projection.dedup();
    projection
}

fn output_names(
    contracts: &[&'static TypeContract],
    requested: &[String],
) -> Result<Vec<String>, QueryError> {
    if !requested.is_empty() {
        for name in requested {
            if !contracts
                .iter()
                .any(|contract| contract.column(name).is_some())
            {
                return Err(QueryError::NoSuchColumn(name.clone()));
            }
        }
        return Ok(requested.to_vec());
    }

    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for contract in contracts {
        for column in contract.columns {
            if seen.insert(column.name) {
                names.push(column.name.to_owned());
            }
        }
    }
    Ok(names)
}

fn validate_filter_names(
    contracts: &[&'static TypeContract],
    filters: &[Filter],
) -> Result<(), QueryError> {
    for filter in filters {
        if !contracts
            .iter()
            .any(|contract| contract.column(&filter.column).is_some())
        {
            return Err(QueryError::NoSuchColumn(filter.column.clone()));
        }
    }
    Ok(())
}

pub(crate) fn chunk_dictionary(
    segment: &Segment,
    rows: &[(u64, Row)],
) -> Result<Dictionary, QueryError> {
    let (dictionary, ids) = dictionary_for_chunk(segment, rows)?;
    if let Some(unresolved) = ids
        .iter()
        .copied()
        .find(|id| dictionary.resolve(*id).is_none())
    {
        return Err(unresolved_dictionary(unresolved));
    }
    Ok(dictionary)
}

pub(crate) fn streaming_chunk_dictionary(
    segment: &Segment,
    rows: &[(u64, Row)],
) -> Result<Dictionary, QueryError> {
    dictionary_for_chunk(segment, rows).map(|(dictionary, _ids)| dictionary)
}

fn dictionary_for_chunk(
    segment: &Segment,
    rows: &[(u64, Row)],
) -> Result<(Dictionary, HashSet<u64>), QueryError> {
    let ids: HashSet<u64> = rows
        .iter()
        .flat_map(|(_ordinal, row)| row.iter())
        .filter_map(|(_name, cell)| match cell {
            Cell::StrId(id) => Some(*id),
            _ => None,
        })
        .collect();
    let dictionary = segment.dictionary_for(&ids)?;
    Ok((dictionary, ids))
}

/// Resolve an exact set of dictionary identities and reject missing entries.
///
/// # Errors
///
/// Returns a query error when the dictionary cannot be read or an identity is unresolved.
pub fn resolved_dictionary(
    segment: &Segment,
    ids: &HashSet<u64, RandomState>,
) -> Result<Dictionary, QueryError> {
    let dictionary = segment.dictionary_for(ids)?;
    if let Some(unresolved) = ids
        .iter()
        .copied()
        .find(|id| dictionary.resolve(*id).is_none())
    {
        return Err(unresolved_dictionary(unresolved));
    }
    Ok(dictionary)
}

fn unresolved_dictionary(id: u64) -> QueryError {
    QueryError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("unresolved dictionary id {id}"),
    )))
}

pub(crate) fn validate_row_dictionary(
    row: &Row,
    dictionary: &Dictionary,
) -> Result<(), QueryError> {
    if let Some(unresolved) = row.iter().find_map(|(_name, cell)| match cell {
        Cell::StrId(id) if dictionary.resolve(*id).is_none() => Some(*id),
        _ => None,
    }) {
        return Err(unresolved_dictionary(unresolved));
    }
    Ok(())
}

impl Plan {
    /// Whether this physical layout can satisfy every typed filter.
    #[must_use]
    pub const fn applies(&self) -> bool {
        !self.matches_none
    }

    /// Whether one row satisfies every typed filter.
    #[must_use]
    pub fn matches(&self, row: &Row, dictionary: &Dictionary) -> bool {
        !self.matches_none
            && self
                .filters
                .iter()
                .all(|filter| filter.matches(row, dictionary))
    }

    /// Resolve dictionary entries needed only for exact filter matching.
    ///
    /// # Errors
    ///
    /// Returns a query error when the dictionary cannot be read or an identity is unresolved.
    pub fn selection_dictionary(
        &self,
        segment: &Segment,
        rows: &[(u64, Row)],
    ) -> Result<Dictionary, QueryError> {
        let mut ids = HashSet::new();
        for (_ordinal, row) in rows {
            self.add_selection_ids(row, &mut ids);
        }
        resolved_dictionary(segment, &ids)
    }

    /// Load dictionary entries addressed directly by typed string filters.
    ///
    /// # Errors
    ///
    /// Returns a query error when the dictionary cannot be read.
    pub fn exact_filter_dictionary(&self, segment: &Segment) -> Result<Dictionary, QueryError> {
        let ids = self
            .filters
            .iter()
            .filter_map(|filter| match filter {
                TypedFilter::Bytes { wanted_id, .. } => *wanted_id,
                TypedFilter::Cell { .. } => None,
            })
            .collect();
        segment.dictionary_for(&ids).map_err(QueryError::from)
    }

    /// Reject a selected string identity that was not present in the dictionary.
    ///
    /// # Errors
    ///
    /// Returns a query error when a selected identity is unresolved.
    pub fn validate_exact_filter_ids(
        &self,
        row: &Row,
        dictionary: &Dictionary,
    ) -> Result<(), QueryError> {
        for filter in &self.filters {
            let TypedFilter::Bytes {
                column, wanted_id, ..
            } = filter
            else {
                continue;
            };
            let Some(Cell::StrId(actual)) = row.get(column) else {
                continue;
            };
            if *wanted_id == Some(*actual) && dictionary.resolve(*actual).is_none() {
                return Err(unresolved_dictionary(*actual));
            }
        }
        Ok(())
    }

    /// Add dictionary identities needed to evaluate this plan's filters.
    pub fn add_selection_ids(&self, row: &Row, ids: &mut HashSet<u64>) {
        for filter in &self.filters {
            if let TypedFilter::Bytes { column, .. } = filter
                && let Some(Cell::StrId(id)) = row.get(column)
            {
                ids.insert(*id);
            }
        }
    }

    /// Whether evaluating this plan requires a dictionary lookup.
    #[must_use]
    pub fn needs_selection_dictionary(&self) -> bool {
        self.filters
            .iter()
            .any(|filter| matches!(filter, TypedFilter::Bytes { .. }))
    }

    /// Add physical inputs needed for computation without exposing them as output fields.
    pub fn add_projection_columns(&mut self, names: &[&'static str]) {
        self.projection.extend(names.iter().copied());
        self.projection.sort_unstable();
        self.projection.dedup();
    }

    /// Keep only the requested physical output fields.
    pub fn retain_output_fields(&mut self, names: &[String]) {
        self.fields.retain(|field| names.contains(&field.name));
    }

    /// Add a server-derived output field backed by separately captured data.
    pub fn add_virtual_output(&mut self, name: &str) {
        if self.fields.iter().all(|field| field.name != name) {
            self.fields.push(OutputField {
                name: name.to_owned(),
                column: None,
            });
        }
    }

    /// Add another public reading backed by an already projected column.
    pub fn add_aliased_output(&mut self, name: &str, column: &'static str) {
        if self.fields.iter().any(|field| field.name == column)
            && self.fields.iter().all(|field| field.name != name)
        {
            self.fields.push(OutputField {
                name: name.to_owned(),
                column: Some(column),
            });
        }
    }

    /// Restore caller field order after physical and virtual planning.
    pub fn order_output_fields(&mut self, names: &[String]) {
        self.fields.sort_by_key(|field| {
            names
                .iter()
                .position(|name| name == &field.name)
                .unwrap_or(usize::MAX)
        });
    }
}

impl TypedFilter {
    const fn column(&self) -> &'static str {
        match self {
            Self::Cell { column, .. } | Self::Bytes { column, .. } => column,
        }
    }

    fn matches(&self, row: &Row, dictionary: &Dictionary) -> bool {
        match self {
            Self::Cell { column, wanted } => row
                .get(column)
                .is_some_and(|actual| cells_equal(actual, wanted)),
            Self::Bytes { column, wanted, .. } => {
                let Some(Cell::StrId(id)) = row.get(column) else {
                    return false;
                };
                match dictionary.resolve(*id) {
                    Some(Resolved::Str(bytes)) => bytes == wanted,
                    Some(Resolved::Blob(blob)) => !blob.truncated && blob.stored_bytes == wanted,
                    None => false,
                }
            }
        }
    }
}

fn typed_filter(
    contract: &'static TypeContract,
    filter: &Filter,
) -> Result<Option<TypedFilter>, QueryError> {
    let Some(column) = contract.column(&filter.column) else {
        return Ok(None);
    };
    let bad = || QueryError::BadFilter(filter.column.clone());
    if column.class != ColumnClass::Label {
        return Err(bad());
    }
    let value = filter.value.as_str();
    let wanted = match column.ty {
        ColumnType::I8 => Cell::I16(i16::from(value.parse::<i8>().map_err(|_error| bad())?)),
        ColumnType::I16 => Cell::I16(value.parse().map_err(|_error| bad())?),
        ColumnType::I32 => Cell::I32(value.parse().map_err(|_error| bad())?),
        ColumnType::I64 => Cell::I64(value.parse().map_err(|_error| bad())?),
        ColumnType::U8 => Cell::U32(u32::from(value.parse::<u8>().map_err(|_error| bad())?)),
        ColumnType::U16 => Cell::U32(u32::from(value.parse::<u16>().map_err(|_error| bad())?)),
        ColumnType::U32 => Cell::U32(value.parse().map_err(|_error| bad())?),
        ColumnType::U64 => Cell::U64(value.parse().map_err(|_error| bad())?),
        ColumnType::F32 => {
            let parsed: f32 = value.parse().map_err(|_error| bad())?;
            if !parsed.is_finite() {
                return Err(bad());
            }
            Cell::F64(f64::from(parsed))
        }
        ColumnType::F64 => {
            let parsed: f64 = value.parse().map_err(|_error| bad())?;
            if !parsed.is_finite() {
                return Err(bad());
            }
            Cell::F64(parsed)
        }
        ColumnType::Bool => Cell::Bool(value.parse().map_err(|_error| bad())?),
        ColumnType::Ts => Cell::Ts(value.parse().map_err(|_error| bad())?),
        ColumnType::StrId => {
            let wanted = value.as_bytes().to_vec();
            return Ok(Some(TypedFilter::Bytes {
                column: column.name,
                wanted,
                wanted_id: StrId::of(value.as_bytes()).map(StrId::get),
            }));
        }
        ColumnType::ListI32 => {
            let values = if value.is_empty() {
                Vec::new()
            } else {
                value
                    .split(',')
                    .map(|part| part.parse().map_err(|_error| bad()))
                    .collect::<Result<_, _>>()?
            };
            Cell::ListI32(values)
        }
    };
    Ok(Some(TypedFilter::Cell {
        column: column.name,
        wanted,
    }))
}

fn cells_equal(actual: &Cell, wanted: &Cell) -> bool {
    match (actual, wanted) {
        (Cell::F64(actual), Cell::F64(wanted)) => actual.to_bits() == wanted.to_bits(),
        _ => actual == wanted,
    }
}

#[cfg(test)]
mod tests;
