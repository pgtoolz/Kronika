//! `kronika_rank_metrics`: thin MCP adapter for the shared Heatmap batch.

use std::sync::Arc;

use kronika_query::{
    HeatmapBatchQuery, HeatmapBatchResult, HeatmapError, HeatmapItemQuery, HeatmapView, MAX_FIELDS,
    NormalizedRanking, QueryContext, StatementScope, execute_heatmap_batch,
};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::api::ApiError;
use crate::config::Config;
use crate::query_adapter::NativeDataset;
use crate::route::MAX_QUERY_BYTES;

use super::catalog::{OVERVIEW_TOOL, OverviewInput, OverviewRankingInput};
use super::semantics::{
    CancellationSink, arguments_within_budget, invalid_arguments, mcp_error, mcp_error_indexed,
    mcp_error_indexed_with, mcp_structured,
};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    if let Err((index, message)) = check_request_budget(&arguments) {
        return mcp_error_indexed(format!("rankings[{index}]: {message}"), index);
    }
    if let Some(rankings) = arguments.get("rankings").and_then(Value::as_array) {
        for (index, ranking) in rankings.iter().enumerate() {
            if let Err(error) = serde_json::from_value::<OverviewRankingInput>(ranking.clone()) {
                return mcp_error_indexed(
                    format!("invalid {OVERVIEW_TOOL} rankings[{index}]: {error}"),
                    index,
                );
            }
        }
    }
    let input: OverviewInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return invalid_arguments(
                OVERVIEW_TOOL,
                "from, to, and a nonempty rankings array are required; each ranking contains section, 1-4 independently ranked fields, and optional top",
                error,
            );
        }
    };
    if input.rankings.is_empty() {
        return mcp_error_indexed("rankings[0]: rankings must not be empty", 0);
    }
    let range = match super::time::resolve_range(&input.from, &input.to) {
        Ok(range) => range,
        Err(error) => return mcp_error_indexed(format!("rankings[0]: {error}"), 0),
    };
    let mut items = Vec::new();
    let mut source_indices = Vec::new();
    for (index, ranking) in input.rankings.into_iter().enumerate() {
        if !(1..=MAX_FIELDS).contains(&ranking.fields.len()) {
            return mcp_error_indexed(
                format!("rankings[{index}]: fields must contain 1 to {MAX_FIELDS} names"),
                index,
            );
        }
        let top = match usize::try_from(ranking.top) {
            Ok(top) => top,
            Err(_error) => {
                return mcp_error_indexed(
                    format!("rankings[{index}]: top does not fit this platform"),
                    index,
                );
            }
        };
        for field in ranking.fields {
            items.push(HeatmapItemQuery {
                ranking: NormalizedRanking {
                    section: ranking.section.clone(),
                    fields: vec![field],
                    top,
                },
                view: HeatmapView::RankingOnly,
                scope: StatementScope::All,
            });
            source_indices.push(index);
        }
    }
    let dataset = match NativeDataset::from_root(&config.data_root) {
        Ok(dataset) => Arc::new(dataset),
        Err(error) => {
            let message = super::semantics::storage_error_message(&ApiError::from(error));
            return mcp_error_indexed(format!("rankings[0]: {message}"), 0);
        }
    };
    let context = QueryContext::new(dataset, config.sources, config.synthetic_demo);
    let result = match execute_heatmap_batch(
        &context,
        HeatmapBatchQuery { range, items },
        &CancellationSink::new(cancelled),
    ) {
        Ok(result) => result,
        Err(error) => return heatmap_error(error, &source_indices),
    };
    let structured = match public_result(&result) {
        Ok(value) => value,
        Err(_error) => return mcp_error("could not produce detail_ref"),
    };
    mcp_structured(structured)
}

fn heatmap_error(error: HeatmapError, source_indices: &[usize]) -> CallToolResult {
    let expanded_index = error.ranking_index();
    let ranking_index = source_indices
        .get(expanded_index)
        .copied()
        .unwrap_or(expanded_index);
    let valid_options = error.valid_options().to_vec();
    let api_error = error.into_query();
    let message = super::semantics::storage_error_message(&api_error);
    let message = if matches!(api_error, ApiError::BadLocator(_)) {
        message
    } else {
        format!("rankings[{ranking_index}]: {message}")
    };
    mcp_error_indexed_with(message, ranking_index, valid_options)
}

fn public_result(result: &HeatmapBatchResult) -> Result<Value, String> {
    let mut structured = serde_json::to_value(result).map_err(|error| error.to_string())?;
    let results = structured["results"]
        .as_array_mut()
        .ok_or_else(|| "overview results are not an array".to_owned())?;
    if results.len() != result.results.len() {
        return Err("overview result count changed during encoding".to_owned());
    }
    for (typed, encoded) in result.results.iter().zip(results) {
        let entities = encoded["entities"]
            .as_array_mut()
            .ok_or_else(|| "overview entities are not an array".to_owned())?;
        if entities.len() != typed.entities.len() {
            return Err("overview entity count changed during encoding".to_owned());
        }
        for (typed, encoded) in typed.entities.iter().zip(entities) {
            super::semantics::set_detail_ref(encoded, typed.detail_ref()?)?;
        }
    }
    Ok(structured)
}

fn check_request_budget(arguments: &Map<String, Value>) -> Result<(), (usize, String)> {
    if arguments_within_budget(arguments) {
        return Ok(());
    }
    let rankings = arguments
        .get("rankings")
        .and_then(Value::as_array)
        .ok_or_else(|| (0, request_overflow_message()))?;
    let mut without_rankings = arguments.clone();
    without_rankings.insert("rankings".to_owned(), Value::Array(Vec::new()));
    let mut used = serde_json::to_vec(&Value::Object(without_rankings))
        .map_err(|error| (0, format!("could not measure arguments: {error}")))?
        .len();
    if used > MAX_QUERY_BYTES {
        return Err((0, request_overflow_message()));
    }
    for (index, ranking) in rankings.iter().enumerate() {
        if index > 0 {
            used = used
                .checked_add(1)
                .ok_or_else(|| (index, request_overflow_message()))?;
        }
        used = used
            .checked_add(serde_json::to_vec(ranking).map_or(0, |bytes| bytes.len()))
            .ok_or_else(|| (index, request_overflow_message()))?;
        if used > MAX_QUERY_BYTES {
            return Err((index, request_overflow_message()));
        }
    }
    Err((rankings.len().saturating_sub(1), request_overflow_message()))
}

fn request_overflow_message() -> String {
    format!("overview arguments exceed {MAX_QUERY_BYTES} encoded bytes")
}
