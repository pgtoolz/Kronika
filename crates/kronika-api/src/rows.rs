//! Physical row and history request parameters.

use kronika_query::{ActiveCursor, DataRequest, Order, RowsRequest, SegmentRequest};

use super::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, RouteError};

use crate::parameters::{active_cursor, bounded, data_parameters, unsigned_32};

pub(super) fn parse_data(segment: SegmentRequest, query: &str) -> Result<DataRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut after = None;
    let mut type_id = None;
    for (name, value) in extras {
        match name.as_str() {
            "after" if after.is_none() => after = Some(active_cursor("after", &value)?),
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(DataRequest {
        segment,
        fields,
        filters,
        type_id,
        after,
    })
}

pub(super) fn parse_rows(segment: SegmentRequest, query: &str) -> Result<RowsRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut order = Order::Asc;
    let mut page_size = DEFAULT_PAGE_SIZE;
    let mut cursor = None;
    let mut after = None;
    let mut type_id = None;
    let mut saw_order = false;
    let mut saw_page_size = false;
    for (name, value) in extras {
        match name.as_str() {
            "order" if !saw_order => {
                order = match value.as_str() {
                    "asc" => Order::Asc,
                    "desc" => Order::Desc,
                    _ => return Err(RouteError::BadParameter(name)),
                };
                saw_order = true;
            }
            "page_size" if !saw_page_size => {
                page_size = bounded("page_size", &value, MAX_PAGE_SIZE)?;
                saw_page_size = true;
            }
            "cursor" if cursor.is_none() && !value.is_empty() => cursor = Some(value),
            "after" if after.is_none() => after = Some(active_cursor("after", &value)?),
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(RowsRequest {
        data: DataRequest {
            segment,
            fields,
            filters,
            type_id,
            after,
        },
        order,
        page_size,
        cursor,
    })
}

fn validate_after(segment: &SegmentRequest, after: Option<ActiveCursor>) -> Result<(), RouteError> {
    if after.is_some_and(|cursor| cursor.segment_id != segment.segment_id) {
        return Err(RouteError::BadParameter("after".to_owned()));
    }
    Ok(())
}
